use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    },
};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
            LLKHF_INJECTED, LLMHF_INJECTED, MSLLHOOKSTRUCT, SetWindowsHookExW, UnhookWindowsHookEx,
            WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
            WM_SYSKEYDOWN, WM_SYSKEYUP,
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
    pub modifier_pressed: bool,
    pub foreground_window: isize,
}

/// A physical left mouse-button down event. It deliberately contains only screen coordinates and
/// the foreground window, never text, a UI Automation element, or chat metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseClick {
    pub screen_x: i32,
    pub screen_y: i32,
    pub foreground_window: isize,
}

type KeyDownHandler = dyn Fn(KeyboardStroke) -> bool + Send + Sync + 'static;
type MouseDownHandler = dyn Fn(MouseClick) -> bool + Send + Sync + 'static;

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

#[derive(Default)]
struct MouseHookState {
    marker: usize,
    handler: RwLock<Option<Arc<MouseDownHandler>>>,
    suppress_left_button_up: AtomicBool,
    suppressed_window: AtomicIsize,
}

/// Low-level mouse hook used by the send-button strategy. The callback is deliberately limited to
/// left-button transitions; callers return `true` only after a fast, cached decision, and the
/// matching button-up is then suppressed as well so Windows cannot synthesize a partial click.
pub struct WindowsMouseHook {
    state: Arc<MouseHookState>,
    native_hook: isize,
}

impl WindowsMouseHook {
    pub fn new(marker: usize) -> Self {
        Self {
            state: Arc::new(MouseHookState {
                marker,
                ..MouseHookState::default()
            }),
            native_hook: 0,
        }
    }

    pub fn set_left_down_handler(&self, handler: Arc<MouseDownHandler>) {
        *write_unpoisoned(&self.state.handler) = Some(handler);
    }

    pub fn clear_left_down_handler(&self) {
        *write_unpoisoned(&self.state.handler) = None;
    }

    pub fn is_started(&self) -> bool {
        self.native_hook != 0
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.is_started() {
            return Ok(());
        }

        let active = active_mouse_hook();
        {
            let mut slot = lock_unpoisoned(active);
            if slot.is_some() {
                return Err(PlatformError::new(
                    "mouse-hook-already-active",
                    "Only one low-level mouse hook may be active in this process.",
                ));
            }
            *slot = Some(Arc::clone(&self.state));
        }

        // SAFETY: the callback is a process-lifetime function pointer and the active state is
        // retained in a global slot before installation.
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), None, 0) };
        match hook {
            Ok(hook) => {
                self.native_hook = hook.0 as isize;
                Ok(())
            }
            Err(error) => {
                *lock_unpoisoned(active) = None;
                Err(PlatformError::new(
                    "mouse-hook-install-failed",
                    error.to_string(),
                ))
            }
        }
    }

    pub fn stop(&mut self) {
        self.clear_left_down_handler();
        self.state
            .suppress_left_button_up
            .store(false, Ordering::Release);
        self.state.suppressed_window.store(0, Ordering::Release);
        if self.native_hook != 0 {
            // SAFETY: this value was returned by SetWindowsHookExW and is unhooked at most once.
            let _ = unsafe { UnhookWindowsHookEx(HHOOK(self.native_hook as _)) };
            self.native_hook = 0;
        }

        let mut active = lock_unpoisoned(active_mouse_hook());
        if active
            .as_ref()
            .is_some_and(|state| Arc::ptr_eq(state, &self.state))
        {
            *active = None;
        }
    }
}

impl Drop for WindowsMouseHook {
    fn drop(&mut self) {
        self.stop();
    }
}

static ACTIVE_HOOK: OnceLock<Mutex<Option<Arc<HookState>>>> = OnceLock::new();
static ACTIVE_MOUSE_HOOK: OnceLock<Mutex<Option<Arc<MouseHookState>>>> = OnceLock::new();

fn active_hook() -> &'static Mutex<Option<Arc<HookState>>> {
    ACTIVE_HOOK.get_or_init(|| Mutex::new(None))
}

