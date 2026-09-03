use std::ffi::{CStr, CString};

use crate::{context::cached_window_bounds, ffi};

pub fn cursor_screen_position() -> Option<(i32, i32)> {
    let mut x = 0;
    let mut y = 0;
    // SAFETY: both out pointers are valid for the synchronous CoreGraphics call.
    unsafe { ffi::WSGMacCopyCursorPosition(&mut x, &mut y) }.then_some((x, y))
}

pub fn center_popup_over_window(
    window_handle: isize,
    popup_width: u32,
    popup_height: u32,
) -> Option<(i32, i32)> {
    let bounds = cached_window_bounds(window_handle)?;
    if popup_width == 0 || popup_height == 0 {
        return None;
    }
    let x = bounds.left + ((bounds.right - bounds.left) - f64::from(popup_width)) / 2.0;
    let y = bounds.top + ((bounds.bottom - bounds.top) - f64::from(popup_height)) / 2.0;
    Some((clamp_to_i32(x), clamp_to_i32(y)))
}

pub fn activate_window(native_view: isize) {
    // SAFETY: Slint owns the NSView for the duration of this best-effort activation call. The
    // bridge does not retain it beyond the main-queue operation.
    unsafe { ffi::WSGMacActivateWindow(native_view as i64) };
}

pub fn apply_popup_window_decorations(_native_view: isize) {}

pub fn apply_main_window_decorations(_native_view: isize) {}

pub fn show_error_dialog(message: &str) {
    let sanitized = message.replace('\0', " ");
    let message = CString::new(sanitized).expect("NUL bytes were removed");
    // SAFETY: the bridge reads the NUL-terminated string synchronously.
    unsafe { ffi::WSGMacShowErrorDialog(message.as_ptr()) };
}

pub fn operating_system_version() -> String {
    let mut output = [0i8; ffi::TEXT_CAPACITY];
    // SAFETY: the bridge receives a valid bounded output buffer and retains nothing.
    if unsafe { ffi::WSGMacCopyOperatingSystemVersion(output.as_mut_ptr(), output.len()) } {
        // SAFETY: successful output is NUL-terminated within the fixed-size array.
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    } else {
        "macOS（版本未能读取）".to_owned()
    }
}

fn clamp_to_i32(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::clamp_to_i32;

    #[test]
    fn popup_coordinates_are_clamped_to_slint_physical_positions() {
        assert_eq!(clamp_to_i32(42.4), 42);
        assert_eq!(clamp_to_i32(f64::MAX), i32::MAX);
    }
}
