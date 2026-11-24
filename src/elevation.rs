//! Detecting admin rights and relaunching elevated (Windows only; no-ops elsewhere).

use std::ffi::OsString;

#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    true
}

/// Starts a new elevated copy of this exe with `args` (shows the UAC prompt).
/// Returns false if the user declined UAC or the launch failed.
#[cfg(windows)]
pub fn relaunch_as_admin(args: &[OsString]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let params = args
        .iter()
        .map(|a| quote_arg(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");

    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let params = wide(params.as_ref());

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Per the docs, values > 32 mean success.
    result as isize > 32
}

#[cfg(not(windows))]
pub fn relaunch_as_admin(_args: &[OsString]) -> bool {
    false
}

/// Quotes one argument for the Windows command-line parser. A trailing
/// backslash (e.g. `C:\`) must be doubled or it would escape the closing quote.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote_arg(arg: &str) -> String {
    let trailing = arg.len() - arg.trim_end_matches('\\').len();
    format!("\"{}{}\"", arg, "\\".repeat(trailing))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_trailing_backslash() {
        assert_eq!(super::quote_arg(r"C:\"), r#""C:\\""#);
        assert_eq!(super::quote_arg(r"C:\Program Files"), r#""C:\Program Files""#);
    }
}
