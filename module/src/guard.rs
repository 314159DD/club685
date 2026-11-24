//! Refuses to run anywhere except the exact rekordbox.exe build the addresses were mapped on.

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// PE header TimeDateStamp and SizeOfImage of rekordbox.exe 6.8.5 (sha256 31898b14...0f0).
pub const EXPECTED_TIMESTAMP: u32 = 0x660b_b684;
pub const EXPECTED_SIZE_OF_IMAGE: u32 = 0x046b_4000;
pub const PREFERRED_BASE: usize = 0x1_4000_0000;

pub fn check_host() -> Result<usize, String> {
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        if base == 0 {
            return Err("no main module".into());
        }
        let e_lfanew = *((base + 0x3c) as *const u32) as usize;
        let nt = base + e_lfanew;
        if *(nt as *const u32) != 0x4550 {
            return Err("main module is not PE".into());
        }
        let ts = *((nt + 8) as *const u32);
        let size = *((nt + 24 + 56) as *const u32);
        if ts != EXPECTED_TIMESTAMP || size != EXPECTED_SIZE_OF_IMAGE {
            return Err(format!("unexpected build: timestamp {ts:#x}, size_of_image {size:#x}"));
        }
        Ok(base)
    }
}

/// Converts a Ghidra address (preferred base 0x140000000) into a live address.
pub fn rva(base: usize, ghidra_va: usize) -> usize {
    base + (ghidra_va - PREFERRED_BASE)
}
