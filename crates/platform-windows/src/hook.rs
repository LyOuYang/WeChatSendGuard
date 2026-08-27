use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    },
};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::{
            Ime::{GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext},
            KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_RETURN, VK_SHIFT},
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
            LLKHF_INJECTED, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
            WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

/// The guarded physical key seen by the low-level hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardKey {
    Enter,
    Escape,
}

/// Snapshot passed from the low-level hook to application orchestration. It contains no text,
/// UI Automation element, or process handle and can be handled without touching Weixin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardStroke {
    pub key: KeyboardKey,
    pub is_numpad_enter: bool,
    pub is_injected: bool,
    pub shift_pressed: bool,
    pub ime_composing: bool,
    pub foreground_window: isize,
}

type KeyDownHandler = dyn Fn(KeyboardStroke) -> bool + Send + Sync + 'static;

#[derive(Default)]
struct HookState {
    marker: usize,
    handler: RwLock<Option<Arc<KeyDownHandler>>>,
    suppress_physical_key_up: AtomicBool,
    suppressed_key_code: AtomicU32,
    suppressed_window: AtomicIsize,
}

/// Installs at most one process-wide `WH_KEYBOARD_LL` hook. Construction and handler assignment
/// are inert; `start` must be explicitly called by the desktop application.
pub struct WindowsKeyboardHook {
    state: Arc<HookState>,
    native_hook: isize,
}

impl WindowsKeyboardHook {
    pub fn new(marker: usize) -> Self {
        Self {
            state: Arc::new(HookState {
                marker,
                ..HookState::default()
            }),
            native_hook: 0,
        }
    }

    pub fn set_key_down_handler(&self, handler: Arc<KeyDownHandler>) {
        *write_unpoisoned(&self.state.handler) = Some(handler);
    }

    pub fn clear_key_down_handler(&self) {
        *write_unpoisoned(&self.state.handler) = None;
    }

    pub fn is_started(&self) -> bool {
        self.native_hook != 0
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.is_started() {
            return Ok(());
        }

        let active = active_hook();
        {
            let mut slot = lock_unpoisoned(active);
            if slot.is_some() {
                return Err(PlatformError::new(
                    "keyboard-hook-already-active",
                    "Only one low-level keyboard hook may be active in this process.",
                ));
            }
            *slot = Some(Arc::clone(&self.state));
        }

        // SAFETY: the callback is a process-lifetime function pointer and the active state is
        // retained in a global slot before installation. No module handle is required for a
        // thread-independent low-level hook in this process.
        let hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), None, 0) };
        match hook {
            Ok(hook) => {
                self.native_hook = hook.0 as isize;
                Ok(())
            }
            Err(error) => {
                *lock_unpoisoned(active) = None;
                Err(PlatformError::new(
                    "keyboard-hook-install-failed",
                    error.to_string(),
                ))
            }
        }
    }

    pub fn stop(&mut self) {
        if self.native_hook != 0 {
            // SAFETY: this value was returned by SetWindowsHookExW and is unhooked at most once.
            let _ = unsafe { UnhookWindowsHookEx(HHOOK(self.native_hook as _)) };
            self.native_hook = 0;
        }

        let mut active = lock_unpoisoned(active_hook());
        if active
            .as_ref()
            .is_some_and(|state| Arc::ptr_eq(state, &self.state))
        {
            *active = None;
        }
        self.state
            .suppress_physical_key_up
            .store(false, Ordering::Release);
        self.state.suppressed_key_code.store(0, Ordering::Release);
        self.state.suppressed_window.store(0, Ordering::Release);
    }
}

impl Drop for WindowsKeyboardHook {
    fn drop(&mut self) {
        self.stop();
    }
}

static ACTIVE_HOOK: OnceLock<Mutex<Option<Arc<HookState>>>> = OnceLock::new();

