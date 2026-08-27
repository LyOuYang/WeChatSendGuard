use std::path::PathBuf;
use wechat_send_guard_platform_api::{PlatformError, PlatformResult, StartupRegistration};
use windows::{
    Win32::{
        Foundation::ERROR_FILE_NOT_FOUND,
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, REG_SZ, RegCloseKey, RegCreateKeyW, RegDeleteValueW,
            RegSetValueExW,
        },
    },
    core::PCWSTR,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "WeChatSendGuard";

/// Current-user Windows startup registration. It writes only this application's named value in
/// HKCU and never requests elevation.
#[derive(Debug, Clone)]
pub struct WindowsStartupRegistration {
    executable_path: PathBuf,
}

impl WindowsStartupRegistration {
    pub fn new(executable_path: impl Into<PathBuf>) -> Self {
        Self {
            executable_path: executable_path.into(),
        }
    }

    pub fn for_current_executable() -> PlatformResult<Self> {
        std::env::current_exe()
            .map(Self::new)
            .map_err(|error| PlatformError::new("startup-executable-path", error.to_string()))
    }

    pub fn launch_command(&self) -> String {
        format!("\"{}\" --background", self.executable_path.display())
    }
}

impl StartupRegistration for WindowsStartupRegistration {
    fn apply(&self, enabled: bool) -> PlatformResult<bool> {
        let key = create_run_key()?;
        let result = if enabled {
            set_run_value(key, &self.launch_command())
        } else {
            delete_run_value(key)
        };
        close_key(key, result)?;
        Ok(enabled)
    }
}

fn create_run_key() -> PlatformResult<HKEY> {
    let mut key = HKEY::default();
    let run_key = wide(RUN_KEY);
    // SAFETY: the key path is a NUL-terminated local UTF-16 buffer, and `key` is a valid out
    // pointer for the synchronous registry call.
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(run_key.as_ptr()), &mut key) };
    status_to_result(status, "startup-registry-open")?;
    Ok(key)
}

fn set_run_value(key: HKEY, command: &str) -> PlatformResult<()> {
    let name = wide(VALUE_NAME);
    let command = wide(command);
    let bytes = command
        .iter()
        .flat_map(|character| character.to_le_bytes())
        .collect::<Vec<_>>();
    // SAFETY: both buffers are live and NUL-terminated for the duration of the synchronous call.
    let status = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            REG_SZ,
            Some(bytes.as_slice()),
        )
    };
    status_to_result(status, "startup-registry-write")
}

fn delete_run_value(key: HKEY) -> PlatformResult<()> {
    let name = wide(VALUE_NAME);
    // SAFETY: `name` is a NUL-terminated local UTF-16 buffer for this synchronous call.
    let status = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    status_to_result(status, "startup-registry-delete")
}

fn close_key(key: HKEY, operation: PlatformResult<()>) -> PlatformResult<()> {
    // SAFETY: `key` was returned by RegCreateKeyW and is closed exactly once by this function.
    let close_status = unsafe { RegCloseKey(key) };
    operation?;
    status_to_result(close_status, "startup-registry-close")
}

fn status_to_result(
    status: windows::Win32::Foundation::WIN32_ERROR,
    code: &'static str,
) -> PlatformResult<()> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(PlatformError::new(
            code,
            format!("Windows error {}", status.0),
        ))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::WindowsStartupRegistration;

    #[test]
    fn startup_command_quotes_paths_with_spaces_without_touching_the_registry() {
        let registration =
            WindowsStartupRegistration::new(r"C:\Program Files\WeChatSendGuard\guard.exe");
        assert_eq!(
            registration.launch_command(),
            r#""C:\Program Files\WeChatSendGuard\guard.exe" --background"#
        );
    }
}
