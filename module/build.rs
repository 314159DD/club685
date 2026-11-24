// Forward every export of the real version.dll by absolute path, so rekordbox keeps working.
const EXPORTS: &[&str] = &[
    "GetFileVersionInfoA", "GetFileVersionInfoExA", "GetFileVersionInfoExW",
    "GetFileVersionInfoSizeA", "GetFileVersionInfoSizeExA", "GetFileVersionInfoSizeExW", "GetFileVersionInfoSizeW",
    "GetFileVersionInfoW", "VerFindFileA", "VerFindFileW", "VerInstallFileA", "VerInstallFileW",
    "VerLanguageNameA", "VerLanguageNameW", "VerQueryValueA", "VerQueryValueW",
];

fn main() {
    cc::Build::new().file("csrc/seh.c").compile("rbmod_seh");
    println!("cargo:rerun-if-changed=csrc/seh.c");
    for e in EXPORTS {
        println!(r"cargo:rustc-cdylib-link-arg=/EXPORT:{e}=C:\Windows\System32\version.{e}");
    }
}