fn active_hook() -> &'static Mutex<Option<Arc<HookState>>> {
    ACTIVE_HOOK.get_or_init(|| Mutex::new(None))
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        let state = lock_unpoisoned(active_hook()).clone();
        if let Some(state) = state {
            // SAFETY: Windows invokes a low-level hook with LPARAM pointing at a valid
            // KBDLLHOOKSTRUCT for non-negative callback codes. We copy it immediately.
            let data = unsafe { (lparam.0 as *const KBDLLHOOKSTRUCT).read_unaligned() };
            if let Some(key) = keyboard_key_from_virtual_key(data.vkCode) {
                let message = wparam.0 as u32;
                if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN)
                    && handle_key_down(&state, data, key)
                {
                    return LRESULT(1);
                }
                if matches!(message, WM_KEYUP | WM_SYSKEYUP) && handle_key_up(&state, data) {
                    return LRESULT(1);
                }
            }
        }
    }

    // SAFETY: forwarding preserves the original callback arguments unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn handle_key_down(state: &HookState, data: KBDLLHOOKSTRUCT, key: KeyboardKey) -> bool {
    let is_injected = data.flags.contains(LLKHF_INJECTED) || data.dwExtraInfo == state.marker;
    if is_injected {
        return false;
    }

    // SAFETY: these APIs read transient system keyboard/foreground state and retain no data.
    let foreground_window = unsafe { GetForegroundWindow().0 as isize };
    let shift_pressed = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) } < 0;
    let stroke = KeyboardStroke {
        key,
        is_numpad_enter: key == KeyboardKey::Enter && data.flags.contains(LLKHF_EXTENDED),
        is_injected,
        shift_pressed,
        ime_composing: key == KeyboardKey::Enter && is_ime_composing(foreground_window),
        foreground_window,
    };

    let handler = read_unpoisoned(&state.handler).clone();
    let handled = handler
        .map(|handler| catch_unwind(AssertUnwindSafe(|| handler(stroke))).unwrap_or(false))
        .unwrap_or(false);
    if handled {
        state
            .suppressed_window
            .store(foreground_window, Ordering::Release);
        state
            .suppressed_key_code
            .store(data.vkCode, Ordering::Release);
        state
            .suppress_physical_key_up
            .store(true, Ordering::Release);
    }
    handled
}

fn handle_key_up(state: &HookState, data: KBDLLHOOKSTRUCT) -> bool {
    let is_injected = data.flags.contains(LLKHF_INJECTED) || data.dwExtraInfo == state.marker;
    if is_injected
        || data.vkCode != state.suppressed_key_code.load(Ordering::Acquire)
        || !state.suppress_physical_key_up.swap(false, Ordering::AcqRel)
    {
        return false;
    }

    state.suppressed_key_code.store(0, Ordering::Release);
    let suppressed_window = state.suppressed_window.swap(0, Ordering::AcqRel);
    // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
    unsafe { GetForegroundWindow().0 as isize == suppressed_window }
}

fn keyboard_key_from_virtual_key(virtual_key: u32) -> Option<KeyboardKey> {
    match virtual_key {
        code if code == u32::from(VK_RETURN.0) => Some(KeyboardKey::Enter),
        code if code == u32::from(VK_ESCAPE.0) => Some(KeyboardKey::Escape),
        _ => None,
    }
}

fn is_ime_composing(window_handle: isize) -> bool {
    if window_handle == 0 {
        return false;
    }

    // SAFETY: HWND is a copied native value. IMM returns a temporary context that we release.
    let input_context = unsafe { ImmGetContext(HWND(window_handle as _)) };
    if input_context.0.is_null() {
        return false;
    }
    // SAFETY: the input context is valid until the matching release below; passing no buffer asks
    // only for the byte count of the in-progress composition string.
    let composing = unsafe { ImmGetCompositionStringW(input_context, GCS_COMPSTR, None, 0) } > 0;
    // SAFETY: this releases exactly the context acquired above.
    let _ = unsafe { ImmReleaseContext(HWND(window_handle as _), input_context) };
    composing
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{KeyboardKey, KeyboardStroke, keyboard_key_from_virtual_key};

    #[test]
    fn keyboard_stroke_is_plain_data_and_does_not_install_a_hook() {
        let stroke = KeyboardStroke {
            key: KeyboardKey::Enter,
            is_numpad_enter: true,
            is_injected: false,
            shift_pressed: false,
            ime_composing: false,
            foreground_window: 42,
        };
        assert!(stroke.is_numpad_enter);
        assert_eq!(stroke.foreground_window, 42);
    }

    #[test]
    fn escape_is_classified_without_installing_a_hook() {
        assert_eq!(
            keyboard_key_from_virtual_key(0x1b),
            Some(KeyboardKey::Escape)
        );
        assert_eq!(keyboard_key_from_virtual_key(0x41), None);
    }
}
