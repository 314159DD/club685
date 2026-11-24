//! The rekordbox module (`version.dll`, a proxy DLL rekordbox loads from its own folder): state detection, and the
//! install / remove helper that runs elevated as `club685.exe --install-module` / `--remove-module`.
//!
//! The helper only copies or deletes one file, `<rekordbox folder>\version.dll`. It refuses unless the source is the
//! module bundled with this build, the folder holds a rekordbox.exe of version 6.8.5 and rekordbox is not running.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MODULE_FILE: &str = "version.dll";
pub const REKORDBOX_EXE: &str = "rekordbox.exe";
pub const FLAG_INSTALL: &str = "--install-module";
pub const FLAG_REMOVE: &str = "--remove-module";

/// Whether the bundled module is in the rekordbox folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModuleState {
    /// No rekordbox folder found, or this build has no bundled module to compare with.
    #[default]
    Unknown,
    Missing,
    /// A `version.dll` is there but it is not the bundled one.
    Outdated,
    Installed,
}

impl ModuleState {
    /// Setup (the INSTALL step) is needed.
    pub fn needs_setup(self) -> bool {
        matches!(self, ModuleState::Missing | ModuleState::Outdated)
    }

    pub fn word(self) -> &'static str {
        match self {
            ModuleState::Unknown => "unknown",
            ModuleState::Missing => "not installed",
            ModuleState::Outdated => "outdated",
            ModuleState::Installed => "installed",
        }
    }
}

pub fn sha256_file(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where the bundled module sits: `Module\version.dll` next to the `Build` folder (or next to the exe).
pub fn bundled_module_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLUB685_MODULE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("..").join("Module").join(MODULE_FILE), dir.join("Module").join(MODULE_FILE)].into_iter().find(|p| p.is_file())
}

/// SHA-256 of the module this build ships: the value baked in by `package.ps1`, else the bundled file itself.
pub fn expected_hash() -> Option<String> {
    if let Some(h) = option_env!("CLUB685_MODULE_SHA256") {
        if !h.is_empty() {
            return Some(h.to_ascii_lowercase());
        }
    }
    sha256_file(&bundled_module_path()?)
}

/// The rekordbox folder: that of the running rekordbox.exe, else the first installed 6.x folder under Program Files.
pub fn find_rekordbox_dir(running_exe: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = running_exe {
        if let Some(d) = Path::new(p).parent() {
            return Some(d.to_path_buf());
        }
    }
    let mut roots = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(v) = std::env::var(var) {
            let p = PathBuf::from(v).join("Pioneer");
            if !roots.contains(&p) {
                roots.push(p);
            }
        }
    }
    for root in roots {
        let exact = root.join("rekordbox 6.8.5");
        if exact.join(REKORDBOX_EXE).is_file() {
            return Some(exact);
        }
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut dirs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("rekordbox")) && p.join(REKORDBOX_EXE).is_file())
                .collect();
            dirs.sort();
            if let Some(d) = dirs.pop() {
                return Some(d);
            }
        }
    }
    None
}

pub fn module_state(rb_dir: Option<&Path>, expected: Option<&str>) -> ModuleState {
    let (Some(dir), Some(expected)) = (rb_dir, expected) else { return ModuleState::Unknown };
    let target = dir.join(MODULE_FILE);
    if !target.is_file() {
        return ModuleState::Missing;
    }
    match sha256_file(&target) {
        Some(h) if h == expected => ModuleState::Installed,
        _ => ModuleState::Outdated,
    }
}

/// What the helper needs to know about the machine; injectable for tests.
pub trait Host {
    fn file_version(&self, exe: &Path) -> Option<(u16, u16, u16, u16)>;
    fn rekordbox_running(&self) -> bool;
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    SourceNotBundled,
    NotRekordbox685,
    RekordboxRunning,
    NotOurModule,
    Io(String),
}

impl InstallError {
    pub fn message(&self) -> String {
        match self {
            InstallError::SourceNotBundled => "The module file is not the one bundled with CLUB685. Nothing was changed.".to_string(),
            InstallError::NotRekordbox685 => "That folder does not contain rekordbox 6.8.5. Nothing was changed.".to_string(),
            InstallError::RekordboxRunning => "Close rekordbox first, then click the button again. Nothing was changed.".to_string(),
            InstallError::NotOurModule => "The version.dll in the rekordbox folder is not the CLUB685 module, so it was left alone.".to_string(),
            InstallError::Io(e) => format!("Could not write to the rekordbox folder: {e}"),
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            InstallError::SourceNotBundled => 11,
            InstallError::NotRekordbox685 => 12,
            InstallError::RekordboxRunning => 13,
            InstallError::NotOurModule => 14,
            InstallError::Io(_) => 15,
        }
    }
}

fn check_target(dir: &Path, host: &dyn Host) -> Result<(), InstallError> {
    let exe = dir.join(REKORDBOX_EXE);
    match exe.is_file().then(|| host.file_version(&exe)).flatten() {
        Some((6, 8, 5, _)) => {}
        _ => return Err(InstallError::NotRekordbox685),
    }
    if host.rekordbox_running() {
        return Err(InstallError::RekordboxRunning);
    }
    Ok(())
}

