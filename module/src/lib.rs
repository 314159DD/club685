//! rbmod: in-process mod runtime for rekordbox 6.8.5.
//!
//! Loaded by rekordbox.exe as `version.dll` from its install folder. All real exports are
//! forwarded to System32 (see build.rs). On attach it starts a worker thread, which checks
//! that the host really is rekordbox.exe 6.8.5 before installing anything.

mod deck;
mod diag;
mod guard;
mod log;

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{BOOL, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Threading::CreateThread;

#[no_mangle]
pub extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(module);
            CreateThread(std::ptr::null(), 0, Some(init), std::ptr::null(), 0, std::ptr::null_mut());
        }
    }
    TRUE
}

unsafe extern "system" fn init(_: *mut c_void) -> u32 {
    log::line("rbmod loaded");
    match guard::check_host() {
        Ok(base) => {
            log::line(&format!("host is rekordbox 6.8.5, image base {base:#x}"));
            if std::env::var("RBMOD_SAFE").map(|v| v == "1").unwrap_or(false) {
                log::line("safe mode (RBMOD_SAFE=1): no engine reads, idling");
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            }
            deck::run(base);
        }
        Err(why) => {
            log::line(&format!("host check failed, staying inactive: {why}"));
            return 0;
        }
    }
    0
}
