use std::{
    collections::HashMap,
    ffi::CStr,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};
use wechat_send_guard_core::ChatContext;
use wechat_send_guard_platform_api::{
    ChatContextProvider, PlatformError, PlatformResult, SendTargetPlatform,
};

use crate::ffi;

const MONITOR_INTERVAL: Duration = Duration::from_millis(80);
const SEND_BUTTON_SNAPSHOT_MAX_AGE: Duration = Duration::from_millis(2_500);
const DRAFT_PREVIEW_CAPACITY: usize = ffi::TEXT_CAPACITY;

static CACHED_FOREGROUND_WINDOW: AtomicIsize = AtomicIsize::new(0);
static CACHED_NATIVE_CONTEXT_GENERATION: AtomicU64 = AtomicU64::new(0);
static WINDOW_BOUNDS: OnceLock<RwLock<HashMap<isize, ScreenRect>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenRect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl ScreenRect {
    fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        (x.is_finite()
            && y.is_finite()
            && width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0)
            .then_some(Self {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            })
    }

    fn contains(self, x: i32, y: i32) -> bool {
        let x = f64::from(x);
        let y = f64::from(y);
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendButtonState {
    NotObserved,
    NotFound,
    Disabled,
    Enabled,
    Fallback,
}

#[derive(Debug, Clone, Copy)]
struct SendButtonSnapshot {
    window_handle: isize,
    observed_at: Option<SystemTime>,
    state: SendButtonState,
    bounds: Option<ScreenRect>,
}

impl Default for SendButtonSnapshot {
    fn default() -> Self {
        Self {
            window_handle: 0,
            observed_at: None,
            state: SendButtonState::NotObserved,
            bounds: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PublishedSnapshot {
    context: ChatContext,
    send_button: SendButtonSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacSendButtonDiagnostic {
    HitEnabledButton,
    HitDisabledButton,
    ClickOutsideButton,
    ButtonNotFound,
    ButtonDisabled,
    ButtonGeometryUnavailable,
    SnapshotUnavailable,
    SnapshotStale,
    SnapshotWindowMismatch,
    UntrustedWindow,
}

impl MacSendButtonDiagnostic {
    pub fn audit_result(self) -> &'static str {
        match self {
            Self::HitEnabledButton => "button-hit-enabled",
            Self::HitDisabledButton => "button-hit-disabled",
            Self::ClickOutsideButton => "point-outside-send-button",
            Self::ButtonNotFound => "send-button-not-found",
            Self::ButtonDisabled => "send-button-disabled",
            Self::ButtonGeometryUnavailable => "send-button-geometry-unavailable",
            Self::SnapshotUnavailable => "send-button-snapshot-unavailable",
            Self::SnapshotStale => "send-button-snapshot-stale",
            Self::SnapshotWindowMismatch => "send-button-window-mismatch",
            Self::UntrustedWindow => "untrusted-window",
        }
    }

    pub fn should_intercept(self) -> bool {
        self == Self::HitEnabledButton
    }
}

#[derive(Debug)]
pub struct MacContextProvider {
    current: RwLock<Arc<PublishedSnapshot>>,
    scan_lock: Mutex<()>,
    generation: AtomicU64,
    observation_enabled: AtomicBool,
    keyboard_enter_observation_enabled: AtomicBool,
    send_button_observation_enabled: AtomicBool,
    last_recognized_chat: RwLock<Option<ChatContext>>,
}

impl Default for MacContextProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MacContextProvider {
    pub fn new() -> Self {
        // Prompting is deliberate and happens only while composing the production adapter. A
        // denied or revoked grant remains a normal unavailable snapshot; it never weakens trust.
        // SAFETY: the bridge displays the standard macOS Accessibility prompt and retains no data.
        let _ = unsafe { ffi::WSGMacRequestAccessibilityAccess() };
        Self {
            current: RwLock::new(Arc::new(PublishedSnapshot {
                context: ChatContext::default(),
                send_button: SendButtonSnapshot::default(),
            })),
            scan_lock: Mutex::new(()),
            generation: AtomicU64::new(0),
            observation_enabled: AtomicBool::new(true),
            keyboard_enter_observation_enabled: AtomicBool::new(true),
            send_button_observation_enabled: AtomicBool::new(true),
            last_recognized_chat: RwLock::new(None),
        }
    }

    pub fn configure_observation(
        &self,
        protection_enabled: bool,
        intercept_keyboard_enter: bool,
        intercept_send_button: bool,
    ) {
        let observe_context =
            protection_enabled && (intercept_keyboard_enter || intercept_send_button);
        self.observation_enabled
            .store(observe_context, Ordering::Release);
        self.keyboard_enter_observation_enabled.store(
            observe_context && intercept_keyboard_enter,
            Ordering::Release,
        );
        self.send_button_observation_enabled
            .store(observe_context && intercept_send_button, Ordering::Release);
        if observe_context {
            self.refresh_foreground();
        } else {
            self.publish(PublishedSnapshot {
                context: ChatContext::default(),
                send_button: SendButtonSnapshot::default(),
            });
        }
    }

    pub fn observation_enabled(&self) -> bool {
        self.observation_enabled.load(Ordering::Acquire)
    }

    pub fn keyboard_enter_observation_enabled(&self) -> bool {
        self.keyboard_enter_observation_enabled
            .load(Ordering::Acquire)
    }

    pub fn send_button_observation_enabled(&self) -> bool {
        self.send_button_observation_enabled.load(Ordering::Acquire)
    }

    pub fn recent_recognized_chat(&self, maximum_age: Duration) -> Option<ChatContext> {
        let context = read_unpoisoned(&self.last_recognized_chat).clone()?;
        let observed_at = context.observed_at?;
        let age = SystemTime::now().duration_since(observed_at).ok()?;
        (age <= maximum_age
            && context.is_known_chat()
            && context.is_message_editor_focused
            && context.is_compatibility_available)
            .then_some(context)
    }

    pub fn refresh_foreground(&self) -> ChatContext {
        if !self.observation_enabled() {
            return self.current();
        }
        let _scan = lock_unpoisoned(&self.scan_lock);
        let observe_button = self.send_button_observation_enabled();
        let mut native = ffi::MacContextSnapshot::default();
        // SAFETY: `native` is a valid out pointer and the bridge retains no reference to it.
        let copied = unsafe { ffi::WSGMacCopyForegroundContext(observe_button, &mut native) };
        if !copied {
            let context = ChatContext::default();
            self.publish(PublishedSnapshot {
                context: context.clone(),
                send_button: SendButtonSnapshot::default(),
            });
            return context;
        }
        self.publish_native(native, observe_button)
    }

    pub fn diagnose_and_gate_send_button_click(
        &self,
        foreground_window: isize,
        screen_x: i32,
        screen_y: i32,
        now: SystemTime,
    ) -> (MacSendButtonDiagnostic, bool) {
        let published = read_unpoisoned(&self.current).clone();
        if !published.context.is_trusted_weixin {
            return (MacSendButtonDiagnostic::UntrustedWindow, false);
        }
        if published.context.window_handle != foreground_window
            || published.send_button.window_handle != foreground_window
        {
            return (MacSendButtonDiagnostic::SnapshotWindowMismatch, false);
        }
        let Some(observed_at) = published.send_button.observed_at else {
            return (MacSendButtonDiagnostic::SnapshotUnavailable, false);
        };
        if now.duration_since(observed_at).unwrap_or(Duration::MAX) > SEND_BUTTON_SNAPSHOT_MAX_AGE {
            return (MacSendButtonDiagnostic::SnapshotStale, false);
        }
        let hit = published
            .send_button
            .bounds
            .is_some_and(|bounds| bounds.contains(screen_x, screen_y));
        match published.send_button.state {
            SendButtonState::Enabled if hit => (MacSendButtonDiagnostic::HitEnabledButton, true),
            SendButtonState::Disabled if hit => (MacSendButtonDiagnostic::HitDisabledButton, false),
            SendButtonState::Fallback if hit => {
                (MacSendButtonDiagnostic::ButtonGeometryUnavailable, true)
            }
            SendButtonState::NotFound => (MacSendButtonDiagnostic::ButtonNotFound, false),
            SendButtonState::Disabled => (MacSendButtonDiagnostic::ButtonDisabled, false),
            SendButtonState::NotObserved => (MacSendButtonDiagnostic::SnapshotUnavailable, false),
            _ => (MacSendButtonDiagnostic::ClickOutsideButton, false),
        }
    }

    fn publish_native(&self, native: ffi::MacContextSnapshot, observe_button: bool) -> ChatContext {
        CACHED_NATIVE_CONTEXT_GENERATION.store(native.context_change_generation, Ordering::Release);
        let observed_at = SystemTime::now();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let process_path = c_buffer_to_string(&native.process_path);
        let title = c_buffer_to_string(&native.chat_title);
        let context = ChatContext {
            window_handle: native.window_id as isize,
            process_id: native.process_id,
            process_path,
            is_trusted_weixin: native.is_trusted_weixin,
            requires_elevation: false,
            is_compatibility_available: native.compatibility_available,
            is_message_editor_focused: native.message_editor_focused,
            is_group_chat: native.is_group_chat,
            is_contact_chat: native.is_contact_chat,
            chat_title: (!title.is_empty()).then_some(title),
            generation,
            observed_at: Some(observed_at),
        };
        let window_bounds = ScreenRect::new(
            native.window_x,
            native.window_y,
            native.window_width,
            native.window_height,
        );
        let button_bounds = ScreenRect::new(
            native.send_button_x,
            native.send_button_y,
            native.send_button_width,
            native.send_button_height,
        );
        let previous = read_unpoisoned(&self.current).clone();
        let send_button = if !observe_button {
            SendButtonSnapshot::default()
        } else if native.send_button_available {
            SendButtonSnapshot {
                window_handle: context.window_handle,
                observed_at: Some(observed_at),
                state: if native.send_button_enabled {
                    SendButtonState::Enabled
                } else {
                    SendButtonState::Disabled
                },
                bounds: button_bounds,
            }
        } else if previous.send_button.window_handle == context.window_handle
            && previous.send_button.bounds.is_some()
            && previous.send_button.observed_at.is_some_and(|time| {
                observed_at.duration_since(time).unwrap_or(Duration::MAX)
                    <= SEND_BUTTON_SNAPSHOT_MAX_AGE
            })
        {
            SendButtonSnapshot {
                window_handle: context.window_handle,
                observed_at: previous.send_button.observed_at,
                state: SendButtonState::Fallback,
                bounds: previous.send_button.bounds,
            }
        } else {
            SendButtonSnapshot {
                window_handle: context.window_handle,
                observed_at: Some(observed_at),
                state: SendButtonState::NotFound,
                bounds: None,
            }
        };
        if let Some(bounds) = window_bounds
            && context.window_handle != 0
        {
            write_unpoisoned(window_bounds_map()).insert(context.window_handle, bounds);
        }
        if context.is_known_chat()
            && context.is_compatibility_available
            && context.is_message_editor_focused
        {
            *write_unpoisoned(&self.last_recognized_chat) = Some(context.clone());
        }
        self.publish(PublishedSnapshot {
            context: context.clone(),
            send_button,
        });
        context
    }

    fn publish(&self, snapshot: PublishedSnapshot) {
        let foreground = if snapshot.context.is_trusted_weixin {
            snapshot.context.window_handle
        } else {
            0
        };
        CACHED_FOREGROUND_WINDOW.store(foreground, Ordering::Release);
        *write_unpoisoned(&self.current) = Arc::new(snapshot);
    }
}

impl ChatContextProvider for MacContextProvider {
    fn current(&self) -> ChatContext {
        read_unpoisoned(&self.current).context.clone()
    }

    fn refresh_now(&self) -> PlatformResult<ChatContext> {
        Ok(self.refresh_foreground())
    }
}

impl SendTargetPlatform for MacContextProvider {
    fn restore_editor_focus_and_refresh(
        &self,
        expected: &ChatContext,
    ) -> PlatformResult<ChatContext> {
        let _scan = lock_unpoisoned(&self.scan_lock);
        let mut native = ffi::MacContextSnapshot::default();
        // SAFETY: expected IDs are value types, `native` is a valid out pointer, and no pointer is
        // retained after the synchronous bridge call.
        if !unsafe {
            ffi::WSGMacRestoreEditorFocusAndCopyContext(
                expected.window_handle as i64,
                expected.process_id,
                &mut native,
            )
        } {
            return Err(PlatformError::new(
                "macos-focus-restore-failed",
                "The original WeChat editor could not be restored and revalidated.",
            ));
        }
        Ok(self.publish_native(native, self.send_button_observation_enabled()))
    }

    fn read_draft_preview(&self, expected: &ChatContext) -> PlatformResult<Option<String>> {
        let mut output = [0i8; DRAFT_PREVIEW_CAPACITY];
        // SAFETY: the bridge receives a valid writable buffer and only reads the expected IDs.
        if !unsafe {
            ffi::WSGMacCopyDraftPreview(
                expected.window_handle as i64,
                expected.process_id,
                output.as_mut_ptr(),
                output.len(),
            )
        } {
            return Err(PlatformError::new(
                "macos-draft-preview-unavailable",
                "The draft preview could not be read from the expected editor.",
            ));
        }
        let preview = c_buffer_to_string(&output);
        Ok((!preview.is_empty()).then_some(preview))
    }
}

pub struct MacContextMonitor {
    provider: Arc<MacContextProvider>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MacContextMonitor {
    pub fn new(provider: Arc<MacContextProvider>) -> Self {
        Self {
            provider,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        self.provider.refresh_foreground();
        self.running.store(true, Ordering::Release);
        let running = Arc::clone(&self.running);
        let provider = Arc::clone(&self.provider);
        self.worker = Some(
            thread::Builder::new()
                .name("wechat-send-guard-macos-context".to_owned())
                .spawn(move || {
                    while running.load(Ordering::Acquire) {
                        if provider.observation_enabled() {
                            provider.refresh_foreground();
                        }
                        thread::sleep(MONITOR_INTERVAL);
                    }
                })
                .map_err(|error| {
                    PlatformError::new("macos-context-monitor-start", error.to_string())
                })?,
        );
        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for MacContextMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn cached_foreground_window() -> isize {
    CACHED_FOREGROUND_WINDOW.load(Ordering::Acquire)
}

pub(crate) fn native_context_cache_is_dirty() -> bool {
    // SAFETY: both bridge functions return process-global atomics and retain no Rust data.
    let native_generation = unsafe { ffi::WSGMacContextChangeGeneration() };
    let frontmost_is_wechat = unsafe { ffi::WSGMacFrontmostIsWeChat() };
    frontmost_is_wechat
        && native_generation != CACHED_NATIVE_CONTEXT_GENERATION.load(Ordering::Acquire)
}

pub(crate) fn cached_window_bounds(window_handle: isize) -> Option<ScreenRect> {
    read_unpoisoned(window_bounds_map())
        .get(&window_handle)
        .copied()
}

fn window_bounds_map() -> &'static RwLock<HashMap<isize, ScreenRect>> {
    WINDOW_BOUNDS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn c_buffer_to_string<const N: usize>(buffer: &[std::ffi::c_char; N]) -> String {
    // SAFETY: every native snapshot starts zeroed and the bridge uses bounded NUL-terminated
    // copies, so a terminator is always present within the fixed-size array.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
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
    use super::{MacSendButtonDiagnostic, ScreenRect};

    #[test]
    fn screen_rect_requires_finite_positive_geometry() {
        assert!(ScreenRect::new(10.0, 20.0, 30.0, 40.0).is_some());
        assert!(ScreenRect::new(10.0, 20.0, 0.0, 40.0).is_none());
        assert!(ScreenRect::new(f64::NAN, 20.0, 30.0, 40.0).is_none());
    }

    #[test]
    fn only_an_exact_enabled_button_is_a_direct_hit() {
        assert!(MacSendButtonDiagnostic::HitEnabledButton.should_intercept());
        assert!(!MacSendButtonDiagnostic::ButtonGeometryUnavailable.should_intercept());
    }
}
