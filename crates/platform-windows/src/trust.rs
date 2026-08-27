use std::path::Path;
use wechat_send_guard_platform_api::PlatformError;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    },
    core::PWSTR,
};

pub const TRUSTED_WEIXIN_PATH: &str = r"C:\Program Files\Tencent\Weixin\Weixin.exe";

/// A user-selected target must remain an absolute `Weixin.exe` path. Existence is not
/// checked here: a missing path is safe because it cannot match a running process.
pub fn is_valid_weixin_executable_path(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    path.is_absolute()
        && path.file_name().is_some_and(|file_name| {
            file_name
                .to_string_lossy()
                .eq_ignore_ascii_case("Weixin.exe")
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTrust {
    pub process_id: u32,
    pub process_path: String,
    pub is_trusted_weixin: bool,
    pub requires_elevation: bool,
}

impl ProcessTrust {
    fn unavailable(process_id: u32, requires_elevation: bool) -> Self {
        Self {
            process_id,
            process_path: String::new(),
            is_trusted_weixin: false,
            requires_elevation,
        }
    }
}

/// Resolves a foreground window's executable through a limited-information handle using the
/// built-in supported Weixin path. An access failure is treated as unavailable/elevated and
/// therefore never trusted.
pub fn assess_window_trust(window_handle: isize) -> Result<ProcessTrust, PlatformError> {
    assess_window_trust_for_executable(window_handle, Path::new(TRUSTED_WEIXIN_PATH))
}

/// Resolves a foreground window's executable using a path selected by the Windows adapter.
/// This stays crate-private so platform configuration cannot be mistaken for a domain rule.
pub(crate) fn assess_window_trust_for_executable(
    window_handle: isize,
    trusted_executable_path: impl AsRef<Path>,
) -> Result<ProcessTrust, PlatformError> {
    if window_handle == 0 {
        return Ok(ProcessTrust::unavailable(0, false));
    }

    let hwnd = HWND(window_handle as _);
    let mut process_id = 0;
    // SAFETY: `process_id` is a valid mutable out pointer and HWND is a value supplied by the
    // caller. The API does not retain either argument.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if process_id == 0 {
        return Ok(ProcessTrust::unavailable(0, false));
    }

    // SAFETY: no handle is inherited and the returned handle is wrapped by an immediate close
    // below on every success path.
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
    {
        Ok(handle) => handle,
        Err(_) => return Ok(ProcessTrust::unavailable(process_id, true)),
    };

    let path = query_process_path(handle)
        .map_err(|error| PlatformError::new("process-path-unavailable", error.to_string()));
    // SAFETY: `handle` was returned by OpenProcess and is closed exactly once here.
    let _ = unsafe { CloseHandle(handle) };

    let process_path = path?;
    Ok(ProcessTrust {
        process_id,
        is_trusted_weixin: path_matches_configured_weixin(&process_path, trusted_executable_path),
        process_path,
        requires_elevation: false,
    })
}

fn query_process_path(handle: windows::Win32::Foundation::HANDLE) -> windows::core::Result<String> {
    let mut capacity = 260usize;
    loop {
        let mut buffer = vec![0u16; capacity];
        let mut length = buffer.len() as u32;
        // SAFETY: `buffer` is writable for `length` UTF-16 code units and the process handle is
        // valid for the duration of the synchronous API call.
        match unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        } {
            Ok(()) if (length as usize) < buffer.len() => {
                return Ok(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            Ok(()) => capacity *= 2,
            Err(error) => return Err(error),
        }
    }
}

/// Windows paths are case-insensitive for this trust check. Failure to match exactly is safe:
/// the caller will not observe or inject into that process.
pub fn path_matches_trusted_weixin(path: impl AsRef<Path>) -> bool {
    path_matches_configured_weixin(path, Path::new(TRUSTED_WEIXIN_PATH))
}

pub(crate) fn path_matches_configured_weixin(
    process_path: impl AsRef<Path>,
    trusted_executable_path: impl AsRef<Path>,
) -> bool {
    normalize_windows_path(process_path.as_ref())
        == normalize_windows_path(trusted_executable_path.as_ref())
}

fn normalize_windows_path(path: &Path) -> String {
    path.as_os_str()
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_weixin_executable_path, path_matches_configured_weixin,
        path_matches_trusted_weixin,
    };

    #[test]
    fn trusted_path_match_is_case_and_separator_insensitive_only() {
        assert!(path_matches_trusted_weixin(
            r"c:/PROGRAM FILES/Tencent/Weixin/Weixin.exe"
        ));
        assert!(!path_matches_trusted_weixin(
            r"C:\Program Files\Tencent\Weixin\other.exe"
        ));
        assert!(!path_matches_trusted_weixin(r"C:\Users\someone\Weixin.exe"));
    }

    #[test]
    fn custom_path_match_remains_exact_after_case_and_separator_normalization() {
        assert!(path_matches_configured_weixin(
            r"d:/Portable/Tencent/Weixin/Weixin.exe",
            r"D:\portable\Tencent\Weixin\Weixin.exe"
        ));
        assert!(!path_matches_configured_weixin(
            r"D:\Portable\Tencent\Weixin\Weixin.exe",
            r"D:\Portable\Tencent\Weixin\other.exe"
        ));
    }

    #[test]
    fn configured_path_must_be_an_absolute_weixin_executable() {
        assert!(is_valid_weixin_executable_path(
            r"D:\Apps\Weixin\Weixin.exe"
        ));
        assert!(!is_valid_weixin_executable_path(r"Weixin.exe"));
        assert!(!is_valid_weixin_executable_path(
            r"D:\Apps\Weixin\other.exe"
        ));
    }
}
