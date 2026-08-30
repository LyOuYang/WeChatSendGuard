//! macOS platform primitives for WeChatSendGuard.
//!
//! The adapter uses only public AppKit, Accessibility, CoreGraphics, and Security APIs. It never
//! starts WeChat automatically, reads its private data, captures the screen, or injects code.

#![cfg_attr(target_os = "macos", deny(unsafe_op_in_unsafe_fn))]

#[cfg(target_os = "macos")]
pub mod audit;
#[cfg(target_os = "macos")]
pub mod context;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
pub mod file_dialog;
#[cfg(target_os = "macos")]
pub mod hook;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod startup;
#[cfg(target_os = "macos")]
pub mod trust;
#[cfg(target_os = "macos")]
pub mod window_placement;

#[cfg(target_os = "macos")]
pub use audit::{MacAuditLog, default_audit_log_directory};
#[cfg(target_os = "macos")]
pub use context::{MacContextMonitor, MacContextProvider, MacSendButtonDiagnostic};
#[cfg(target_os = "macos")]
pub use file_dialog::{
    select_diagnostic_export, select_protected_chat_export, select_protected_chat_import,
};
#[cfg(target_os = "macos")]
pub use hook::{KeyboardKey, KeyboardStroke, MacKeyboardHook, MacMouseHook, MouseClick};
#[cfg(target_os = "macos")]
pub use input::MacInputInjector;
#[cfg(target_os = "macos")]
pub use startup::MacStartupRegistration;
#[cfg(target_os = "macos")]
pub use trust::{
    TRUSTED_WECHAT_BUNDLE_ID, TRUSTED_WECHAT_TEAM_ID, installed_wechat_version,
    trusted_wechat_identity,
};
#[cfg(target_os = "macos")]
pub use window_placement::{
    activate_window, center_popup_over_window, cursor_screen_position, operating_system_version,
    show_error_dialog,
};