fn active_mouse_hook() -> &'static Mutex<Option<Arc<MouseHookState>>> {
    ACTIVE_MOUSE_HOOK.get_or_init(|| Mutex::new(None))
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

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        let state = lock_unpoisoned(active_mouse_hook()).clone();
        if let Some(state) = state {
            // SAFETY: Windows invokes a low-level mouse hook with LPARAM pointing at a valid
            // MSLLHOOKSTRUCT for non-negative callback codes. We copy it immediately.
            let data = unsafe { (lparam.0 as *const MSLLHOOKSTRUCT).read_unaligned() };
            match wparam.0 as u32 {
                WM_LBUTTONDOWN if handle_left_mouse_down(&state, data) => {
                    // Pair a consumed down event only with the up event that still belongs to
                    // the same foreground window. This avoids swallowing an unrelated click-up
                    // if focus changes while a confirmation window is being shown.
                    let foreground_window = unsafe { GetForegroundWindow().0 as isize };
                    state
                        .suppressed_window
                        .store(foreground_window, Ordering::Release);
                    state.suppress_left_button_up.store(true, Ordering::Release);
                    return LRESULT(1);
                }
                WM_LBUTTONUP if state.suppress_left_button_up.swap(false, Ordering::AcqRel) => {
                    let suppressed_window = state.suppressed_window.swap(0, Ordering::AcqRel);
                    // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned
                    // value handle.
                    if unsafe { GetForegroundWindow().0 as isize } == suppressed_window {
                        return LRESULT(1);
                    }
                }
                _ => {}
            }
        }
    }

    // SAFETY: forwarding preserves the original callback arguments for pass-through events.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn handle_left_mouse_down(state: &MouseHookState, data: MSLLHOOKSTRUCT) -> bool {
    let is_injected = data.flags & LLMHF_INJECTED != 0 || data.dwExtraInfo == state.marker;
    if is_injected {
        return false;
    }

    // SAFETY: this reads transient foreground-window state and retains no data.
    let foreground_window = unsafe { GetForegroundWindow().0 as isize };
    let click = MouseClick {
        screen_x: data.pt.x,
        screen_y: data.pt.y,
        foreground_window,
    };
    if let Some(handler) = read_unpoisoned(&state.handler).clone() {
        return catch_unwind(AssertUnwindSafe(|| handler(click))).unwrap_or(false);
    }
    false
}

fn handle_key_down(state: &HookState, data: KBDLLHOOKSTRUCT, key: KeyboardKey) -> bool {
    let is_injected = data.flags.contains(LLKHF_INJECTED) || data.dwExtraInfo == state.marker;
    if is_injected {
        return false;
    }

    // SAFETY: these APIs read transient system keyboard/foreground state and retain no data.
    let foreground_window = unsafe { GetForegroundWindow().0 as isize };
    let shift_pressed = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) } < 0;
    let modifier_pressed = shift_pressed
        || unsafe { GetAsyncKeyState(i32::from(VK_CONTROL.0)) } < 0
        || unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) } < 0
        || unsafe { GetAsyncKeyState(i32::from(VK_LWIN.0)) } < 0
        || unsafe { GetAsyncKeyState(i32::from(VK_RWIN.0)) } < 0;
    let stroke = KeyboardStroke {
        key,
        is_numpad_enter: key == KeyboardKey::Enter && data.flags.contains(LLKHF_EXTENDED),
        is_injected,
        shift_pressed,
        modifier_pressed,
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
    use super::{KeyboardKey, KeyboardStroke, MouseClick, keyboard_key_from_virtual_key};

    #[test]
    fn keyboard_stroke_is_plain_data_and_does_not_install_a_hook() {
        let stroke = KeyboardStroke {
            key: KeyboardKey::Enter,
            is_numpad_enter: true,
            is_injected: false,
            shift_pressed: false,
            modifier_pressed: false,
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

    #[test]
    fn mouse_click_is_plain_data_and_does_not_install_a_hook() {
        let click = MouseClick {
            screen_x: 300,
            screen_y: 500,
            foreground_window: 42,
        };
        assert_eq!((click.screen_x, click.screen_y), (300, 500));
        assert_eq!(click.foreground_window, 42);
    }
}
