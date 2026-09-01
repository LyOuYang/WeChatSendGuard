use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
        atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
use wechat_send_guard_core::{ChatContext, ChatTargetKind, SendGuardStateMachine};
use wechat_send_guard_platform_api::{
    ChatContextProvider, ContextDiagnostics, PlatformError, PlatformResult, SendTargetPlatform,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, RECT, RPC_E_CHANGED_MODE, WPARAM},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
    System::Threading::GetCurrentThreadId,
    System::Variant::VARIANT,
    UI::{
        Accessibility::{
            CUIAutomation, HWINEVENTHOOK, IUIAutomation, IUIAutomationElement,
            IUIAutomationTextPattern, IUIAutomationValuePattern,
            PropertyConditionFlags_MatchSubstring, SetWinEventHook, TreeScope_Children,
            TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId,
            UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_NamePropertyId,
            UIA_TextPatternId, UIA_ValuePatternId, UnhookWinEvent,
        },
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW,
            EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GA_ROOT,
            GetAncestor, GetForegroundWindow, GetMessageW, GetWindowRect, MSG, OBJID_WINDOW,
            PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetForegroundWindow, TranslateMessage,
            WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
        },
    },
};

use crate::trust::{
    ProcessTrust, assess_window_trust_for_executable, is_valid_weixin_executable_path,
};