/// Copies `src` to `<dir>\version.dll`. Writes nothing else.
pub fn install(src: &Path, dir: &Path, expected: &str, host: &dyn Host) -> Result<(), InstallError> {
    if sha256_file(src).as_deref() != Some(expected) {
        return Err(InstallError::SourceNotBundled);
    }
    check_target(dir, host)?;
    let target = dir.join(MODULE_FILE);
    std::fs::copy(src, &target).map_err(|e| InstallError::Io(e.to_string()))?;
    if sha256_file(&target).as_deref() != Some(expected) {
        return Err(InstallError::Io("the copy does not match the source".to_string()));
    }
    Ok(())
}

/// Deletes `<dir>\version.dll`, but only when it is the bundled module.
pub fn remove(dir: &Path, expected: &str, host: &dyn Host) -> Result<(), InstallError> {
    check_target(dir, host)?;
    let target = dir.join(MODULE_FILE);
    if !target.exists() {
        return Ok(());
    }
    if sha256_file(&target).as_deref() != Some(expected) {
        return Err(InstallError::NotOurModule);
    }
    std::fs::remove_file(&target).map_err(|e| InstallError::Io(e.to_string()))
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, SW_SHOWNORMAL};

    pub struct RealHost;

    impl Host for RealHost {
        fn file_version(&self, exe: &Path) -> Option<(u16, u16, u16, u16)> {
            club685_capture::window::file_version(&exe.to_string_lossy())
        }

        fn rekordbox_running(&self) -> bool {
            unsafe {
                let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
                let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
                let mut found = false;
                if Process32FirstW(snap, &mut e).is_ok() {
                    loop {
                        let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                        if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(REKORDBOX_EXE) {
                            found = true;
                            break;
                        }
                        if Process32NextW(snap, &mut e).is_err() {
                            break;
                        }
                    }
                }
                let _ = CloseHandle(snap);
                found
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn message(text: &str, error: bool) {
        let t = wide(text);
        unsafe {
            MessageBoxW(None, PCWSTR(t.as_ptr()), w!("CLUB685"), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION });
        }
    }

    /// Runs this exe again elevated (UAC prompt) with `flag "<bundled>" "<dir>"`. True when Windows started it.
    pub fn launch_elevated(flag: &str, bundled: &Path, dir: &Path) -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let exe_w = wide(&exe.to_string_lossy());
        let params = wide(&format!("{flag} \"{}\" \"{}\"", bundled.display(), dir.display()));
        unsafe {
            let r = ShellExecuteW(None, w!("runas"), PCWSTR(exe_w.as_ptr()), PCWSTR(params.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
            r.0 as isize > 32
        }
    }

    /// Entry point of `club685.exe --install-module|--remove-module <bundled> <rekordbox dir>`. Returns the exit code.
    pub fn run_helper(args: &[String]) -> i32 {
        let (Some(mode), Some(src), Some(dir)) = (args.first(), args.get(1), args.get(2)) else {
            message("Usage: club685.exe --install-module \"<bundled version.dll>\" \"<rekordbox folder>\"", true);
            return 2;
        };
        let (src, dir) = (PathBuf::from(src), PathBuf::from(dir));
        let result = (|| {
            let bundled = bundled_module_path().ok_or(InstallError::SourceNotBundled)?;
            let same = match (std::fs::canonicalize(&src), std::fs::canonicalize(&bundled)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if !same {
                return Err(InstallError::SourceNotBundled);
            }
            let expected = expected_hash().ok_or(InstallError::SourceNotBundled)?;
            if mode == FLAG_INSTALL {
                install(&src, &dir, &expected, &RealHost)
            } else {
                remove(&dir, &expected, &RealHost)
            }
        })();
        match result {
            Ok(()) if mode == FLAG_INSTALL => {
                message("The rekordbox module is installed. Start rekordbox.", false);
                0
            }
            Ok(()) => {
                message("The rekordbox module is removed. Restart rekordbox if it is running.", false);
                0
            }
            Err(e) => {
                message(&e.message(), true);
                e.code()
            }
        }
    }
}

#[cfg(windows)]
pub use win::{launch_elevated, run_helper, RealHost};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Fake {
        version: Option<(u16, u16, u16, u16)>,
        running: bool,
    }

    impl Host for Fake {
        fn file_version(&self, _: &Path) -> Option<(u16, u16, u16, u16)> {
            self.version
        }
        fn rekordbox_running(&self) -> bool {
            self.running
        }
    }

    const GOOD: Fake = Fake { version: Some((6, 8, 5, 0)), running: false };

    /// A temp folder with a fake rekordbox.exe and a bundled module file; removed on drop.
    struct Sandbox {
        root: PathBuf,
        rb: PathBuf,
        src: PathBuf,
        hash: String,
    }

    impl Sandbox {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!("club685-install-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            let rb = root.join("rekordbox 6.8.5");
            std::fs::create_dir_all(&rb).unwrap();
            std::fs::write(rb.join(REKORDBOX_EXE), b"fake exe").unwrap();
            let src = root.join("bundled-version.dll");
            std::fs::write(&src, b"module bytes").unwrap();
            let hash = sha256_file(&src).unwrap();
            Sandbox { root, rb, src, hash }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn state_follows_the_target_file() {
        let s = Sandbox::new();
        assert_eq!(module_state(None, Some(&s.hash)), ModuleState::Unknown);
        assert_eq!(module_state(Some(&s.rb), None), ModuleState::Unknown);
        assert_eq!(module_state(Some(&s.rb), Some(&s.hash)), ModuleState::Missing);
        std::fs::write(s.rb.join(MODULE_FILE), b"something else").unwrap();
        assert_eq!(module_state(Some(&s.rb), Some(&s.hash)), ModuleState::Outdated);
        std::fs::write(s.rb.join(MODULE_FILE), b"module bytes").unwrap();
        assert_eq!(module_state(Some(&s.rb), Some(&s.hash)), ModuleState::Installed);
        assert!(ModuleState::Missing.needs_setup() && ModuleState::Outdated.needs_setup());
        assert!(!ModuleState::Installed.needs_setup() && !ModuleState::Unknown.needs_setup());
    }

    #[test]
    fn install_copies_exactly_one_file() {
        let s = Sandbox::new();
        install(&s.src, &s.rb, &s.hash, &GOOD).unwrap();
        assert_eq!(module_state(Some(&s.rb), Some(&s.hash)), ModuleState::Installed);
        let mut names: Vec<String> = std::fs::read_dir(&s.rb).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["rekordbox.exe".to_string(), "version.dll".to_string()]);
    }

    #[test]
    fn install_refuses_a_foreign_source() {
        let s = Sandbox::new();
        std::fs::write(&s.src, b"tampered").unwrap();
        assert_eq!(install(&s.src, &s.rb, &s.hash, &GOOD), Err(InstallError::SourceNotBundled));
        assert!(!s.rb.join(MODULE_FILE).exists());
    }

    #[test]
    fn install_refuses_a_wrong_rekordbox() {
        let s = Sandbox::new();
        for v in [Some((6, 8, 4, 0)), Some((7, 0, 0, 0)), None] {
            assert_eq!(install(&s.src, &s.rb, &s.hash, &Fake { version: v, running: false }), Err(InstallError::NotRekordbox685));
        }
        std::fs::remove_file(s.rb.join(REKORDBOX_EXE)).unwrap();
        assert_eq!(install(&s.src, &s.rb, &s.hash, &GOOD), Err(InstallError::NotRekordbox685));
        assert!(!s.rb.join(MODULE_FILE).exists());
    }

    #[test]
    fn real_version_check_rejects_a_non_rekordbox_exe() {
        let s = Sandbox::new();
        let notepad = Path::new(r"C:\Windows\System32\notepad.exe");
        if notepad.is_file() {
            std::fs::copy(notepad, s.rb.join(REKORDBOX_EXE)).unwrap();
            let r = install(&s.src, &s.rb, &s.hash, &RealHost);
            assert_eq!(r, Err(InstallError::NotRekordbox685));
            assert!(!s.rb.join(MODULE_FILE).exists());
        }
    }

    #[test]
    fn install_refuses_while_rekordbox_runs() {
        let s = Sandbox::new();
        let r = install(&s.src, &s.rb, &s.hash, &Fake { version: Some((6, 8, 5, 0)), running: true });
        assert_eq!(r, Err(InstallError::RekordboxRunning));
        assert!(r.unwrap_err().message().contains("Close rekordbox first"));
        assert!(!s.rb.join(MODULE_FILE).exists());
    }

    #[test]
    fn install_replaces_an_outdated_module() {
        let s = Sandbox::new();
        std::fs::write(s.rb.join(MODULE_FILE), b"old").unwrap();
        install(&s.src, &s.rb, &s.hash, &GOOD).unwrap();
        assert_eq!(module_state(Some(&s.rb), Some(&s.hash)), ModuleState::Installed);
    }

    #[test]
    fn remove_deletes_only_our_module() {
        let s = Sandbox::new();
        // nothing there: fine
        remove(&s.rb, &s.hash, &GOOD).unwrap();
        // somebody else's version.dll stays
        std::fs::write(s.rb.join(MODULE_FILE), b"foreign").unwrap();
        assert_eq!(remove(&s.rb, &s.hash, &GOOD), Err(InstallError::NotOurModule));
        assert!(s.rb.join(MODULE_FILE).exists());
        // ours goes, rekordbox.exe stays
        std::fs::write(s.rb.join(MODULE_FILE), b"module bytes").unwrap();
        assert_eq!(remove(&s.rb, &s.hash, &Fake { version: Some((6, 8, 5, 0)), running: true }), Err(InstallError::RekordboxRunning));
        remove(&s.rb, &s.hash, &GOOD).unwrap();
        assert!(!s.rb.join(MODULE_FILE).exists());
        assert!(s.rb.join(REKORDBOX_EXE).exists());
    }
}
