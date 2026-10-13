//! Native Windows 11 Quick Settings injection launcher.
//!
//! This module must stay independent of the BE200 USB/HCI transport. It only
//! locates ShellHost.exe and asks the OS XAML Diagnostics runtime to load a
//! Rust TAP COM server into that process.

use crate::error::{AppError, Result};

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [verb] if verb == "inject" => platform::inject(None),
        [verb, flag, path] if verb == "inject" && flag == "--tap" => {
            platform::inject(Some(path))
        }
        _ => Err(AppError::Usage(
            "Usage: be200hub ui quick-settings inject [--tap <be200_qs_tap.dll>]".into(),
        )),
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub fn inject(_: Option<&String>) -> Result<()> {
        Err(AppError::Protocol("Windows Shell integration is Windows-only".into()))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{ffi::{c_void, CStr}, os::windows::ffi::OsStrExt, path::{Path, PathBuf}};

    type Hwnd = *mut c_void;
    type Hmodule = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // Must match the TAP's DllGetClassObject CLSID exactly.
    const TAP_CLSID: Guid = Guid {
        data1: 0xf18c96a3,
        data2: 0x281a,
        data3: 0x4dd0,
        data4: [0x9c, 0x81, 0xb0, 0x20, 0x40, 0xb8, 0x9e, 0x3c],
    };

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> Hmodule;
        fn GetProcAddress(module: Hmodule, name: *const u8) -> *mut c_void;
        fn GetLastError() -> u32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(handle: *mut c_void, flags: u32,
                                      path: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn FreeLibrary(module: Hmodule) -> i32;
    }

    type InitXamlDiagnosticsEx = unsafe extern "system" fn(
        endpoint: *const u16,
        pid: u32,
        xaml_diag_dll: *const u16,
        tap_dll: *const u16,
        clsid: Guid,
        initialization_data: *const u16,
    ) -> i32;

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    fn xaml_error(message: impl Into<String>) -> AppError {
        AppError::Protocol(format!("Quick Settings: {}", message.into()))
    }

    fn shellhost_process_id() -> Result<u32> {
        unsafe {
            let klass = wide(std::ffi::OsStr::new("ControlCenterWindow"));
            let hwnd = FindWindowW(klass.as_ptr(), std::ptr::null());
            if hwnd.is_null() {
                return Err(xaml_error(
                    "ControlCenterWindow not found. Open Win+A and retry."
                ));
            }
            let mut pid = 0;
            if GetWindowThreadProcessId(hwnd, &mut pid) == 0 || pid == 0 {
                return Err(xaml_error("couldn't get ControlCenterWindow PID"));
            }

            // Refuse to target a foreign process even if it creates an HWND
            // with the same class name.
            let process = OpenProcess(0x1000, 0, pid); // QUERY_LIMITED_INFORMATION
            if process.is_null() {
                return Err(xaml_error(format!(
                    "OpenProcess for PID {pid} failed: {}", GetLastError()
                )));
            }
            let mut path = [0u16; 32768];
            let mut len = path.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len);
            CloseHandle(process);
            if ok == 0 {
                return Err(xaml_error(format!("couldn't identify PID {pid}")));
            }
            let actual = String::from_utf16_lossy(&path[..len as usize]);
            if !actual.rsplit(['\\', '/']).next().unwrap_or_default()
                .eq_ignore_ascii_case("ShellHost.exe")
            {
                return Err(xaml_error(format!(
                    "refusing to inject into unexpected process: {actual}"
                )));
            }
            Ok(pid)
        }
    }

    fn tap_path(override_path: Option<&String>) -> Result<PathBuf> {
        let path = match override_path {
            Some(path) => Path::new(path).to_path_buf(),
            None => std::env::current_exe()
                .map_err(|e| xaml_error(format!("current_exe: {e}")))?
                .with_file_name("be200_qs_tap.dll"),
        };
        let path = path.canonicalize().map_err(|e| xaml_error(format!(
            "TAP DLL missing: {} ({e}); first build it using cargo build --manifest-path integrations/windows-quick-settings/Cargo.toml",
            path.display()
        )))?;
        if !path.extension().map_or(false, |e| e.eq_ignore_ascii_case("dll")) {
            return Err(xaml_error("TAP must be a DLL"));
        }
        Ok(path)
    }

    pub fn inject(tap: Option<&String>) -> Result<()> {
        let pid = shellhost_process_id()?;
        let path = tap_path(tap)?;
        println!("[QS] Target: ShellHost.exe PID={pid}");
        println!("[QS] Rust TAP: {}", path.display());
        unsafe {
            let module = LoadLibraryW(wide(std::ffi::OsStr::new("Windows.UI.Xaml.dll")).as_ptr());
            if module.is_null() {
                return Err(xaml_error(format!(
                    "LoadLibrary(Windows.UI.Xaml.dll) failed: {}", GetLastError()
                )));
            }
            let proc = GetProcAddress(module,
                CStr::from_bytes_with_nul_unchecked(b"InitializeXamlDiagnosticsEx\0").as_ptr()
                    as *const u8);
            if proc.is_null() {
                FreeLibrary(module);
                return Err(xaml_error("InitializeXamlDiagnosticsEx export not found"));
            }
            let init: InitXamlDiagnosticsEx = std::mem::transmute(proc);
            let xaml_name = wide(std::ffi::OsStr::new(""));
            let tap_name = wide(path.as_os_str());
            let mut last_hr = 0x80070490u32; // HRESULT_FROM_WIN32(ERROR_NOT_FOUND)
            for i in 1..=10000u32 {
                let endpoint = wide(std::ffi::OsStr::new(&format!("VisualDiagConnection{i}")));
                let hr = init(endpoint.as_ptr(), pid, xaml_name.as_ptr(),
                              tap_name.as_ptr(), TAP_CLSID, std::ptr::null());
                if hr as u32 == 0x80070490 {
                    continue;
                }
                last_hr = hr as u32;
                if hr >= 0 {
                    FreeLibrary(module);
                    println!("[QS] XAML Diagnostics TAP request accepted on endpoint {i} (HRESULT=0x{hr:08X}).");
                    println!("[QS] This is NOT proof of rendered tile. Inspect ShellHost debug output for [BE200-QS/TAP].");
                    return Ok(());
                }
                break;
            }
            FreeLibrary(module);
            Err(xaml_error(format!(
                "InitializeXamlDiagnosticsEx failed: HRESULT=0x{last_hr:08X}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_path_is_separate_from_hci() {
        let error = super::run(&["nonsense".into()]).unwrap_err();
        assert!(error.to_string().contains("quick-settings inject"));
    }
}