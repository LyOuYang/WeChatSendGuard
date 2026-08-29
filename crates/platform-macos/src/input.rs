use uuid::Uuid;
use wechat_send_guard_platform_api::{InputInjector, PlatformError, PlatformResult};

use crate::ffi;

const RETURN_KEY_CODE: u16 = 36;
const NUMPAD_ENTER_KEY_CODE: u16 = 76;

#[derive(Debug, Clone)]
pub struct MacInputInjector {
    marker: usize,
}

impl MacInputInjector {
    pub fn new(marker: usize) -> Self {
        Self { marker }
    }

    pub fn with_random_marker() -> Self {
        Self::new(Uuid::new_v4().as_u128() as usize)
    }

    pub fn marker(&self) -> usize {
        self.marker
    }
}

impl InputInjector for MacInputInjector {
    fn send_enter(&self, is_numpad_enter: bool) -> PlatformResult<()> {
        let key_code = if is_numpad_enter {
            NUMPAD_ENTER_KEY_CODE
        } else {
            RETURN_KEY_CODE
        };
        // SAFETY: the bridge creates and posts exactly one marked key-down/key-up pair and retains
        // no Rust data.
        if unsafe { ffi::WSGMacPostEnter(key_code, self.marker as u64) } {
            Ok(())
        } else {
            Err(PlatformError::new(
                "quartz-post-event-failed",
                "macOS refused synthetic input. Accessibility input-posting permission is required.",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MacInputInjector;

    #[test]
    fn marker_is_retained_without_posting_input() {
        assert_eq!(MacInputInjector::new(42).marker(), 42);
    }
}
