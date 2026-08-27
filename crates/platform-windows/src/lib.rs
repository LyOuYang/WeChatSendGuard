//! Windows-only primitives for WeChatSendGuard.
//!
//! These modules never run automatically. Callers must explicitly start the input gate or
//! request injection only after `guard-core` has accepted a freshly revalidated target.

#![cfg_attr(windows, deny(unsafe_op_in_unsafe_fn))]

#[cfg(windows)]
pub mod audit;
#[cfg(windows)]
pub mod context;
#[cfg(windows)]
pub mod file_dialog;
#[cfg(windows)]
pub mod hook;
#[cfg(windows)]
pub mod input;
#[cfg(windows)]
pub mod startup;
#[cfg(windows)]
pub mod trust;
#[cfg(windows)]
pub mod window_placement;

#[cfg(windows)]
pub use audit::{WindowsAuditLog, default_audit_log_directory};
#[cfg(windows)]
pub use context::{SendButtonDiagnostic, WindowsContextMonitor, WindowsContextProvider};
#[cfg(windows)]
pub use file_dialog::{select_protected_chat_export, select_protected_chat_import};
#[cfg(windows)]
pub use hook::{KeyboardKey, KeyboardStroke, MouseClick, WindowsKeyboardHook, WindowsMouseHook};
#[cfg(windows)]
pub use input::WindowsInputInjector;
#[cfg(windows)]
pub use startup::WindowsStartupRegistration;
#[cfg(windows)]
pub use trust::{
    ProcessTrust, TRUSTED_WEIXIN_PATH, assess_window_trust, is_valid_weixin_executable_path,
    path_matches_trusted_weixin,
};
pub use window_placement::{
    activate_window, center_popup_over_window, cursor_screen_position, enable_high_dpi_awareness,
};
