use std::ffi::{c_char, c_void};

pub const PATH_CAPACITY: usize = 4096;
pub const TITLE_CAPACITY: usize = 512;
pub const TEXT_CAPACITY: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MacContextSnapshot {
    pub context_change_generation: u64,
    pub window_id: i64,
    pub process_id: u32,
    pub is_trusted_weixin: bool,
    pub accessibility_available: bool,
    pub compatibility_available: bool,
    pub message_editor_focused: bool,
    pub is_group_chat: bool,
    pub is_contact_chat: bool,
    pub send_button_available: bool,
    pub send_button_enabled: bool,
    pub window_x: f64,
    pub window_y: f64,
    pub window_width: f64,
    pub window_height: f64,
    pub send_button_x: f64,
    pub send_button_y: f64,
    pub send_button_width: f64,
    pub send_button_height: f64,
    pub process_path: [c_char; PATH_CAPACITY],
    pub chat_title: [c_char; TITLE_CAPACITY],
}

impl Default for MacContextSnapshot {
    fn default() -> Self {
        // SAFETY: this C-compatible snapshot is entirely integers, floating-point values, bools,
        // and byte arrays. An all-zero value is the documented inactive snapshot.
        unsafe { std::mem::zeroed() }
    }
}

pub type KeyboardCallback = unsafe extern "C" fn(u16, bool, bool, *mut c_void) -> bool;
pub type MouseCallback = unsafe extern "C" fn(i32, i32, *mut c_void) -> bool;

unsafe extern "C" {
    pub fn WSGMacRequestAccessibilityAccess() -> bool;
    pub fn WSGMacContextChangeGeneration() -> u64;
    pub fn WSGMacFrontmostIsWeChat() -> bool;
    pub fn WSGMacCopyForegroundContext(
        observe_send_button: bool,
        snapshot: *mut MacContextSnapshot,
    ) -> bool;
    pub fn WSGMacRestoreEditorFocusAndCopyContext(
        expected_window_id: i64,
        expected_process_id: u32,
        snapshot: *mut MacContextSnapshot,
    ) -> bool;
    pub fn WSGMacCopyDraftPreview(
        expected_window_id: i64,
        expected_process_id: u32,
        output: *mut c_char,
        output_capacity: usize,
    ) -> bool;

    pub fn WSGMacStartKeyboardTap(
        marker: u64,
        callback: KeyboardCallback,
        context: *mut c_void,
    ) -> *mut c_void;
    pub fn WSGMacStartMouseTap(
        marker: u64,
        callback: MouseCallback,
        context: *mut c_void,
    ) -> *mut c_void;
    pub fn WSGMacStopInputTap(handle: *mut c_void);
    pub fn WSGMacPostEnter(key_code: u16, marker: u64) -> bool;

    pub fn WSGMacCopyCursorPosition(x: *mut i32, y: *mut i32) -> bool;
    pub fn WSGMacActivateWindow(native_view: i64);
    pub fn WSGMacShowErrorDialog(message: *const c_char);
    pub fn WSGMacSelectOpenJSON(output: *mut c_char, output_capacity: usize) -> bool;
    pub fn WSGMacSelectSavePath(
        default_name: *const c_char,
        allowed_extension: *const c_char,
        output: *mut c_char,
        output_capacity: usize,
    ) -> bool;
    pub fn WSGMacCopyOperatingSystemVersion(output: *mut c_char, output_capacity: usize) -> bool;
    pub fn WSGMacCopyInstalledWeChatVersion(output: *mut c_char, output_capacity: usize) -> bool;
    pub fn WSGMacCopyLocalDate(year: *mut u16, month: *mut u16, day: *mut u16) -> bool;
}
