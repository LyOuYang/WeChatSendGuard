use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult};

use crate::{
    context::{cached_foreground_window, native_context_cache_is_dirty},
    ffi,
};

const RETURN_KEY_CODE: u16 = 36;
const NUMPAD_ENTER_KEY_CODE: u16 = 76;
const ESCAPE_KEY_CODE: u16 = 53;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardKey {
    Enter,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardStroke {
    pub key: KeyboardKey,
    pub is_numpad_enter: bool,
    pub is_injected: bool,
    pub shift_pressed: bool,
    pub modifier_pressed: bool,
    pub foreground_window: isize,
    pub context_cache_dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseClick {
    pub screen_x: i32,
    pub screen_y: i32,
    pub foreground_window: isize,
    pub context_cache_dirty: bool,
}

type KeyDownHandler = dyn Fn(KeyboardStroke) -> bool + Send + Sync + 'static;
type MouseDownHandler = dyn Fn(MouseClick) -> bool + Send + Sync + 'static;

#[derive(Default)]
struct KeyboardState {
    handler: RwLock<Option<Arc<KeyDownHandler>>>,
}

pub struct MacKeyboardHook {
    marker: usize,
    state: Arc<KeyboardState>,
    native_handle: *mut c_void,
    callback_context: *mut Arc<KeyboardState>,
}

impl MacKeyboardHook {
    pub fn new(marker: usize) -> Self {
        Self {
            marker,
            state: Arc::new(KeyboardState::default()),
            native_handle: ptr::null_mut(),
            callback_context: ptr::null_mut(),
        }
    }

    pub fn set_key_down_handler(&self, handler: Arc<KeyDownHandler>) {
        *write_unpoisoned(&self.state.handler) = Some(handler);
    }

    pub fn clear_key_down_handler(&self) {
        *write_unpoisoned(&self.state.handler) = None;
    }

    pub fn is_started(&self) -> bool {
        !self.native_handle.is_null()
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.is_started() {
            return Ok(());
        }
        let callback_context = Box::into_raw(Box::new(Arc::clone(&self.state)));
        // SAFETY: the boxed Arc remains live until `stop` joins the native event-tap thread. The
        // callback is a process-lifetime function with the exact C ABI declared by the bridge.
        let native_handle = unsafe {
            ffi::WSGMacStartKeyboardTap(
                self.marker as u64,
                keyboard_callback,
                callback_context.cast(),
            )
        };
        if native_handle.is_null() {
            // SAFETY: native startup failed synchronously and therefore retained no callback use.
            drop(unsafe { Box::from_raw(callback_context) });
            return Err(PlatformError::new(
                "macos-keyboard-tap-unavailable",
                "Input Monitoring permission is required. Enable WeChatSendGuard in System Settings > Privacy & Security > Input Monitoring, then reopen it.",
            ));
        }
        self.native_handle = native_handle;
        self.callback_context = callback_context;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.clear_key_down_handler();
        if !self.native_handle.is_null() {
            // SAFETY: the handle was returned by the bridge and is stopped at most once.
            unsafe { ffi::WSGMacStopInputTap(self.native_handle) };
            self.native_handle = ptr::null_mut();
        }
        if !self.callback_context.is_null() {
            // SAFETY: stopping the tap joins its callback thread, so the boxed Arc is no longer in
            // use and is reclaimed exactly once.
            drop(unsafe { Box::from_raw(self.callback_context) });
            self.callback_context = ptr::null_mut();
        }
    }
}

impl Drop for MacKeyboardHook {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct MouseState {
    handler: RwLock<Option<Arc<MouseDownHandler>>>,
}

pub struct MacMouseHook {
    marker: usize,
    state: Arc<MouseState>,
    native_handle: *mut c_void,
    callback_context: *mut Arc<MouseState>,
}

impl MacMouseHook {
    pub fn new(marker: usize) -> Self {
        Self {
            marker,
            state: Arc::new(MouseState::default()),
            native_handle: ptr::null_mut(),
            callback_context: ptr::null_mut(),
        }
    }

    pub fn set_left_down_handler(&self, handler: Arc<MouseDownHandler>) {
        *write_unpoisoned(&self.state.handler) = Some(handler);
    }

    pub fn clear_left_down_handler(&self) {
        *write_unpoisoned(&self.state.handler) = None;
    }

    pub fn is_started(&self) -> bool {
        !self.native_handle.is_null()
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.is_started() {
            return Ok(());
        }
        let callback_context = Box::into_raw(Box::new(Arc::clone(&self.state)));
        // SAFETY: see the keyboard-tap lifetime argument above; this tap has an independent boxed
        // Arc and native thread.
        let native_handle = unsafe {
            ffi::WSGMacStartMouseTap(self.marker as u64, mouse_callback, callback_context.cast())
        };
        if native_handle.is_null() {
            // SAFETY: native startup failed synchronously and retained no callback use.
            drop(unsafe { Box::from_raw(callback_context) });
            return Err(PlatformError::new(
                "macos-mouse-tap-unavailable",
                "Input Monitoring permission is required before the send button can be guarded.",
            ));
        }
        self.native_handle = native_handle;
        self.callback_context = callback_context;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.clear_left_down_handler();
        if !self.native_handle.is_null() {
            // SAFETY: the handle was returned by the bridge and is stopped at most once.
            unsafe { ffi::WSGMacStopInputTap(self.native_handle) };
            self.native_handle = ptr::null_mut();
        }
        if !self.callback_context.is_null() {
            // SAFETY: the callback thread has been joined by `WSGMacStopInputTap`.
            drop(unsafe { Box::from_raw(self.callback_context) });
            self.callback_context = ptr::null_mut();
        }
    }
}

impl Drop for MacMouseHook {
    fn drop(&mut self) {
        self.stop();
    }
}

unsafe extern "C" fn keyboard_callback(
    key_code: u16,
    is_injected: bool,
    shift_pressed: bool,
    modifier_pressed: bool,
    context: *mut c_void,
) -> bool {
    if context.is_null() {
        return false;
    }
    // SAFETY: the context points to the boxed Arc retained for the full native tap lifetime.
    let state = unsafe { &*context.cast::<Arc<KeyboardState>>() };
    let stroke = match key_code {
        RETURN_KEY_CODE => KeyboardStroke {
            key: KeyboardKey::Enter,
            is_numpad_enter: false,
            is_injected,
            shift_pressed,
            modifier_pressed,
            foreground_window: cached_foreground_window(),
            context_cache_dirty: native_context_cache_is_dirty(),
        },
        NUMPAD_ENTER_KEY_CODE => KeyboardStroke {
            key: KeyboardKey::Enter,
            is_numpad_enter: true,
            is_injected,
            shift_pressed,
            modifier_pressed,
            foreground_window: cached_foreground_window(),
            context_cache_dirty: native_context_cache_is_dirty(),
        },
        ESCAPE_KEY_CODE => KeyboardStroke {
            key: KeyboardKey::Escape,
            is_numpad_enter: false,
            is_injected,
            shift_pressed,
            modifier_pressed,
            foreground_window: cached_foreground_window(),
            context_cache_dirty: native_context_cache_is_dirty(),
        },
        _ => return false,
    };
    let handler = read_unpoisoned(&state.handler).clone();
    handler
        .is_some_and(|handler| catch_unwind(AssertUnwindSafe(|| handler(stroke))).unwrap_or(false))
}

unsafe extern "C" fn mouse_callback(screen_x: i32, screen_y: i32, context: *mut c_void) -> bool {
    if context.is_null() {
        return false;
    }
    // SAFETY: the context points to the boxed Arc retained for the full native tap lifetime.
    let state = unsafe { &*context.cast::<Arc<MouseState>>() };
    let click = MouseClick {
        screen_x,
        screen_y,
        foreground_window: cached_foreground_window(),
        context_cache_dirty: native_context_cache_is_dirty(),
    };
    let handler = read_unpoisoned(&state.handler).clone();
    handler
        .is_some_and(|handler| catch_unwind(AssertUnwindSafe(|| handler(click))).unwrap_or(false))
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
