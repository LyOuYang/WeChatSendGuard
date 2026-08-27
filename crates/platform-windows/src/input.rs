use std::mem::size_of;
use uuid::Uuid;
use wechat_send_guard_platform_api::{InputInjector, PlatformError, PlatformResult};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput,
    VK_RETURN,
};

/// Native input injector. Constructing it is inert; only `send_enter` reaches `SendInput`.
#[derive(Debug, Clone)]
pub struct WindowsInputInjector {
    marker: usize,
}

impl WindowsInputInjector {
    pub fn new(marker: usize) -> Self {
        Self { marker }
    }

    pub fn with_random_marker() -> Self {
        Self::new(Uuid::new_v4().as_u128() as usize)
    }

    pub fn marker(&self) -> usize {
        self.marker
    }

    pub fn native_input_size_is_valid() -> bool {
        size_of::<INPUT>() == expected_input_size()
    }
}

impl InputInjector for WindowsInputInjector {
    fn send_enter(&self, is_numpad_enter: bool) -> PlatformResult<()> {
        let input_size = size_of::<INPUT>();
        if input_size != expected_input_size() {
            return Err(PlatformError::new(
                "input-layout-invalid",
                format!(
                    "INPUT is {input_size} bytes; expected {}",
                    expected_input_size()
                ),
            ));
        }

        let extended_flag = if is_numpad_enter {
            KEYEVENTF_EXTENDEDKEY
        } else {
            Default::default()
        };
        let inputs = [
            keyboard_input(extended_flag, self.marker),
            keyboard_input(extended_flag | KEYEVENTF_KEYUP, self.marker),
        ];

        // SAFETY: `inputs` is a valid contiguous array for the duration of this call and the
        // checked byte size matches the Win32 INPUT ABI for the active pointer width.
        let sent = unsafe { SendInput(&inputs, input_size as i32) };
        if sent != inputs.len() as u32 {
            return Err(PlatformError::new(
                "send-input-failed",
                format!(
                    "SendInput accepted {sent} of {} keyboard events",
                    inputs.len()
                ),
            ));
        }
        Ok(())
    }
}

fn expected_input_size() -> usize {
    if size_of::<usize>() == 8 { 40 } else { 28 }
}

fn keyboard_input(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
    marker: usize,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_RETURN,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: marker,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsInputInjector;

    #[test]
    fn input_layout_matches_the_active_windows_abi_without_injecting() {
        assert!(WindowsInputInjector::native_input_size_is_valid());
    }

    #[test]
    fn marker_is_retained_without_calling_send_input() {
        assert_eq!(WindowsInputInjector::new(42).marker(), 42);
    }
}
