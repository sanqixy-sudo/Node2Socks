#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Keep one desktop process per user for repeated clicks on the executable.
    #[cfg(windows)]
    if !acquire_single_instance() {
        return;
    }
    node2socks_desktop::run();
}

#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = std::ffi::OsStr::new("Node2Socks.SingleInstance")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: the UTF-16 name is NUL terminated and valid for this call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return false;
    }
    // Keep the mutex handle open until process exit.
    let _ = handle;
    true
}
