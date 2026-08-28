//! Small Win32 window-placement helpers used by the desktop composition root.
//!
//! They operate only on caller-provided native handles. No helper discovers or inspects a
//! Weixin window on its own, which keeps UI placement separate from context recognition.

use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowRect,
            GetWindowThreadProcessId, IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow,
            ShowWindow,
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Returns the current pointer location in physical screen pixels.
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: Windows writes a POINT into the stack value and retains no pointer after return.
    unsafe { GetCursorPos(&mut point).ok()? };
    Some((point.x, point.y))
}

/// Returns a popup origin that centers a physical-pixel popup over a caller-provided top-level
/// window. Invalid handles and unavailable windows are represented as `None`.
pub fn center_popup_over_window(
    window_handle: isize,
    popup_width: u32,
    popup_height: u32,
) -> Option<(i32, i32)> {
    if window_handle == 0 {
        return None;
    }

    let mut rect = RECT::default();
    // SAFETY: The handle is checked non-zero and RECT is written directly on the stack.
    unsafe { GetWindowRect(HWND(window_handle as _), &mut rect).ok()? };
    center_popup_in_rect(
        ScreenRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
        popup_width,
        popup_height,
    )
}

/// Brings a caller-provided top-level window forward and asks Windows to activate it.
///
/// Windows may refuse a foreground request according to its normal focus-stealing policy. The
/// caller must therefore remain correct even when this function cannot transfer focus.
pub fn activate_window(window_handle: isize) {
    if window_handle == 0 {
        return;
    }

    let window = HWND(window_handle as _);
    // SAFETY: HWND is caller-provided and these APIs neither retain it nor dereference caller
    // memory. Failures are intentionally non-fatal because focus acquisition is best-effort.
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        } else {
            let _ = ShowWindow(window, SW_SHOW);
        }

        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let current_thread = GetCurrentThreadId();

        if foreground_thread != 0 && foreground_thread != current_thread {
            let _ = AttachThreadInput(current_thread, foreground_thread, true);
            let _ = BringWindowToTop(window);
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(Some(window));
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        } else {
            let _ = BringWindowToTop(window);
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(Some(window));
        }
    }
}

fn center_popup_in_rect(
    bounds: ScreenRect,
    popup_width: u32,
    popup_height: u32,
) -> Option<(i32, i32)> {
    if popup_width == 0
        || popup_height == 0
        || bounds.right <= bounds.left
        || bounds.bottom <= bounds.top
    {
        return None;
    }

    let target_width = i64::from(bounds.right) - i64::from(bounds.left);
    let target_height = i64::from(bounds.bottom) - i64::from(bounds.top);
    let x = i64::from(bounds.left) + (target_width - i64::from(popup_width)) / 2;
    let y = i64::from(bounds.top) + (target_height - i64::from(popup_height)) / 2;
    Some((clamp_to_i32(x), clamp_to_i32(y)))
}

fn clamp_to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Configures Per-Monitor V2 high-DPI awareness on Windows 10/11 to avoid DWM bitmap stretching.
pub fn enable_high_dpi_awareness() {
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(test)]
mod tests {
    use super::{ScreenRect, center_popup_in_rect};

    #[test]
    fn centers_a_popup_in_supplied_bounds_without_querying_a_window() {
        let bounds = ScreenRect {
            left: 100,
            top: 200,
            right: 900,
            bottom: 800,
        };

        assert_eq!(center_popup_in_rect(bounds, 400, 200), Some((300, 400)));
    }

    #[test]
    fn rejects_empty_popup_or_invalid_bounds() {
        let bounds = ScreenRect {
            left: 10,
            top: 10,
            right: 10,
            bottom: 100,
        };

        assert_eq!(center_popup_in_rect(bounds, 100, 100), None);
        assert_eq!(
            center_popup_in_rect(
                ScreenRect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                0,
                100,
            ),
            None
        );
    }
}