const INPUT_AUTOMATION_ID: &str = "chat_input_field";
const CHAT_NAME_AUTOMATION_ID: &str = "current_chat_name_label";
const SEND_TOOLBAR_AUTOMATION_ID: &str = "chatinput_toolbar_right_view";
const GROUP_TITLE_CLASS_SUFFIX: &str = "mmui::ChatTitleBarChatRoomView";
const DRAFT_PREVIEW_LIMIT: usize = 240;
const FOCUS_RECOVERY_TIMEOUT: Duration = Duration::from_millis(900);
const FOCUS_RECOVERY_RETRY: Duration = Duration::from_millis(30);
const SEND_BUTTON_SNAPSHOT_MAX_AGE: Duration = Duration::from_millis(2_500);
const MONITOR_SCAN_COOLDOWN: Duration = Duration::from_secs(2);
const INPUT_CANDIDATE_MAX_VERTICAL_GAP: i32 = 160;
const FALLBACK_SEND_CANDIDATE_MIN_WIDTH: i32 = 96;
const FALLBACK_SEND_CANDIDATE_MAX_WIDTH: i32 = 220;
const FALLBACK_SEND_CANDIDATE_MIN_HEIGHT: i32 = 48;
const FALLBACK_SEND_CANDIDATE_MAX_HEIGHT: i32 = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ScreenRect {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn from_windows_rect(rect: RECT) -> Option<Self> {
        (rect.right > rect.left && rect.bottom > rect.top).then_some(Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn offset(self, dx: i32, dy: i32) -> Self {
        Self {
            left: self.left.saturating_add(dx),
            top: self.top.saturating_add(dy),
            right: self.right.saturating_add(dx),
            bottom: self.bottom.saturating_add(dy),
        }
    }

    fn rebase(self, old_window: Self, new_window: Self) -> Option<Self> {
        if old_window.width() <= 0
            || old_window.height() <= 0
            || new_window.width() <= 0
            || new_window.height() <= 0
        {
            return None;
        }
        // Keep element coordinates relative to the root origin. We intentionally do not scale
        // them with the root: opening Weixin's third pane expands the root while the chat editor
        // often retains its width, so proportional scaling would move the candidate into the
        // embedded pane. A size change is marked unavailable below and refreshed via WinEvent.
        Some(self.offset(
            new_window.left - old_window.left,
            new_window.top - old_window.top,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendButtonSnapshotState {
    NotObserved,
    QueryFailed,
    NotFound,
    Disabled,
    Enabled,
    StateUnavailable,
    GeometryUnavailable,
}

#[derive(Debug, Clone, Copy)]
struct SendButtonSnapshot {
    window_handle: isize,
    observed_at: Option<SystemTime>,
    state: SendButtonSnapshotState,
    bounds: Option<ScreenRect>,
    /// The top-level Weixin window rectangle captured with the UIA bounds. Keeping this pair
    /// lets the input hook translate button coordinates after a pure window move without a UIA
    /// traversal. A size/DPI change temporarily rebases the conservative candidate and marks the
    /// exact observation unavailable until the event-driven refresh replaces it.
    window_bounds: Option<ScreenRect>,
    window_dpi: u32,
    /// A narrow send-zone fallback retained for a brief UIA layout rebuild. When exact button
    /// geometry is available it is always preferred over this zone.
    candidate_bounds: Option<ScreenRect>,
    /// A second narrow candidate used only across a root resize. One rectangle keeps the old
    /// root-relative position (such as opening a third pane); the other follows a bottom/right
    /// anchored editor (ordinary resize) without turning the entire strip between them hot.
    alternate_candidate_bounds: Option<ScreenRect>,
}

impl SendButtonSnapshot {
    fn not_observed(window_handle: isize, observed_at: Option<SystemTime>) -> Self {
        Self {
            window_handle,
            observed_at,
            state: SendButtonSnapshotState::NotObserved,
            bounds: None,
            window_bounds: None,
            window_dpi: 0,
            candidate_bounds: None,
            alternate_candidate_bounds: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PublishedSnapshot {
    observation_id: u64,
    context: ChatContext,
    send_button: SendButtonSnapshot,
}

#[derive(Debug, Default)]
struct SendButtonInspection {
    toolbar_count: Option<usize>,
    candidate_count: Option<usize>,
    error_code: Option<String>,
}

#[derive(Debug)]
struct WindowInspection {
    context: ChatContext,
    send_button: SendButtonSnapshot,
    diagnostics: ContextDiagnostics,
}

/// Content-free result of matching a physical click against the cached Weixin send-button
/// snapshot. It is also written to the audit log to explain why a click was or was not blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendButtonDiagnostic {
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

impl SendButtonDiagnostic {
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

/// Cached foreground-context provider. Constructing it only allocates an in-memory snapshot;
/// use `refresh_now` or `WindowsContextMonitor::start` to request Windows metadata.
#[derive(Debug)]
pub struct WindowsContextProvider {
    /// Context and send-button data are published as one immutable value. A hook therefore can
    /// never observe a new window context paired with an old button rectangle.
    current: RwLock<Arc<PublishedSnapshot>>,
    current_diagnostics: RwLock<ContextDiagnostics>,
    observation_sequence: AtomicU64,
    observation_enabled: Arc<AtomicBool>,
    keyboard_enter_observation_enabled: Arc<AtomicBool>,
    send_button_observation_enabled: Arc<AtomicBool>,
    observed_window: Arc<AtomicIsize>,
    layout_dirty: Arc<AtomicBool>,
    refresh_signal: Arc<RefreshSignal>,
    last_observation_attempt: Mutex<Option<(isize, Instant)>>,
    last_recognized_chat: RwLock<Option<ChatContext>>,
    trusted_executable: RwLock<TrustedExecutable>,
}

#[derive(Debug, Clone)]
struct TrustedExecutable {
    path: Option<PathBuf>,
    generation: u64,
}

impl Default for WindowsContextProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsContextProvider {
    pub fn new() -> Self {
        Self::new_with_trusted_weixin_executable_path(None)
    }

    /// Creates a provider with the optional auto-detected Windows executable path. Without a
    /// detected path no process is trusted until the application discovers a running Weixin.
    pub fn new_with_trusted_weixin_executable_path(configured_path: Option<&str>) -> Self {
        Self {
            current: RwLock::new(Arc::new(PublishedSnapshot {
                observation_id: 0,
                context: ChatContext::default(),
                send_button: SendButtonSnapshot::not_observed(0, None),
            })),
            current_diagnostics: RwLock::new(ContextDiagnostics::default()),
            observation_sequence: AtomicU64::new(0),
            observation_enabled: Arc::new(AtomicBool::new(true)),
            keyboard_enter_observation_enabled: Arc::new(AtomicBool::new(true)),
            send_button_observation_enabled: Arc::new(AtomicBool::new(true)),
            observed_window: Arc::new(AtomicIsize::new(0)),
            layout_dirty: Arc::new(AtomicBool::new(false)),
            refresh_signal: Arc::new(RefreshSignal::default()),
            last_observation_attempt: Mutex::new(None),
            last_recognized_chat: RwLock::new(None),
            trusted_executable: RwLock::new(TrustedExecutable {
                path: resolved_trusted_weixin_path(configured_path),
                generation: 0,
            }),
        }
    }

    /// Configures the minimum observation work required by the enabled input strategies. The
    /// monitor can still maintain context for Enter while completely skipping send-button UIA
    /// queries when that strategy is disabled.
    pub fn configure_observation(
        &self,
        protection_enabled: bool,
        intercept_keyboard_enter: bool,
        intercept_send_button: bool,
    ) {
        let observe_context =
            protection_enabled && (intercept_keyboard_enter || intercept_send_button);
        self.send_button_observation_enabled
            .store(observe_context && intercept_send_button, Ordering::Release);
        self.keyboard_enter_observation_enabled.store(
            observe_context && intercept_keyboard_enter,
            Ordering::Release,
        );
        self.observation_enabled
            .store(observe_context, Ordering::Release);
        // The first scan after a feature is enabled must rebuild all cached geometry/context;
        // otherwise a button snapshot from a previous enabled interval could be used briefly.
        self.layout_dirty.store(true, Ordering::Release);
        self.refresh_signal.request_immediate();
    }

    pub fn observation_enabled(&self) -> bool {
        self.observation_enabled.load(Ordering::Acquire)
    }

    pub fn send_button_observation_enabled(&self) -> bool {
        self.send_button_observation_enabled.load(Ordering::Acquire)
    }

    pub fn keyboard_enter_observation_enabled(&self) -> bool {
        self.keyboard_enter_observation_enabled
            .load(Ordering::Acquire)
    }

    fn observed_window(&self) -> Arc<AtomicIsize> {
        Arc::clone(&self.observed_window)
    }

    fn layout_dirty(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.layout_dirty)
    }

    fn refresh_signal(&self) -> Arc<RefreshSignal> {
        Arc::clone(&self.refresh_signal)
    }

    /// Changes the exact executable identity used for future observations. The existing cached
    /// context is invalidated before a fresh observation so an older trusted snapshot cannot
    /// authorize a key after this setting changes.
    pub fn set_trusted_weixin_executable_path(&self, configured_path: Option<&str>) {
        let generation = {
            let mut trusted = write_unpoisoned(&self.trusted_executable);
            trusted.path = resolved_trusted_weixin_path(configured_path);
            trusted.generation = trusted.generation.saturating_add(1);
            trusted.generation
        };

        *write_unpoisoned(&self.last_recognized_chat) = None;
        self.publish(ChatContext::default(), generation);
        let _ = self.refresh_foreground();
    }

    /// Returns the most recent usable Weixin chat observed before another window took focus.
    /// This is intended only for configuration actions that necessarily activate this app, never
    /// for authorizing a send.
    pub fn recent_recognized_chat(&self, maximum_age: Duration) -> Option<ChatContext> {
        let context = read_unpoisoned(&self.last_recognized_chat).clone()?;
        let observed_at = context.observed_at?;
        let age = SystemTime::now().duration_since(observed_at).ok()?;
        (age <= maximum_age && is_recognized_chat(&context)).then_some(context)
    }

    pub fn refresh_foreground(&self) -> ChatContext {
        if !self.observation_enabled() {
            return read_unpoisoned(&self.current).context.clone();
        }
        let observation_id = self.next_observation_id();
        // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
        let window_handle = unsafe { GetForegroundWindow().0 as isize };
        self.observed_window.store(window_handle, Ordering::Release);
        *lock_unpoisoned(&self.last_observation_attempt) = Some((window_handle, Instant::now()));
        let scan_started_at = SystemTime::now();
        let (trusted_path, trust_generation) = {
            let trusted = read_unpoisoned(&self.trusted_executable);
            (trusted.path.clone(), trusted.generation)
        };
        let trust = trusted_path
            .as_deref()
            .and_then(|path| assess_window_trust_for_executable(window_handle, path).ok())
            .unwrap_or_else(|| ProcessTrust {
                process_id: 0,
                process_path: String::new(),
                is_trusted_weixin: false,
                requires_elevation: false,
            });
        let mut diagnostics = diagnostics_from_trust(
            observation_id,
            window_handle,
            &trust,
            self.send_button_observation_enabled(),
        );

        let (mut candidate, mut send_button, mut diagnostics, observation_completed) = if !trust
            .is_trusted_weixin
        {
            diagnostics.query_status = "not-queried-untrusted-window".to_owned();
            (
                context_from_trust(window_handle, trust, scan_started_at),
                SendButtonSnapshot::not_observed(window_handle, Some(scan_started_at)),
                diagnostics,
                true,
            )
        } else {
            // Publish trust before the potentially slow UIA query. Input callbacks can then fail
            // closed while a newly focused Weixin window is still being recognized. For the same
            // trusted HWND, preserve the last complete chat context while only marking the button
            // observation as in progress. A three-pane window can make the UIA query relatively
            // slow; clearing compatibility/focus at the start of every periodic scan would leave
            // the application in `context-unavailable` for most of its lifetime and silently eat
            // otherwise valid clicks. The preserved timestamp remains unchanged, so stale
            // contexts still cannot authorize pass-through, and confirmation completion performs
            // the normal fresh target revalidation before any input is injected.
            let previous = read_unpoisoned(&self.current).clone();
            let previous_context = previous.context.clone();
            let previous_button = previous.send_button;
            let same_window = previous.context.window_handle == window_handle
                && previous.context.is_trusted_weixin;
            let provisional_context = if same_window {
                previous.context.clone()
            } else {
                context_from_trust(window_handle, trust.clone(), scan_started_at)
            };
            // A different HWND has no reusable identity and therefore remains conservative. For
            // the same HWND, the complete prior context is intentionally retained; its age makes
            // the service fail closed for non-protected/pass-through decisions while still
            // allowing a protected click to open the confirmation immediately.
            let provisional_button = if previous_button.window_handle == window_handle {
                SendButtonSnapshot {
                    state: SendButtonSnapshotState::QueryFailed,
                    ..previous_button
                }
            } else {
                SendButtonSnapshot::not_observed(window_handle, None)
            };
            drop(previous);
            diagnostics.query_status = "query-in-progress".to_owned();
            self.publish_diagnostics(diagnostics.clone());
            self.publish_with_button_for_observation(
                provisional_context,
                provisional_button,
                trust_generation,
                observation_id,
                false,
            );

            let inspect_send_button = self.send_button_observation_enabled();
            match inspect_supported_weixin_window(
                observation_id,
                window_handle,
                trust.clone(),
                scan_started_at,
                inspect_send_button,
            ) {
                Ok(inspection) if inspection.context.is_compatibility_available => (
                    inspection.context,
                    inspection.send_button,
                    inspection.diagnostics,
                    true,
                ),
                Ok(inspection) => {
                    // Weixin's three-pane provider can return a technically successful query
                    // while temporarily omitting the editor/title nodes. Treat that as an
                    // incomplete observation, not as proof that the current chat disappeared.
                    // Retain a recognized same-window identity while accepting any newly found
                    // button geometry. Its original timestamp remains stale, so it can never
                    // authorize a pass-through and is revalidated before confirmed injection.
                    let same_window_recognized = previous_context.window_handle == window_handle
                        && is_recognized_chat(&previous_context);
                    if same_window_recognized {
                        (
                            previous_context.clone(),
                            inspection.send_button,
                            inspection.diagnostics,
                            false,
                        )
                    } else {
                        (
                            inspection.context,
                            inspection.send_button,
                            inspection.diagnostics,
                            true,
                        )
                    }
                }
                Err(error) => {
                    // Keep the last same-window identity during a transient UIA failure. Its old
                    // timestamp deliberately remains stale, so the service can open a confirmation
                    // and the final focus/target revalidation still decides whether to inject.
                    let same_window = previous_context.window_handle == window_handle
                        && previous_context.is_trusted_weixin;
                    let candidate = if same_window {
                        previous_context.clone()
                    } else {
                        context_from_trust(window_handle, trust, scan_started_at)
                    };
                    let send_button = if previous_button.window_handle == window_handle {
                        SendButtonSnapshot {
                            window_handle,
                            observed_at: previous_button.observed_at,
                            state: SendButtonSnapshotState::QueryFailed,
                            bounds: previous_button.bounds,
                            window_bounds: previous_button.window_bounds,
                            window_dpi: previous_button.window_dpi,
                            candidate_bounds: previous_button.candidate_bounds,
                            alternate_candidate_bounds: previous_button.alternate_candidate_bounds,
                        }
                    } else {
                        SendButtonSnapshot::not_observed(window_handle, None)
                    };
                    diagnostics.query_status = "query-failed".to_owned();
                    diagnostics.error_code = Some(diagnostic_error_code(&error));
                    diagnostics.scan_duration_milliseconds =
                        scan_started_at.elapsed().unwrap_or_default().as_millis();
                    (candidate, send_button, diagnostics, false)
                }
            }
        };
        // For a successful observation the timestamp represents when the complete observation
        // finished, rather than when the potentially slow UIA traversal started. Failed scans keep
        // the prior timestamp so stale data cannot silently become fresh.
        if observation_completed {
            let completed_at = SystemTime::now();
            candidate.observed_at = Some(completed_at);
            send_button.observed_at = Some(completed_at);
            self.layout_dirty.store(false, Ordering::Release);
        }
        // UIA calls may outlive a focus change. Never publish a completed scan for an old window;
        // publish a conservative snapshot for the new foreground instead so the next monitor pass
        // can recognize it without opening a fail-open interval.
        // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
        let latest_window_handle = unsafe { GetForegroundWindow().0 as isize };
        if latest_window_handle != window_handle {
            let latest_trust = trusted_path
                .as_deref()
                .and_then(|path| {
                    assess_window_trust_for_executable(latest_window_handle, path).ok()
                })
                .unwrap_or_else(|| ProcessTrust {
                    process_id: 0,
                    process_path: String::new(),
                    is_trusted_weixin: false,
                    requires_elevation: false,
                });
            let latest_observed_at = SystemTime::now();
            let mut latest_diagnostics = diagnostics_from_trust(
                observation_id,
                latest_window_handle,
                &latest_trust,
                self.send_button_observation_enabled(),
            );
            latest_diagnostics.query_status = "not-queried-foreground-changed".to_owned();
            let published = self.publish_with_button_for_observation(
                context_from_trust(latest_window_handle, latest_trust, latest_observed_at),
                SendButtonSnapshot::not_observed(latest_window_handle, Some(latest_observed_at)),
                trust_generation,
                observation_id,
                false,
            );
            self.publish_diagnostics(latest_diagnostics);
            return published;
        }
        let published = self.publish_with_button_for_observation(
            candidate,
            send_button,
            trust_generation,
            observation_id,
            true,
        );
        diagnostics.scan_duration_milliseconds =
            scan_started_at.elapsed().unwrap_or_default().as_millis();
        self.publish_diagnostics(diagnostics);
        published
    }

    /// Reconciles the foreground window for the background monitor. WinEvent notifications pass
    /// `force = true` and bypass the cooldown; timer wakeups reuse the latest immutable snapshot
    /// for a short period so a slow or temporarily unavailable UIA provider cannot cause repeated
    /// full-tree scans. WinEvent notifications remain the fast path for actual changes.
    fn refresh_foreground_for_monitor(&self, force: bool) -> ChatContext {
        if !self.observation_enabled() {
            return read_unpoisoned(&self.current).context.clone();
        }
        // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
        let window_handle = unsafe { GetForegroundWindow().0 as isize };
        let layout_dirty = self.layout_dirty.load(Ordering::Acquire);
        let recent_attempt = {
            let attempt = lock_unpoisoned(&self.last_observation_attempt);
            attempt.is_some_and(|(attempted_window, started_at)| {
                attempted_window == window_handle && started_at.elapsed() < MONITOR_SCAN_COOLDOWN
            })
        };
        if !force && !layout_dirty && recent_attempt {
            return read_unpoisoned(&self.current).context.clone();
        }
        self.refresh_foreground()
    }

    fn next_observation_id(&self) -> u64 {
        self.observation_sequence
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1)
    }

    fn publish_diagnostics(&self, diagnostics: ContextDiagnostics) {
        let mut current = write_unpoisoned(&self.current_diagnostics);
        if current.observation_id <= diagnostics.observation_id {
            *current = diagnostics;
        }
    }

    fn publish(&self, candidate: ChatContext, trust_generation: u64) -> ChatContext {
        self.publish_with_button(
            candidate,
            SendButtonSnapshot::not_observed(0, None),
            trust_generation,
        )
    }

    fn publish_with_button(
        &self,
        candidate: ChatContext,
        send_button: SendButtonSnapshot,
        trust_generation: u64,
    ) -> ChatContext {
        let observation_id = self.next_observation_id();
        self.publish_with_button_for_observation(
            candidate,
            send_button,
            trust_generation,
            observation_id,
            true,
        )
    }

    fn publish_with_button_for_observation(
        &self,
        mut candidate: ChatContext,
        send_button: SendButtonSnapshot,
        trust_generation: u64,
        observation_id: u64,
        remember_chat: bool,
    ) -> ChatContext {
        let trusted = read_unpoisoned(&self.trusted_executable);
        if trusted.generation != trust_generation {
            return read_unpoisoned(&self.current).context.clone();
        }

        // Hold the write lock while deriving the generation and replacing both values. This
        // keeps concurrent refreshes from publishing an older context with a newer button (or
        // vice versa) and makes the generation monotonic even if a manual refresh races the
        // monitor thread.
        let mut current = write_unpoisoned(&self.current);
        if current.observation_id > observation_id {
            return current.context.clone();
        }
        candidate.generation = if same_observation(&current.context, &candidate) {
            current.context.generation
        } else {
            current.context.generation.saturating_add(1)
        };
        *current = Arc::new(PublishedSnapshot {
            observation_id,
            context: candidate.clone(),
            send_button,
        });
        drop(current);
        if remember_chat && candidate.is_trusted_weixin {
            let mut last_recognized_chat = write_unpoisoned(&self.last_recognized_chat);
            *last_recognized_chat = is_recognized_chat(&candidate).then(|| candidate.clone());
        }
        candidate
    }

    fn focus_expected_editor(&self, expected: &ChatContext) -> PlatformResult<bool> {
        if expected.window_handle == 0
            || !expected.is_trusted_weixin
            || !expected.is_known_chat()
            || expected.normalized_chat_title().is_empty()
        {
            return Ok(false);
        }

        // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
        let foreground = unsafe { GetForegroundWindow().0 as isize };
        if foreground != expected.window_handle {
            return Ok(false);
        }

        let fresh = self.refresh_foreground();
        if !SendGuardStateMachine::represents_same_session(expected, &fresh) {
            return Ok(false);
        }

        with_automation(|automation| {
            let root = element_from_handle(automation, expected.window_handle)?;
            let Some(editor) =
                find_by_automation_id_suffix(automation, &root, INPUT_AUTOMATION_ID)?
            else {
                return Ok(false);
            };
            // SAFETY: editor belongs to the current foreground window and the COM proxy remains
            // valid through this synchronous call. Any UIA failure is converted to no focus.
            unsafe { editor.SetFocus() }
                .map(|_| true)
                .map_err(|error| PlatformError::new("editor-focus-failed", error.to_string()))
        })
    }

    fn read_preview_for_expected(&self, expected: &ChatContext) -> PlatformResult<Option<String>> {
        let fresh = self.refresh_foreground();
        if !SendGuardStateMachine::represents_same_session(expected, &fresh) {
            return Ok(None);
        }

        with_automation(|automation| {
            let root = element_from_handle(automation, expected.window_handle)?;
            let Some(editor) =
                find_by_automation_id_suffix(automation, &root, INPUT_AUTOMATION_ID)?
            else {
                return Ok(None);
            };
            Ok(read_draft_preview(&editor))
        })
    }

    /// Classifies a click using only the most recent monitor snapshot. This method is deliberately
    /// small and read-only so it can run in the low-level mouse callback.
    pub fn classify_send_button_click(
        &self,
        window_handle: isize,
        screen_x: i32,
        screen_y: i32,
        now: SystemTime,
    ) -> SendButtonDiagnostic {
        let published = read_unpoisoned(&self.current).clone();
        let Some(published) = snapshot_for_click(
            &published,
            window_handle,
            self.layout_dirty.load(Ordering::Acquire),
        ) else {
            return SendButtonDiagnostic::SnapshotUnavailable;
        };
        classify_send_button_click_from_snapshot(&published, window_handle, screen_x, screen_y, now)
    }

    /// Atomically diagnoses and gates one click from the same immutable snapshot. Keeping these
    /// two decisions together closes the race where a monitor publication landed between the
    /// diagnostic log entry and the low-level hook's consume/pass decision.
    pub fn diagnose_and_gate_send_button_click(
        &self,
        window_handle: isize,
        screen_x: i32,
        screen_y: i32,
        now: SystemTime,
    ) -> (SendButtonDiagnostic, bool) {
        let published = read_unpoisoned(&self.current).clone();
        let effective = snapshot_for_click(
            &published,
            window_handle,
            self.layout_dirty.load(Ordering::Acquire),
        );
        let diagnosis =
            effective
                .as_ref()
                .map_or(SendButtonDiagnostic::SnapshotUnavailable, |published| {
                    classify_send_button_click_from_snapshot(
                        published,
                        window_handle,
                        screen_x,
                        screen_y,
                        now,
                    )
                });
        let should_intercept = effective.as_ref().is_some_and(|published| {
            diagnosis_allows_interception(diagnosis)
                && click_is_candidate_in_snapshot(
                    published,
                    window_handle,
                    screen_x,
                    screen_y,
                    diagnosis != SendButtonDiagnostic::HitEnabledButton,
                )
        });
        (diagnosis, should_intercept)
    }

    /// Returns whether a click is inside the last known send-button/toolbar candidate area. This
    /// is intentionally separate from the diagnostic enum so stale snapshots can retain their
    /// content-free audit meaning while the input gate still makes a conservative decision.
    pub fn click_is_send_button_candidate(
        &self,
        window_handle: isize,
        screen_x: i32,
        screen_y: i32,
    ) -> bool {
        let published = read_unpoisoned(&self.current).clone();
        snapshot_for_click(
            &published,
            window_handle,
            self.layout_dirty.load(Ordering::Acquire),
        )
        .is_some_and(|published| {
            click_is_candidate_in_snapshot(&published, window_handle, screen_x, screen_y, false)
        })
    }

    /// Decides whether the low-level mouse hook should consume this click. This compatibility
    /// wrapper rechecks the current snapshot; production uses `diagnose_and_gate_send_button_click`
    /// so diagnosis and gating are fully atomic.
    pub fn should_intercept_send_button_click(
        &self,
        diagnosis: SendButtonDiagnostic,
        window_handle: isize,
        screen_x: i32,
        screen_y: i32,
    ) -> bool {
        if !diagnosis_allows_interception(diagnosis) {
            return false;
        }
        let published = read_unpoisoned(&self.current).clone();
        snapshot_for_click(
            &published,
            window_handle,
            self.layout_dirty.load(Ordering::Acquire),
        )
        .is_some_and(|published| {
            click_is_candidate_in_snapshot(
                &published,
                window_handle,
                screen_x,
                screen_y,
                diagnosis != SendButtonDiagnostic::HitEnabledButton,
            )
        })
    }
}

fn classify_send_button_click_from_snapshot(
    published: &PublishedSnapshot,
    window_handle: isize,
    screen_x: i32,
    screen_y: i32,
    now: SystemTime,
) -> SendButtonDiagnostic {
    let context = &published.context;
    if !context.is_trusted_weixin {
        return SendButtonDiagnostic::UntrustedWindow;
    }

    let snapshot = published.send_button;
    if snapshot.window_handle != window_handle {
        return SendButtonDiagnostic::SnapshotWindowMismatch;
    }
    let Some(observed_at) = snapshot.observed_at else {
        return SendButtonDiagnostic::SnapshotUnavailable;
    };
    if now
        .duration_since(observed_at)
        .map_or(true, |age| age > SEND_BUTTON_SNAPSHOT_MAX_AGE)
    {
        return SendButtonDiagnostic::SnapshotStale;
    }

    match snapshot.state {
        SendButtonSnapshotState::NotObserved | SendButtonSnapshotState::QueryFailed => {
            SendButtonDiagnostic::SnapshotUnavailable
        }
        SendButtonSnapshotState::NotFound => SendButtonDiagnostic::ButtonNotFound,
        SendButtonSnapshotState::Disabled => {
            if snapshot
                .bounds
                .is_some_and(|bounds| bounds.contains(screen_x, screen_y))
            {
                SendButtonDiagnostic::HitDisabledButton
            } else {
                SendButtonDiagnostic::ButtonDisabled
            }
        }
        SendButtonSnapshotState::StateUnavailable
        | SendButtonSnapshotState::GeometryUnavailable => {
            SendButtonDiagnostic::ButtonGeometryUnavailable
        }
        SendButtonSnapshotState::Enabled => {
            if snapshot
                .bounds
                .is_some_and(|bounds| bounds.contains(screen_x, screen_y))
            {
                SendButtonDiagnostic::HitEnabledButton
            } else {
                SendButtonDiagnostic::ClickOutsideButton
            }
        }
    }
}

fn diagnosis_allows_interception(diagnosis: SendButtonDiagnostic) -> bool {
    matches!(
        diagnosis,
        SendButtonDiagnostic::HitEnabledButton
            | SendButtonDiagnostic::ButtonNotFound
            | SendButtonDiagnostic::SnapshotStale
            | SendButtonDiagnostic::SnapshotUnavailable
            | SendButtonDiagnostic::ButtonGeometryUnavailable
    )
}

fn click_is_candidate_in_snapshot(
    published: &PublishedSnapshot,
    window_handle: isize,
    screen_x: i32,
    screen_y: i32,
    prefer_fallback: bool,
) -> bool {
    if published.context.window_handle != window_handle || !published.context.is_trusted_weixin {
        return false;
    }
    let bounds = if prefer_fallback {
        published
            .send_button
            .candidate_bounds
            .or(published.send_button.bounds)
    } else {
        published
            .send_button
            .bounds
            .or(published.send_button.candidate_bounds)
    };
    bounds.is_some_and(|bounds| bounds.contains(screen_x, screen_y))
        || published
            .send_button
            .alternate_candidate_bounds
            .is_some_and(|bounds| bounds.contains(screen_x, screen_y))
}

fn snapshot_for_click(
    published: &PublishedSnapshot,
    window_handle: isize,
    layout_dirty: bool,
) -> Option<PublishedSnapshot> {
    let mut effective = published.clone();
    if effective.send_button.window_handle != window_handle {
        return Some(effective);
    }
    if effective.send_button.window_bounds.is_some() {
        effective.send_button = rebase_snapshot_for_current_window(effective.send_button)?;
    }
    if layout_dirty && effective.send_button.window_handle == window_handle {
        effective.send_button.state = SendButtonSnapshotState::QueryFailed;
    }
    Some(effective)
}

fn current_window_bounds(window_handle: isize) -> Option<ScreenRect> {
    if window_handle == 0 {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: the HWND is a copied native handle. GetWindowRect only writes to the stack value.
    unsafe { GetWindowRect(HWND(window_handle as _), &mut rect).ok()? };
    ScreenRect::from_windows_rect(rect)
}

fn current_window_dpi(window_handle: isize) -> u32 {
    if window_handle == 0 {
        return 0;
    }
    // SAFETY: the HWND is a copied native handle and the function retains no borrow.
    unsafe { GetDpiForWindow(HWND(window_handle as _)) }
}

fn rebase_snapshot_for_current_window(
    mut snapshot: SendButtonSnapshot,
) -> Option<SendButtonSnapshot> {
    let observed_window = snapshot.window_bounds?;
    let current_window = current_window_bounds(snapshot.window_handle)?;
    let current_dpi = current_window_dpi(snapshot.window_handle);
    let geometry_changed = observed_window.width() != current_window.width()
        || observed_window.height() != current_window.height()
        || (snapshot.window_dpi != 0 && current_dpi != 0 && snapshot.window_dpi != current_dpi);
    snapshot.bounds = snapshot
        .bounds
        .and_then(|bounds| bounds.rebase(observed_window, current_window));
    snapshot.candidate_bounds = snapshot
        .candidate_bounds
        .and_then(|bounds| bounds.rebase(observed_window, current_window));
    snapshot.alternate_candidate_bounds = snapshot
        .alternate_candidate_bounds
        .and_then(|bounds| bounds.rebase(observed_window, current_window));
    if geometry_changed {
        // Preserve the old root-relative candidate for third-pane expansion, and also retain one
        // bottom/right anchored alternative for ordinary resize. Both are narrow; the space
        // between them remains pass-through. A WinEvent refresh replaces them shortly after.
        snapshot.alternate_candidate_bounds = snapshot.candidate_bounds.map(|bounds| {
            bounds.offset(
                current_window.width() - observed_window.width(),
                current_window.height() - observed_window.height(),
            )
        });
        snapshot.state = SendButtonSnapshotState::QueryFailed;
    }
    snapshot.window_bounds = Some(current_window);
    snapshot.window_dpi = current_dpi;
    Some(snapshot)
}

fn resolved_trusted_weixin_path(configured_path: Option<&str>) -> Option<PathBuf> {
    match configured_path {
        None => None,
        Some(value) => {
            let path = PathBuf::from(value.trim().trim_matches('"').trim());
            is_valid_weixin_executable_path(&path).then_some(path)
        }
    }
}

fn is_recognized_chat(context: &ChatContext) -> bool {
    context.is_trusted_weixin
        && context.is_compatibility_available
        && context.is_message_editor_focused
        && context.is_known_chat()
        && !context.normalized_chat_title().is_empty()
}

impl ChatContextProvider for WindowsContextProvider {
    fn current(&self) -> ChatContext {
        let mut context = read_unpoisoned(&self.current).context.clone();
        if self.layout_dirty.load(Ordering::Acquire) {
            // A chat/layout event may arrive just before its background UIA refresh completes.
            // Mark the immutable snapshot stale during that narrow window so it cannot authorize
            // a pass-through for a chat that may already have changed.
            context.observed_at = Some(SystemTime::UNIX_EPOCH);
        }
        context
    }

    fn refresh_now(&self) -> PlatformResult<ChatContext> {
        Ok(self.refresh_foreground())
    }

    fn current_diagnostics(&self) -> Option<ContextDiagnostics> {
        Some(read_unpoisoned(&self.current_diagnostics).clone())
    }
}

impl SendTargetPlatform for WindowsContextProvider {
    fn restore_editor_focus_and_refresh(
        &self,
        expected: &ChatContext,
    ) -> PlatformResult<ChatContext> {
        let deadline = Instant::now() + FOCUS_RECOVERY_TIMEOUT;
        while Instant::now() < deadline {
            // SAFETY: HWND is the recorded native handle from a pending confirmation. The API
            // does not retain it and its result is checked by the following refresh.
            let _ = unsafe { SetForegroundWindow(HWND(expected.window_handle as _)) };

            if self.focus_expected_editor(expected)? {
                thread::sleep(FOCUS_RECOVERY_RETRY);
                let revalidated = self.refresh_foreground();
                if SendGuardStateMachine::represents_same_send_target(expected, &revalidated) {
                    return Ok(revalidated);
                }
            }
            thread::sleep(FOCUS_RECOVERY_RETRY);
        }

        Err(PlatformError::new(
            "editor-focus-timeout",
            "The original Weixin message editor could not be restored and revalidated.",
        ))
    }

    fn read_draft_preview(&self, expected: &ChatContext) -> PlatformResult<Option<String>> {
        self.read_preview_for_expected(expected)
    }
}

/// A small cross-thread wakeup used by the WinEvent callback and the reconciliation worker. The
/// callback only flips atomics and wakes the worker; it never performs COM/UIA work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshRequest {
    TimedOut,
    Debounced,
    Immediate,
}

#[derive(Debug, Default)]
struct RefreshSignal {
    pending: AtomicU8,
    wait_lock: Mutex<()>,
    wake: Condvar,
}

impl RefreshSignal {
    fn request_debounced(&self) {
        self.pending.fetch_max(1, Ordering::AcqRel);
        self.wake.notify_one();
    }

    fn request_immediate(&self) {
        self.pending.store(2, Ordering::Release);
        self.wake.notify_one();
    }

    fn wait(&self, timeout: Duration) -> RefreshRequest {
        let pending = self.pending.swap(0, Ordering::AcqRel);
        if pending != 0 {
            return Self::decode(pending);
        }
        let guard = self
            .wait_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.pending.load(Ordering::Acquire) == 0 {
            let _ = self.wake.wait_timeout(guard, timeout);
        }
        Self::decode(self.pending.swap(0, Ordering::AcqRel))
    }

    fn decode(value: u8) -> RefreshRequest {
        match value {
            2.. => RefreshRequest::Immediate,
            1 => RefreshRequest::Debounced,
            _ => RefreshRequest::TimedOut,
        }
    }
}

#[derive(Debug)]
struct WindowEventRuntime {
    signal: Arc<RefreshSignal>,
    observation_enabled: Arc<AtomicBool>,
    observed_window: Arc<AtomicIsize>,
    layout_dirty: Arc<AtomicBool>,
}

static ACTIVE_WINDOW_EVENT_RUNTIME: OnceLock<Mutex<Option<Weak<WindowEventRuntime>>>> =
    OnceLock::new();

fn active_window_event_runtime() -> &'static Mutex<Option<Weak<WindowEventRuntime>>> {
    ACTIVE_WINDOW_EVENT_RUNTIME.get_or_init(|| Mutex::new(None))
}

/// Receives inexpensive WinEvent notifications and schedules a background reconciliation. Child
/// location/reorder events invalidate the layout fingerprint; top-level move events only wake the
/// worker, allowing the click hook to rebase its cached geometry immediately.
unsafe extern "system" fn window_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let runtime = lock_unpoisoned(active_window_event_runtime())
        .as_ref()
        .and_then(Weak::upgrade);
    let Some(runtime) = runtime else {
        return;
    };
    if !runtime.observation_enabled.load(Ordering::Acquire) {
        return;
    }
    if hwnd.0.is_null() {
        return;
    }

    if event == EVENT_SYSTEM_FOREGROUND {
        runtime.signal.request_immediate();
        return;
    }

    // SAFETY: hwnd is supplied by the system callback and the result is an owned HWND value.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    if root.0 as isize != runtime.observed_window.load(Ordering::Acquire) {
        return;
    }

    let recognition_affecting = (event == EVENT_OBJECT_LOCATIONCHANGE
        && id_object != OBJID_WINDOW.0)
        || matches!(
            event,
            EVENT_OBJECT_FOCUS
                | EVENT_OBJECT_HIDE
                | EVENT_OBJECT_NAMECHANGE
                | EVENT_OBJECT_REORDER
                | EVENT_OBJECT_SHOW
        );
    if recognition_affecting {
        runtime.layout_dirty.store(true, Ordering::Release);
    }
    if matches!(
        event,
        EVENT_OBJECT_FOCUS | EVENT_OBJECT_NAMECHANGE | EVENT_SYSTEM_MOVESIZEEND
    ) {
        runtime.signal.request_immediate();
    } else {
        runtime.signal.request_debounced();
    }
}

fn install_window_event_hooks() -> Vec<HWINEVENTHOOK> {
    let event_ranges = [
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_OBJECT_REORDER, EVENT_OBJECT_REORDER),
        (EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
    ];
    event_ranges
        .into_iter()
        .filter_map(|(event_min, event_max)| {
            // SAFETY: callback has process lifetime and no module handle is required for an
            // out-of-context hook. The returned handle is released by the event thread.
            let hook = unsafe {
                SetWinEventHook(
                    event_min,
                    event_max,
                    None,
                    Some(window_event_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            (!hook.is_invalid()).then_some(hook)
        })
        .collect()
}

const ACTIVE_RECONCILIATION_INTERVAL: Duration = Duration::from_millis(750);
const INACTIVE_RECONCILIATION_INTERVAL: Duration = Duration::from_millis(1_500);
const EVENT_DEBOUNCE_INTERVAL: Duration = Duration::from_millis(120);

/// Maintains the immutable input snapshot away from the low-level hook. UIA providers used by
/// Weixin do not consistently emit structure/focus events, so a small adaptive reconciliation
/// loop remains as a watchdog. The loop is intentionally much less frequent than the old 75 ms
/// full-tree scan, and each refresh now uses property-filtered, anchored queries.
pub struct WindowsContextMonitor {
    provider: Arc<WindowsContextProvider>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    signal: Arc<RefreshSignal>,
    event_stopping: Arc<AtomicBool>,
    event_thread_id: Option<u32>,
    event_worker: Option<JoinHandle<()>>,
}

impl WindowsContextMonitor {
    pub fn new(provider: Arc<WindowsContextProvider>) -> Self {
        let signal = provider.refresh_signal();
        Self {
            provider,
            stopping: Arc::new(AtomicBool::new(false)),
            worker: None,
            signal,
            event_stopping: Arc::new(AtomicBool::new(false)),
            event_thread_id: None,
            event_worker: None,
        }
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        self.stopping.store(false, Ordering::Release);
        let provider = Arc::clone(&self.provider);
        let stopping = Arc::clone(&self.stopping);
        let signal = Arc::clone(&self.signal);
        self.worker = Some(
            thread::Builder::new()
                .name("wsg-context-monitor".to_owned())
                .spawn(move || {
                    let mut force_refresh = true;
                    while !stopping.load(Ordering::Acquire) {
                        if !provider.observation_enabled() {
                            let _ = signal.wait(INACTIVE_RECONCILIATION_INTERVAL);
                            force_refresh = true;
                            continue;
                        }
                        let context = provider.refresh_foreground_for_monitor(force_refresh);
                        force_refresh = false;
                        let interval = if context.is_trusted_weixin {
                            ACTIVE_RECONCILIATION_INTERVAL
                        } else {
                            INACTIVE_RECONCILIATION_INTERVAL
                        };
                        match signal.wait(interval) {
                            RefreshRequest::Immediate => force_refresh = true,
                            RefreshRequest::Debounced => {
                                // A drag/reflow can produce a burst of WinEvent notifications.
                                // Extend the quiet window only for more layout events; once the
                                // burst ends, the next context scan bypasses the cooldown.
                                while signal.wait(EVENT_DEBOUNCE_INTERVAL)
                                    == RefreshRequest::Debounced
                                {}
                                force_refresh = true;
                            }
                            RefreshRequest::TimedOut => {}
                        }
                    }
                })
                .map_err(|error| {
                    PlatformError::new("context-monitor-start-failed", error.to_string())
                })?,
        );

        self.start_event_listener();
        Ok(())
    }

    fn start_event_listener(&mut self) {
        if self.event_worker.is_some() {
            return;
        }

        let runtime = Arc::new(WindowEventRuntime {
            signal: Arc::clone(&self.signal),
            observation_enabled: Arc::clone(&self.provider.observation_enabled),
            observed_window: self.provider.observed_window(),
            layout_dirty: self.provider.layout_dirty(),
        });
        *lock_unpoisoned(active_window_event_runtime()) = Some(Arc::downgrade(&runtime));

        let event_stopping = Arc::new(AtomicBool::new(false));
        let event_stopping_for_thread = Arc::clone(&event_stopping);
        let runtime_for_thread = Arc::clone(&runtime);
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let event_worker = thread::Builder::new()
            .name("wsg-window-events".to_owned())
            .spawn(move || {
                let _runtime = runtime_for_thread;
                let mut message = MSG::default();
                // SAFETY: touching the queue before installing an out-of-context WinEvent hook
                // ensures Windows can dispatch callbacks on this dedicated thread.
                unsafe {
                    let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                }
                let thread_id = unsafe { GetCurrentThreadId() };
                let hooks = install_window_event_hooks();
                if hooks.is_empty() {
                    let _ = init_tx.send(None);
                    *lock_unpoisoned(active_window_event_runtime()) = None;
                    return;
                }
                let _ = init_tx.send(Some(thread_id));

                while !event_stopping_for_thread.load(Ordering::Acquire) {
                    // SAFETY: message points to a stack-owned MSG and the queue belongs to this
                    // event thread. WM_QUIT terminates the loop without dispatching user input.
                    let received = unsafe { GetMessageW(&mut message, None, 0, 0) };
                    if !received.as_bool() {
                        break;
                    }
                    // SAFETY: the message was retrieved from this thread's queue.
                    unsafe {
                        let _ = TranslateMessage(&message);
                        let _ = DispatchMessageW(&message);
                    }
                }

                for hook in hooks {
                    // SAFETY: each handle was returned by SetWinEventHook on this thread.
                    unsafe {
                        let _ = UnhookWinEvent(hook);
                    }
                }
                *lock_unpoisoned(active_window_event_runtime()) = None;
            });

        let Ok(event_worker) = event_worker else {
            *lock_unpoisoned(active_window_event_runtime()) = None;
            return;
        };
        let event_thread_id = init_rx.recv().ok().flatten();
        if event_thread_id.is_none() {
            *lock_unpoisoned(active_window_event_runtime()) = None;
            let _ = event_worker.join();
            return;
        }
        self.event_stopping = event_stopping;
        self.event_thread_id = event_thread_id;
        self.event_worker = Some(event_worker);
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.signal.request_immediate();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.event_stopping.store(true, Ordering::Release);
        if let Some(thread_id) = self.event_thread_id.take() {
            // SAFETY: this ID was returned by the event thread itself and WM_QUIT only wakes its
            // private message queue so it can unhook its WinEvent handles.
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(worker) = self.event_worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for WindowsContextMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn diagnostics_from_trust(
    observation_id: u64,
    window_handle: isize,
    trust: &ProcessTrust,
    inspect_send_button: bool,
) -> ContextDiagnostics {
    ContextDiagnostics {
        observation_id,
        window_handle,
        process_id: trust.process_id,
        process_path_available: !trust.process_path.is_empty(),
        is_trusted_weixin: trust.is_trusted_weixin,
        requires_elevation: trust.requires_elevation,
        send_button_inspected: inspect_send_button,
        ..ContextDiagnostics::default()
    }
}

fn record_diagnostic_error(diagnostics: &mut ContextDiagnostics, error: &PlatformError) {
    if diagnostics.error_code.is_none() {
        diagnostics.error_code = Some(diagnostic_error_code(error));
    }
    diagnostics.query_status = "partial-query-failed".to_owned();
}

fn diagnostic_error_code(error: &PlatformError) -> String {
    let platform_code = error.message.split_whitespace().find_map(|token| {
        let token = token
            .trim_matches(|character: char| !character.is_ascii_hexdigit() && character != 'x');
        let value = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))?;
        (value.len() >= 8
            && value
                .chars()
                .take(8)
                .all(|character| character.is_ascii_hexdigit()))
        .then(|| format!("0x{}", &value[..8]))
    });
    platform_code.map_or_else(
        || error.code.to_owned(),
        |platform_code| format!("{}:{platform_code}", error.code),
    )
}

fn diagnostic_query<T>(
    result: PlatformResult<T>,
    diagnostics: &mut ContextDiagnostics,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            record_diagnostic_error(diagnostics, &error);
            None
        }
    }
}

fn query_anchor_with_diagnostics(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
    suffix: &str,
    diagnostics: &mut ContextDiagnostics,
) -> (
    Option<IUIAutomationElement>,
    String,
    Option<usize>,
    Option<String>,
) {
    let (fast_match, fast_error) = match find_property_suffix(automation, root, property_id, suffix)
    {
        Ok(element) => (element, None),
        Err(error) => {
            let code = diagnostic_error_code(&error);
            record_diagnostic_error(diagnostics, &error);
            (None, Some(code))
        }
    };
    if fast_match.is_some() {
        return (fast_match, "found-fast".to_owned(), None, fast_error);
    }

    match find_property_suffix_all(automation, root, property_id, suffix) {
        Ok(matches) => {
            let status = match (fast_error.is_some(), matches.is_empty()) {
                (false, true) => "not-found",
                (false, false) => "found-only-by-full-probe",
                (true, true) => "fast-query-failed-full-probe-empty",
                (true, false) => "fast-query-failed-full-probe-found",
            };
            (None, status.to_owned(), Some(matches.len()), fast_error)
        }
        Err(error) => {
            let code = diagnostic_error_code(&error);
            record_diagnostic_error(diagnostics, &error);
            (None, "query-failed".to_owned(), None, Some(code))
        }
    }
}

fn capture_tree_diagnostics(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    diagnostics: &mut ContextDiagnostics,
) {
    const MAX_SAMPLED_ELEMENTS: usize = 512;

    let condition = match unsafe { automation.CreateTrueCondition() } {
        Ok(condition) => condition,
        Err(error) => {
            let error = PlatformError::new("uia-tree-condition-failed", error.to_string());
            diagnostics.tree_query_status = "condition-failed".to_owned();
            diagnostics.tree_error_code = Some(diagnostic_error_code(&error));
            record_diagnostic_error(diagnostics, &error);
            return;
        }
    };
    let elements = match unsafe { root.FindAll(TreeScope_Descendants, &condition) } {
        Ok(elements) => elements,
        Err(error) => {
            let error = PlatformError::new("uia-tree-query-failed", error.to_string());
            diagnostics.tree_query_status = "query-failed".to_owned();
            diagnostics.tree_error_code = Some(diagnostic_error_code(&error));
            record_diagnostic_error(diagnostics, &error);
            return;
        }
    };
    let length = match unsafe { elements.Length() } {
        Ok(length) if length >= 0 => length as usize,
        Ok(_) => 0,
        Err(error) => {
            let error = PlatformError::new("uia-tree-length-failed", error.to_string());
            diagnostics.tree_query_status = "length-failed".to_owned();
            diagnostics.tree_error_code = Some(diagnostic_error_code(&error));
            record_diagnostic_error(diagnostics, &error);
            return;
        }
    };

    let sampled_count = length.min(MAX_SAMPLED_ELEMENTS);
    let mut control_type_counts = BTreeMap::<i32, usize>::new();
    let mut automation_id_readable_count = 0usize;
    let mut automation_id_nonempty_count = 0usize;
    let mut class_name_readable_count = 0usize;
    let mut class_name_nonempty_count = 0usize;
    let mut property_read_failure_count = 0usize;
    let mut first_property_error = None;

    for index in 0..sampled_count {
        let element = match unsafe { elements.GetElement(index as i32) } {
            Ok(element) => element,
            Err(error) => {
                property_read_failure_count = property_read_failure_count.saturating_add(1);
                first_property_error.get_or_insert_with(|| {
                    PlatformError::new("uia-tree-element-read-failed", error.to_string())
                });
                continue;
            }
        };
        match unsafe { element.CurrentControlType() } {
            Ok(control_type) => {
                let count = control_type_counts.entry(control_type.0).or_default();
                *count = count.saturating_add(1);
            }
            Err(error) => {
                property_read_failure_count = property_read_failure_count.saturating_add(1);
                first_property_error.get_or_insert_with(|| {
                    PlatformError::new("uia-tree-control-type-read-failed", error.to_string())
                });
            }
        }
        match unsafe { element.CurrentAutomationId() } {
            Ok(value) => {
                automation_id_readable_count = automation_id_readable_count.saturating_add(1);
                if !value.to_string().is_empty() {
                    automation_id_nonempty_count = automation_id_nonempty_count.saturating_add(1);
                }
            }
            Err(error) => {
                property_read_failure_count = property_read_failure_count.saturating_add(1);
                first_property_error.get_or_insert_with(|| {
                    PlatformError::new("uia-tree-automation-id-read-failed", error.to_string())
                });
            }
        }
        match unsafe { element.CurrentClassName() } {
            Ok(value) => {
                class_name_readable_count = class_name_readable_count.saturating_add(1);
                if !value.to_string().is_empty() {
                    class_name_nonempty_count = class_name_nonempty_count.saturating_add(1);
                }
            }
            Err(error) => {
                property_read_failure_count = property_read_failure_count.saturating_add(1);
                first_property_error.get_or_insert_with(|| {
                    PlatformError::new("uia-tree-class-name-read-failed", error.to_string())
                });
            }
        }
    }

    diagnostics.tree_query_status = if property_read_failure_count == 0 {
        "success"
    } else {
        "partial-property-read-failure"
    }
    .to_owned();
    diagnostics.tree_descendant_count = Some(length);
    diagnostics.tree_sampled_count = Some(sampled_count);
    diagnostics.tree_sample_truncated = length > sampled_count;
    diagnostics.tree_control_type_counts = Some(
        control_type_counts
            .into_iter()
            .take(16)
            .map(|(control_type, count)| format!("{control_type}:{count}"))
            .collect::<Vec<_>>()
            .join(","),
    );
    diagnostics.tree_automation_id_readable_count = Some(automation_id_readable_count);
    diagnostics.tree_automation_id_nonempty_count = Some(automation_id_nonempty_count);
    diagnostics.tree_class_name_readable_count = Some(class_name_readable_count);
    diagnostics.tree_class_name_nonempty_count = Some(class_name_nonempty_count);
    diagnostics.tree_property_read_failure_count = Some(property_read_failure_count);
    if let Some(error) = first_property_error {
        diagnostics.tree_error_code = Some(diagnostic_error_code(&error));
        record_diagnostic_error(diagnostics, &error);
    }
}

fn inspect_supported_weixin_window(
    observation_id: u64,
    window_handle: isize,
    trust: ProcessTrust,
    observed_at: SystemTime,
    inspect_send_button: bool,
) -> PlatformResult<WindowInspection> {
    with_automation(|automation| {
        let root = element_from_handle(automation, window_handle)?;
        let mut diagnostics =
            diagnostics_from_trust(observation_id, window_handle, &trust, inspect_send_button);
        diagnostics.query_status = "success".to_owned();
        diagnostics.root_available = true;
        diagnostics.root_class_name = read_diagnostic_class_name(&root);
        diagnostics.root_control_type = unsafe { root.CurrentControlType() }
            .ok()
            .map(|control_type| control_type.0);
        diagnostics.provider_kind = classify_uia_provider(&root);
        diagnostics.root_child_count =
            diagnostic_query(root_direct_child_count(automation, &root), &mut diagnostics);

        let (editor, editor_query_status, editor_candidate_count, editor_query_error_code) =
            query_anchor_with_diagnostics(
                automation,
                &root,
                UIA_AutomationIdPropertyId,
                INPUT_AUTOMATION_ID,
                &mut diagnostics,
            );
        diagnostics.editor_query_status = editor_query_status;
        diagnostics.editor_candidate_count = editor_candidate_count;
        diagnostics.editor_query_error_code = editor_query_error_code;
        let (
            title_element,
            chat_title_query_status,
            chat_title_candidate_count,
            chat_title_query_error_code,
        ) = query_anchor_with_diagnostics(
            automation,
            &root,
            UIA_AutomationIdPropertyId,
            CHAT_NAME_AUTOMATION_ID,
            &mut diagnostics,
        );
        diagnostics.chat_title_query_status = chat_title_query_status;
        diagnostics.chat_title_candidate_count = chat_title_candidate_count;
        diagnostics.chat_title_query_error_code = chat_title_query_error_code;
        // The editor and current-title label are stable anchors shared by both layouts. Their
        // nearest common ancestor is the chat column, so toolbar/button queries below never enter
        // a sibling web-view branch in three-pane Weixin.
        let chat_branch = editor.as_ref().and_then(|editor| {
            title_element
                .as_ref()
                .and_then(|title| find_nearest_common_ancestor(automation, &root, editor, title))
        });
        let chat_root = chat_branch.as_ref().unwrap_or(&root);
        // Some older providers omit the title automation id but still expose the stable group
        // title-bar class. Keep that compatibility signal inside the selected branch.
        let (
            group_title,
            group_title_query_status,
            group_title_candidate_count,
            group_title_query_error_code,
        ) = query_anchor_with_diagnostics(
            automation,
            chat_root,
            UIA_ClassNamePropertyId,
            GROUP_TITLE_CLASS_SUFFIX,
            &mut diagnostics,
        );
        diagnostics.group_title_query_status = group_title_query_status;
        diagnostics.group_title_candidate_count = group_title_candidate_count;
        diagnostics.group_title_query_error_code = group_title_query_error_code;
        let chat_title = title_element.as_ref().and_then(read_name);
        let is_group_chat = group_title.is_some()
            || title_element.as_ref().is_some_and(|title| {
                ancestor_has_class_suffix(automation, &root, title, GROUP_TITLE_CLASS_SUFFIX)
            });
        let target_kind = if is_group_chat {
            Some(ChatTargetKind::Group)
        } else if chat_title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty())
        {
            Some(ChatTargetKind::Contact)
        } else {
            None
        };
        let is_message_editor_focused = editor
            .as_ref()
            .map(|editor| is_editor_focused(automation, editor, &root))
            .unwrap_or(false);
        diagnostics.editor_found = editor.is_some();
        diagnostics.chat_title_element_found = title_element.is_some();
        diagnostics.chat_title_readable = chat_title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty());
        diagnostics.group_title_found = group_title.is_some();
        diagnostics.chat_branch_found = chat_branch.is_some();
        diagnostics.editor_focused = is_message_editor_focused;
        if editor.is_none() || title_element.is_none() || target_kind.is_none() {
            capture_tree_diagnostics(automation, &root, &mut diagnostics);
        }

        let editor_bounds = editor.as_ref().and_then(|editor| {
            // SAFETY: the editor proxy is live for the duration of this synchronous read.
            unsafe { editor.CurrentBoundingRectangle() }
                .ok()
                .and_then(ScreenRect::from_windows_rect)
        });
        let (send_button, send_button_inspection) = if inspect_send_button {
            find_send_button_snapshot(
                automation,
                chat_root,
                window_handle,
                observed_at,
                editor_bounds,
            )
        } else {
            (
                SendButtonSnapshot::not_observed(window_handle, Some(observed_at)),
                SendButtonInspection::default(),
            )
        };
        diagnostics.toolbar_count = send_button_inspection.toolbar_count;
        diagnostics.send_button_candidate_count = send_button_inspection.candidate_count;
        if diagnostics.error_code.is_none() {
            diagnostics.error_code = send_button_inspection.error_code;
        }
        if diagnostics.error_code.is_some() {
            diagnostics.query_status = "partial-query-failed".to_owned();
        }
        diagnostics.send_button_state =
            send_button_snapshot_state_name(send_button.state).to_owned();
        if diagnostics.tree_query_status == "not-queried"
            && inspect_send_button
            && matches!(
                send_button.state,
                SendButtonSnapshotState::QueryFailed
                    | SendButtonSnapshotState::NotFound
                    | SendButtonSnapshotState::StateUnavailable
                    | SendButtonSnapshotState::GeometryUnavailable
            )
        {
            capture_tree_diagnostics(automation, &root, &mut diagnostics);
        }
        diagnostics.scan_duration_milliseconds =
            observed_at.elapsed().unwrap_or_default().as_millis();

        Ok(WindowInspection {
            context: ChatContext {
                window_handle,
                process_id: trust.process_id,
                process_path: trust.process_path,
                is_trusted_weixin: true,
                requires_elevation: trust.requires_elevation,
                is_compatibility_available: editor.is_some() && target_kind.is_some(),
                is_message_editor_focused,
                is_group_chat: target_kind == Some(ChatTargetKind::Group),
                is_contact_chat: target_kind == Some(ChatTargetKind::Contact),
                chat_title,
                generation: 0,
                observed_at: Some(observed_at),
            },
            send_button,
            diagnostics,
        })
    })
}

fn root_direct_child_count(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> PlatformResult<i32> {
    let condition = unsafe { automation.CreateTrueCondition() }
        .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
    let children = unsafe { root.FindAll(TreeScope_Children, &condition) }
        .map_err(|error| PlatformError::new("uia-root-children-query-failed", error.to_string()))?;
    unsafe { children.Length() }
        .map_err(|error| PlatformError::new("uia-root-children-query-failed", error.to_string()))
}

fn read_diagnostic_class_name(root: &IUIAutomationElement) -> Option<String> {
    let value = unsafe { root.CurrentClassName() }.ok()?.to_string();
    let normalized = value.replace(['\r', '\n'], " ");
    (!normalized.is_empty()).then(|| normalized.chars().take(128).collect())
}

fn classify_uia_provider(root: &IUIAutomationElement) -> String {
    let Ok(description) = (unsafe { root.CurrentProviderDescription() }) else {
        return "unavailable".to_owned();
    };
    let description = description.to_string().to_ascii_lowercase();
    if description.contains("qt") {
        "qt".to_owned()
    } else if description.contains("msaa") {
        "msaa-proxy".to_owned()
    } else if description.contains("native") || description.contains("hwnd") {
        "native-window-proxy".to_owned()
    } else {
        "other".to_owned()
    }
}

fn send_button_snapshot_state_name(state: SendButtonSnapshotState) -> &'static str {
    match state {
        SendButtonSnapshotState::NotObserved => "not-observed",
        SendButtonSnapshotState::QueryFailed => "query-failed",
        SendButtonSnapshotState::NotFound => "not-found",
        SendButtonSnapshotState::Disabled => "disabled",
        SendButtonSnapshotState::Enabled => "enabled",
        SendButtonSnapshotState::StateUnavailable => "state-unavailable",
        SendButtonSnapshotState::GeometryUnavailable => "geometry-unavailable",
    }
}

fn context_from_trust(
    window_handle: isize,
    trust: ProcessTrust,
    observed_at: SystemTime,
) -> ChatContext {
    ChatContext {
        window_handle,
        process_id: trust.process_id,
        process_path: trust.process_path,
        is_trusted_weixin: trust.is_trusted_weixin,
        requires_elevation: trust.requires_elevation,
        observed_at: Some(observed_at),
        ..ChatContext::default()
    }
}

fn with_automation<T>(
    operation: impl FnOnce(&IUIAutomation) -> PlatformResult<T>,
) -> PlatformResult<T> {
    let apartment = ComApartment::initialize()?;
    // SAFETY: COM is initialized for this thread or already initialized by its caller. The
    // returned automation proxy remains local to this closure and is dropped before the apartment.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| PlatformError::new("uia-create-failed", error.to_string()))?;
    let result = operation(&automation);
    drop(automation);
    drop(apartment);
    result
}

fn element_from_handle(
    automation: &IUIAutomation,
    window_handle: isize,
) -> PlatformResult<IUIAutomationElement> {
    // SAFETY: HWND is a copied native value and UI Automation does not retain a Rust borrow.
    unsafe { automation.ElementFromHandle(HWND(window_handle as _)) }
        .map_err(|error| PlatformError::new("uia-window-unavailable", error.to_string()))
}

fn control_view_ancestor_chain(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    element: &IUIAutomationElement,
) -> Option<Vec<IUIAutomationElement>> {
    let walker = unsafe { automation.ControlViewWalker() }.ok()?;
    let mut current = element.clone();
    let mut chain = vec![current.clone()];
    for _ in 0..32 {
        if elements_are_equal(automation, &current, root) {
            return Some(chain);
        }
        let parent = unsafe { walker.GetParentElement(&current) }.ok()?;
        chain.push(parent.clone());
        current = parent;
    }
    None
}

fn find_nearest_common_ancestor(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    first: &IUIAutomationElement,
    second: &IUIAutomationElement,
) -> Option<IUIAutomationElement> {
    let mut first_chain = control_view_ancestor_chain(automation, root, first)?;
    let mut second_chain = control_view_ancestor_chain(automation, root, second)?;
    first_chain.reverse();
    second_chain.reverse();

    let mut common = None;
    for (first_ancestor, second_ancestor) in first_chain.iter().zip(second_chain.iter()) {
        if !elements_are_equal(automation, first_ancestor, second_ancestor) {
            break;
        }
        common = Some(first_ancestor.clone());
    }
    common
}

fn ancestor_has_class_suffix(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    element: &IUIAutomationElement,
    suffix: &str,
) -> bool {
    control_view_ancestor_chain(automation, root, element).is_some_and(|chain| {
        chain.into_iter().any(|ancestor| {
            unsafe { ancestor.CurrentClassName() }
                .map(|class_name| class_name.to_string().ends_with(suffix))
                .unwrap_or(false)
        })
    })
}

fn find_by_automation_id_suffix(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    suffix: &str,
) -> PlatformResult<Option<IUIAutomationElement>> {
    find_property_suffix(automation, root, UIA_AutomationIdPropertyId, suffix)
}

/// UIA's condition engine can locate an element without materializing every descendant and then
/// issuing a separate COM property call for each one. Substring matching preserves compatibility
/// with Weixin's version-specific prefixes; an exact-match retry covers providers that do not
/// implement substring filtering correctly.
fn find_property_suffix(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
    suffix: &str,
) -> PlatformResult<Option<IUIAutomationElement>> {
    // `FindFirst` stops the provider traversal as soon as the first matching element is found.
    // This matters for the editor/title anchors because a three-pane web view can contain a large
    // number of unrelated descendants with the same generic property shape.
    let substring_value = VARIANT::from(suffix);
    if let Ok(condition) = unsafe {
        automation.CreatePropertyConditionEx(
            property_id,
            &substring_value,
            PropertyConditionFlags_MatchSubstring,
        )
    } && let Ok(element) = unsafe { root.FindFirst(TreeScope_Descendants, &condition) }
    {
        return Ok(Some(element));
    }

    let exact_value = VARIANT::from(suffix);
    let exact = unsafe { automation.CreatePropertyCondition(property_id, &exact_value) }
        .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
    Ok(unsafe { root.FindFirst(TreeScope_Descendants, &exact) }.ok())
}

fn find_property_suffix_all(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
    suffix: &str,
) -> PlatformResult<Vec<IUIAutomationElement>> {
    // MatchSubstring includes exact values and avoids the common two-traversal path. The exact
    // query is only a compatibility fallback for providers that return no result for substring
    // conditions despite exposing an exact property value.
    let substring_value = VARIANT::from(suffix);
    let substring = match unsafe {
        automation.CreatePropertyConditionEx(
            property_id,
            &substring_value,
            PropertyConditionFlags_MatchSubstring,
        )
    } {
        Ok(condition) => condition,
        Err(_) => {
            let exact_value = VARIANT::from(suffix);
            let exact = unsafe { automation.CreatePropertyCondition(property_id, &exact_value) }
                .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
            return find_all_with_condition(root, &exact);
        }
    };
    let matches = match find_all_with_condition(root, &substring) {
        Ok(matches) => matches,
        Err(_) => {
            // Some UIA providers accept CreatePropertyConditionEx but reject substring
            // evaluation during FindAll. Retry with the exact condition before declaring the
            // whole foreground observation unavailable.
            let exact_value = VARIANT::from(suffix);
            let exact = unsafe { automation.CreatePropertyCondition(property_id, &exact_value) }
                .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
            return find_all_with_condition(root, &exact);
        }
    };
    if !matches.is_empty() {
        return Ok(matches);
    }

    let exact_value = VARIANT::from(suffix);
    let exact = unsafe { automation.CreatePropertyCondition(property_id, &exact_value) }
        .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
    find_all_with_condition(root, &exact)
}

fn find_all_with_condition(
    root: &IUIAutomationElement,
    condition: &windows::Win32::UI::Accessibility::IUIAutomationCondition,
) -> PlatformResult<Vec<IUIAutomationElement>> {
    // FindAll with a property condition lets the provider filter before returning elements while
    // preserving an explicit `None` result (FindFirst's generated binding represents a null
    // result as an error on some UIA providers).
    let elements = unsafe { root.FindAll(TreeScope_Descendants, condition) }
        .map_err(|error| PlatformError::new("uia-query-failed", error.to_string()))?;
    let length = unsafe { elements.Length() }
        .map_err(|error| PlatformError::new("uia-query-failed", error.to_string()))?;
    let mut result = Vec::with_capacity(length as usize);
    for index in 0..length {
        if let Ok(element) = unsafe { elements.GetElement(index) } {
            result.push(element);
        }
    }
    Ok(result)
}

fn find_by_control_type(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    control_type: i32,
) -> PlatformResult<Vec<IUIAutomationElement>> {
    let value = VARIANT::from(control_type);
    let condition =
        unsafe { automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &value) }
            .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
    // The returned collection is scoped to the toolbar (normally only a handful of controls),
    // unlike the old root-wide TrueCondition scan.
    find_all_with_condition(root, &condition)
}

fn find_named_send_button(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> PlatformResult<Vec<IUIAutomationElement>> {
    let buttons = match find_by_control_type(automation, root, UIA_ButtonControlTypeId.0) {
        Ok(buttons) => buttons,
        Err(_) => return find_named_send_button_by_name(automation, root),
    };
    let matches: Vec<_> = buttons.into_iter().filter(is_named_send_button).collect();
    if !matches.is_empty() {
        return Ok(matches);
    }

    // Weixin's custom XOutlineButton sometimes exposes only its inner XTextView as the named
    // control. Keep the search anchored to the same toolbar and accept an exact "发送" name in
    // that fallback; the toolbar bounds still constrain the eventual click candidate.
    find_named_send_button_by_name(automation, root)
}

fn find_named_send_button_by_name(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> PlatformResult<Vec<IUIAutomationElement>> {
    let named = find_property_suffix_all(automation, root, UIA_NamePropertyId, "发送")?;
    Ok(named
        .into_iter()
        .filter(|element| read_name(element).as_deref() == Some("发送"))
        .filter_map(|element| {
            if is_named_send_button(&element) {
                Some(element)
            } else {
                find_button_ancestor(automation, &element)
            }
        })
        .collect())
}

fn find_button_ancestor(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
) -> Option<IUIAutomationElement> {
    // A custom Weixin button can surface its clickable label as XTextView instead of a Button.
    // Limit that fallback to a short control-view ancestor chain that contains a Button, so a
    // random "发送" text in the embedded right-side browser cannot become an input candidate.
    let Ok(walker) = (unsafe { automation.ControlViewWalker() }) else {
        return None;
    };
    let mut current = element.clone();
    for _ in 0..8 {
        let Ok(parent) = (unsafe { walker.GetParentElement(&current) }) else {
            return None;
        };
        let is_button = unsafe { parent.CurrentControlType() }
            .map(|control_type| control_type == UIA_ButtonControlTypeId)
            .unwrap_or(false);
        if is_button {
            return Some(parent);
        }
        current = parent;
    }
    None
}

fn find_send_button_snapshot(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    window_handle: isize,
    observed_at: SystemTime,
    editor_bounds: Option<ScreenRect>,
) -> (SendButtonSnapshot, SendButtonInspection) {
    let mut inspection = SendButtonInspection::default();
    let base = SendButtonSnapshot {
        window_handle,
        observed_at: Some(observed_at),
        state: SendButtonSnapshotState::NotFound,
        bounds: None,
        window_bounds: current_window_bounds(window_handle),
        window_dpi: current_window_dpi(window_handle),
        candidate_bounds: None,
        alternate_candidate_bounds: None,
    };

    let toolbars = match find_property_suffix_all(
        automation,
        root,
        UIA_AutomationIdPropertyId,
        SEND_TOOLBAR_AUTOMATION_ID,
    ) {
        Ok(toolbars) => toolbars,
        Err(_) => {
            inspection.error_code = Some("uia-send-toolbar-query-failed".to_owned());
            return (
                SendButtonSnapshot {
                    state: SendButtonSnapshotState::QueryFailed,
                    candidate_bounds: conservative_send_candidate_bounds(editor_bounds, &[]),
                    ..base
                },
                inspection,
            );
        }
    };
    inspection.toolbar_count = Some(toolbars.len());

    let mut candidates = Vec::new();
    let mut toolbar_bounds = Vec::new();
    for toolbar in &toolbars {
        let bounds = element_bounds(toolbar);
        if let Some(bounds) = bounds {
            toolbar_bounds.push(bounds);
        }
        match find_named_send_button(automation, toolbar) {
            Ok(buttons) => candidates.extend(buttons.into_iter().map(|button| (button, bounds))),
            Err(_) => {
                inspection.error_code = Some("uia-send-button-query-failed".to_owned());
                return (
                    SendButtonSnapshot {
                        state: SendButtonSnapshotState::QueryFailed,
                        candidate_bounds: conservative_send_candidate_bounds(
                            editor_bounds,
                            &toolbar_bounds,
                        ),
                        ..base
                    },
                    inspection,
                );
            }
        }
    }
    // A known toolbar gives us a narrow region. If it is transiently absent, retain the legacy
    // root query as a compatibility fallback, but do not scan every property on every descendant.
    if candidates.is_empty() && toolbars.is_empty() {
        match find_named_send_button(automation, root) {
            Ok(buttons) => candidates.extend(buttons.into_iter().map(|button| (button, None))),
            Err(_) => {
                inspection.error_code = Some("uia-send-button-root-query-failed".to_owned());
                return (
                    SendButtonSnapshot {
                        state: SendButtonSnapshotState::QueryFailed,
                        candidate_bounds: conservative_send_candidate_bounds(editor_bounds, &[]),
                        ..base
                    },
                    inspection,
                );
            }
        }
    }
    inspection.candidate_count = Some(candidates.len());
    let fallback_bounds = conservative_send_candidate_bounds(editor_bounds, &toolbar_bounds);
    let Some((button, toolbar_bounds)) = choose_send_button_candidate(candidates, editor_bounds)
    else {
        return (
            SendButtonSnapshot {
                candidate_bounds: fallback_bounds,
                ..base
            },
            inspection,
        );
    };

    // SAFETY: property reads are synchronous calls on a live UI Automation element proxy.
    let enabled = match unsafe { button.CurrentIsEnabled() } {
        Ok(value) => value.as_bool(),
        Err(_) => {
            inspection.error_code = Some("uia-send-button-state-failed".to_owned());
            return (
                SendButtonSnapshot {
                    state: SendButtonSnapshotState::StateUnavailable,
                    candidate_bounds: conservative_send_candidate(toolbar_bounds, editor_bounds),
                    ..base
                },
                inspection,
            );
        }
    };
    // SAFETY: the bounding rectangle is copied immediately and no COM proxy escapes this scope.
    let bounds = element_bounds(&button);
    // Some Weixin builds expose the outer custom button as disabled while its inner text control
    // remains enabled when a draft is present. Treat that structurally verified state as
    // actionable; `IsEnabled` alone is not a reliable clickability signal for this control.
    let actionable = enabled || has_enabled_send_descendant(automation, &button);
    let state = if actionable {
        if bounds.is_some() {
            SendButtonSnapshotState::Enabled
        } else {
            SendButtonSnapshotState::GeometryUnavailable
        }
    } else {
        SendButtonSnapshotState::Disabled
    };
    (
        SendButtonSnapshot {
            state,
            bounds,
            candidate_bounds: conservative_send_candidate(toolbar_bounds, editor_bounds),
            ..base
        },
        inspection,
    )
}

fn choose_send_button_candidate(
    candidates: Vec<(IUIAutomationElement, Option<ScreenRect>)>,
    editor_bounds: Option<ScreenRect>,
) -> Option<(IUIAutomationElement, Option<ScreenRect>)> {
    candidates
        .into_iter()
        .min_by_key(|(button, toolbar_bounds)| {
            let button_bounds = element_bounds(button);
            let region = button_bounds.or(*toolbar_bounds);
            send_button_candidate_score(editor_bounds, region)
        })
}

/// Prefer the toolbar directly beneath the editor and closest to its right edge. This works for
/// both the ordinary two-column layout and a three-column layout where an embedded browser adds
/// unrelated controls to the right, without relying on the global UIA traversal order.
fn send_button_candidate_score(
    editor_bounds: Option<ScreenRect>,
    candidate_bounds: Option<ScreenRect>,
) -> (u8, i64, i64, i64) {
    let Some(editor) = editor_bounds else {
        return (1, 0, 0, 0);
    };
    let Some(candidate) = candidate_bounds else {
        return (2, i64::MAX, i64::MAX, i64::MAX);
    };

    let is_below_editor = candidate.top >= editor.top;
    let raw_vertical_gap = (candidate.top - editor.bottom).max(0);
    let is_near_editor = candidate.top <= editor.bottom + INPUT_CANDIDATE_MAX_VERTICAL_GAP;
    let vertical_gap = i64::from(raw_vertical_gap);
    let right_gap = i64::from((editor.right - candidate.right).abs());
    let overlap_penalty = if candidate.left < editor.left || candidate.right > editor.right {
        1
    } else {
        0
    };
    (
        if is_below_editor && is_near_editor {
            0
        } else {
            1
        },
        i64::from(overlap_penalty),
        vertical_gap,
        right_gap,
    )
}

fn closest_bounds(reference: Option<ScreenRect>, bounds: &[ScreenRect]) -> Option<ScreenRect> {
    bounds
        .iter()
        .copied()
        .min_by_key(|bounds| send_button_candidate_score(reference, Some(*bounds)))
}

/// Derives a small, right-aligned candidate area for the short interval in which UIA has exposed
/// the editor/toolbar but not the button properties. The area is deliberately much narrower than
/// the whole toolbar so emoji, attachment, and voice controls continue to pass through normally.
fn conservative_send_candidate_bounds(
    editor_bounds: Option<ScreenRect>,
    toolbar_bounds: &[ScreenRect],
) -> Option<ScreenRect> {
    let anchor = closest_bounds(editor_bounds, toolbar_bounds)?;
    conservative_send_candidate(Some(anchor), editor_bounds)
}

fn conservative_send_candidate(
    anchor: Option<ScreenRect>,
    editor_bounds: Option<ScreenRect>,
) -> Option<ScreenRect> {
    let anchor = anchor?;
    let width = (anchor.right - anchor.left).saturating_div(4).clamp(
        FALLBACK_SEND_CANDIDATE_MIN_WIDTH,
        FALLBACK_SEND_CANDIDATE_MAX_WIDTH,
    );
    let height = (anchor.bottom - anchor.top).saturating_div(2).clamp(
        FALLBACK_SEND_CANDIDATE_MIN_HEIGHT,
        FALLBACK_SEND_CANDIDATE_MAX_HEIGHT,
    );
    let right = editor_bounds.map_or(anchor.right, |editor| anchor.right.min(editor.right));
    let left = right
        .saturating_sub(width)
        .max(anchor.left)
        .max(editor_bounds.map_or(i32::MIN, |editor| editor.left));
    let bottom = anchor.bottom;
    let top = bottom.saturating_sub(height).max(anchor.top);
    (right > left && bottom > top).then_some(ScreenRect {
        left,
        top,
        right,
        bottom,
    })
}

fn element_bounds(element: &IUIAutomationElement) -> Option<ScreenRect> {
    // SAFETY: the rectangle is copied synchronously from a live UIA element proxy.
    unsafe { element.CurrentBoundingRectangle() }
        .ok()
        .and_then(ScreenRect::from_windows_rect)
}

fn has_enabled_send_descendant(automation: &IUIAutomation, button: &IUIAutomationElement) -> bool {
    let Ok(values) = find_property_suffix_all(automation, button, UIA_NamePropertyId, "发送")
    else {
        return false;
    };
    values.into_iter().any(|element| {
        read_name(&element).as_deref() == Some("发送")
            && unsafe { element.CurrentIsEnabled() }
                .map(|enabled| enabled.as_bool())
                .unwrap_or(false)
    })
}

fn read_name(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: property access is a synchronous COM call on a live element proxy.
    unsafe { element.CurrentName() }
        .ok()
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
}

fn is_named_send_button(element: &IUIAutomationElement) -> bool {
    // SAFETY: property access is a synchronous COM call on a live element proxy.
    let is_button = unsafe { element.CurrentControlType() }
        .map(|control_type| control_type == UIA_ButtonControlTypeId)
        .unwrap_or(false);
    is_button && read_name(element).as_deref() == Some("发送")
}

fn is_editor_focused(
    automation: &IUIAutomation,
    editor: &IUIAutomationElement,
    root: &IUIAutomationElement,
) -> bool {
    // SAFETY: the automation proxy is valid for the duration of these synchronous calls.
    let mut focused = match unsafe { automation.GetFocusedElement() } {
        Ok(element) => element,
        Err(_) => return false,
    };
    // SAFETY: the automation proxy is valid for the duration of this synchronous query.
    let walker = match unsafe { automation.ControlViewWalker() } {
        Ok(walker) => walker,
        Err(_) => return false,
    };

    for _ in 0..32 {
        if elements_are_equal(automation, &focused, editor) {
            return true;
        }
        if elements_are_equal(automation, &focused, root) {
            return false;
        }
        // SAFETY: focused is a live element proxy and the walker does not retain it.
        focused = match unsafe { walker.GetParentElement(&focused) } {
            Ok(parent) => parent,
            Err(_) => return false,
        };
    }
    false
}

fn elements_are_equal(
    automation: &IUIAutomation,
    left: &IUIAutomationElement,
    right: &IUIAutomationElement,
) -> bool {
    // SAFETY: all COM proxies are valid for this synchronous identity comparison.
    unsafe { automation.CompareElements(left, right) }
        .map(|value| value.as_bool())
        .unwrap_or(false)
}

fn read_draft_preview(editor: &IUIAutomationElement) -> Option<String> {
    // SAFETY: retrieving a supported value pattern is a synchronous query on a live element.
    if let Ok(pattern) =
        unsafe { editor.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
    {
        // SAFETY: the pattern proxy remains valid through this synchronous property read.
        if let Ok(value) = unsafe { pattern.CurrentValue() } {
            return normalize_draft_preview(value.to_string());
        }
    }
    // SAFETY: retrieving a supported text pattern is a synchronous query on a live element.
    if let Ok(pattern) =
        unsafe { editor.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
    {
        // SAFETY: both calls are synchronous and pattern/range proxies remain locally owned.
        if let Ok(range) = unsafe { pattern.DocumentRange() }
            && let Ok(value) = unsafe { range.GetText((DRAFT_PREVIEW_LIMIT + 1) as i32) }
        {
            return normalize_draft_preview(value.to_string());
        }
    }
    None
}

fn normalize_draft_preview(value: String) -> Option<String> {
    let value = value.replace('\0', "");
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut characters = trimmed.chars();
    let preview: String = characters.by_ref().take(DRAFT_PREVIEW_LIMIT).collect();
    if characters.next().is_some() {
        Some(format!("{preview}..."))
    } else {
        Some(preview)
    }
}

fn same_observation(left: &ChatContext, right: &ChatContext) -> bool {
    left.window_handle == right.window_handle
        && left.process_id == right.process_id
        && left.is_trusted_weixin == right.is_trusted_weixin
        && left.requires_elevation == right.requires_elevation
        && left.is_compatibility_available == right.is_compatibility_available
        && left.is_message_editor_focused == right.is_message_editor_focused
        && left.is_group_chat == right.is_group_chat
        && left.is_contact_chat == right.is_contact_chat
        && left.normalized_chat_title() == right.normalized_chat_title()
}

struct ComApartment {
    uninitialize: bool,
}

impl ComApartment {
    fn initialize() -> PlatformResult<Self> {
        // SAFETY: no reserved pointer is supplied and COM initialization is scoped by Drop.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            return Ok(Self { uninitialize: true });
        }
        if result == RPC_E_CHANGED_MODE {
            // Another component selected the apartment model. It owns the uninitialization;
            // UI Automation may still be used through that already-initialized apartment.
            return Ok(Self {
                uninitialize: false,
            });
        }
        Err(PlatformError::new(
            "com-initialize-failed",
            format!(
                "CoInitializeEx failed with HRESULT 0x{:08X}",
                result.0 as u32
            ),
        ))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: this instance successfully called CoInitializeEx on the current thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{
        ScreenRect, SendButtonDiagnostic, SendButtonSnapshot, SendButtonSnapshotState,
        WindowsContextProvider, diagnostic_error_code, normalize_draft_preview,
        resolved_trusted_weixin_path, send_button_candidate_score,
    };
    use std::time::{Duration, SystemTime};
    use wechat_send_guard_core::ChatContext;
    use wechat_send_guard_platform_api::PlatformError;

    fn recognized_chat(observed_at: SystemTime) -> ChatContext {
        ChatContext {
            window_handle: 42,
            process_id: 7,
            is_trusted_weixin: true,
            is_compatibility_available: true,
            is_message_editor_focused: true,
            is_group_chat: true,
            chat_title: Some("测试群".to_owned()),
            observed_at: Some(observed_at),
            ..ChatContext::default()
        }
    }

    #[test]
    fn diagnostic_error_keeps_only_the_stable_code_and_hresult() {
        let error = PlatformError::new(
            "uia-query-failed",
            "Element is unavailable. (0x80040201) private details are discarded",
        );
        assert_eq!(diagnostic_error_code(&error), "uia-query-failed:0x80040201");
    }

    #[test]
    fn draft_preview_is_trimmed_bounded_and_ephemeral_without_uia() {
        assert_eq!(
            normalize_draft_preview("  draft\0 ".into()),
            Some("draft".into())
        );
        assert_eq!(normalize_draft_preview("   ".into()), None);
        assert_eq!(
            normalize_draft_preview("a".repeat(241)),
            Some(format!("{}...", "a".repeat(240)))
        );
    }

    #[test]
    fn recent_recognized_chat_survives_the_settings_window_taking_focus() {
        let provider = WindowsContextProvider::new();
        provider.publish(recognized_chat(SystemTime::now()), 0);
        provider.publish(
            ChatContext {
                window_handle: 99,
                ..ChatContext::default()
            },
            0,
        );

        let remembered = provider
            .recent_recognized_chat(Duration::from_secs(5))
            .expect("the last usable Weixin chat should remain available briefly");
        assert_eq!(remembered.normalized_chat_title(), "测试群");
    }

    #[test]
    fn recent_recognized_chat_expires_and_is_cleared_by_an_unfocused_weixin_view() {
        let provider = WindowsContextProvider::new();
        provider.publish(
            recognized_chat(SystemTime::now() - Duration::from_secs(6)),
            0,
        );
        assert!(
            provider
                .recent_recognized_chat(Duration::from_secs(5))
                .is_none()
        );

        provider.publish(recognized_chat(SystemTime::now()), 0);
        provider.publish(
            ChatContext {
                window_handle: 42,
                process_id: 7,
                is_trusted_weixin: true,
                is_compatibility_available: true,
                is_group_chat: true,
                chat_title: Some("测试群".to_owned()),
                observed_at: Some(SystemTime::now()),
                ..ChatContext::default()
            },
            0,
        );
        assert!(
            provider
                .recent_recognized_chat(Duration::from_secs(5))
                .is_none()
        );
    }

    #[test]
    fn missing_or_invalid_external_path_disables_trust() {
        assert_eq!(resolved_trusted_weixin_path(None), None);
        assert_eq!(
            resolved_trusted_weixin_path(Some(r"D:\Apps\Weixin\other.exe")),
            None
        );
        assert_eq!(resolved_trusted_weixin_path(Some("Weixin.exe")), None);
    }

    #[test]
    fn send_button_diagnostic_uses_content_free_audit_codes() {
        assert_eq!(
            SendButtonDiagnostic::HitEnabledButton.audit_result(),
            "button-hit-enabled"
        );
        assert_eq!(
            SendButtonDiagnostic::ClickOutsideButton.audit_result(),
            "point-outside-send-button"
        );
    }

    #[test]
    fn cached_send_button_hit_is_fast_and_rejects_stale_or_wrong_window_points() {
        let provider = WindowsContextProvider::new();
        let observed_at = SystemTime::now();
        provider.publish_with_button(
            recognized_chat(observed_at),
            SendButtonSnapshot {
                window_handle: 42,
                observed_at: Some(observed_at),
                state: SendButtonSnapshotState::Enabled,
                bounds: Some(ScreenRect {
                    left: 100,
                    top: 200,
                    right: 180,
                    bottom: 240,
                }),
                window_bounds: None,
                window_dpi: 0,
                candidate_bounds: None,
                alternate_candidate_bounds: None,
            },
            0,
        );

        assert!(
            provider
                .classify_send_button_click(42, 120, 220, observed_at)
                .should_intercept()
        );
        assert_eq!(
            provider.classify_send_button_click(42, 90, 220, observed_at),
            SendButtonDiagnostic::ClickOutsideButton
        );
        assert_eq!(
            provider.classify_send_button_click(99, 120, 220, observed_at),
            SendButtonDiagnostic::SnapshotWindowMismatch
        );
        assert_eq!(
            provider.classify_send_button_click(
                42,
                120,
                220,
                observed_at + Duration::from_millis(2_501)
            ),
            SendButtonDiagnostic::SnapshotStale
        );
    }

    #[test]
    fn stale_or_unavailable_snapshot_is_consumed_only_inside_the_cached_toolbar() {
        let provider = WindowsContextProvider::new();
        let observed_at = SystemTime::now();
        provider.publish_with_button(
            recognized_chat(observed_at),
            SendButtonSnapshot {
                window_handle: 42,
                observed_at: Some(observed_at),
                state: SendButtonSnapshotState::QueryFailed,
                bounds: None,
                window_bounds: None,
                window_dpi: 0,
                candidate_bounds: Some(ScreenRect {
                    left: 100,
                    top: 200,
                    right: 260,
                    bottom: 250,
                }),
                alternate_candidate_bounds: None,
            },
            0,
        );

        assert!(provider.should_intercept_send_button_click(
            SendButtonDiagnostic::SnapshotUnavailable,
            42,
            180,
            220,
        ));
        assert!(!provider.should_intercept_send_button_click(
            SendButtonDiagnostic::SnapshotUnavailable,
            42,
            90,
            220,
        ));
        assert!(!provider.should_intercept_send_button_click(
            SendButtonDiagnostic::SnapshotStale,
            99,
            180,
            220,
        ));
    }

    #[test]
    fn conservative_candidate_is_narrow_and_right_aligned() {
        let editor = ScreenRect {
            left: 390,
            top: 700,
            right: 1110,
            bottom: 910,
        };
        let toolbar = ScreenRect {
            left: 390,
            top: 910,
            right: 1110,
            bottom: 970,
        };
        let candidate = super::conservative_send_candidate_bounds(Some(editor), &[toolbar])
            .expect("toolbar fallback should produce a candidate");

        assert_eq!(candidate.right, toolbar.right);
        assert_eq!(candidate.bottom, toolbar.bottom);
        assert!(candidate.left >= 890);
        assert!(candidate.right - candidate.left <= 220);
        assert!(candidate.bottom - candidate.top <= 96);
    }

    #[test]
    fn no_toolbar_does_not_guess_a_send_zone_from_the_editor() {
        let editor = ScreenRect {
            left: 390,
            top: 700,
            right: 1110,
            bottom: 910,
        };

        assert!(super::conservative_send_candidate_bounds(Some(editor), &[]).is_none());
    }

    #[test]
    fn send_button_scoring_prefers_the_toolbar_below_the_active_editor() {
        let editor = ScreenRect {
            left: 390,
            top: 700,
            right: 1110,
            bottom: 910,
        };
        let message_toolbar = ScreenRect {
            left: 390,
            top: 910,
            right: 1110,
            bottom: 970,
        };
        let right_browser_toolbar = ScreenRect {
            left: 1140,
            top: 650,
            right: 1980,
            bottom: 710,
        };

        assert!(
            send_button_candidate_score(Some(editor), Some(message_toolbar))
                < send_button_candidate_score(Some(editor), Some(right_browser_toolbar))
        );
    }

    #[test]
    fn cached_geometry_follows_window_origin_without_scaling_for_third_pane() {
        let original_window = ScreenRect {
            left: 100,
            top: 80,
            right: 1_100,
            bottom: 780,
        };
        let button = ScreenRect {
            left: 900,
            top: 690,
            right: 1_050,
            bottom: 750,
        };

        let moved = button
            .rebase(
                original_window,
                ScreenRect {
                    left: 350,
                    top: 200,
                    right: 1_350,
                    bottom: 900,
                },
            )
            .expect("a valid root can be rebased");
        assert_eq!(moved.left, 1_150);
        assert_eq!(moved.top, 810);

        let third_pane = button
            .rebase(
                original_window,
                ScreenRect {
                    left: 100,
                    top: 80,
                    right: 1_600,
                    bottom: 780,
                },
            )
            .expect("a valid root can be rebased");
        assert_eq!(third_pane, button);
    }

    #[test]
    fn observation_switches_enable_only_the_requested_input_paths() {
        let provider = WindowsContextProvider::new();

        provider.configure_observation(true, true, false);
        assert!(provider.observation_enabled());
        assert!(provider.keyboard_enter_observation_enabled());
        assert!(!provider.send_button_observation_enabled());

        provider.configure_observation(true, false, true);
        assert!(provider.observation_enabled());
        assert!(!provider.keyboard_enter_observation_enabled());
        assert!(provider.send_button_observation_enabled());

        provider.configure_observation(false, true, true);
        assert!(!provider.observation_enabled());
        assert!(!provider.keyboard_enter_observation_enabled());
        assert!(!provider.send_button_observation_enabled());
    }
}
