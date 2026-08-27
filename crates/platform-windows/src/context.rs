use std::{
    path::PathBuf,
    sync::{
        Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
use wechat_send_guard_core::{ChatContext, ChatTargetKind, SendGuardStateMachine};
use wechat_send_guard_platform_api::{
    ChatContextProvider, PlatformError, PlatformResult, SendTargetPlatform,
};
use windows::Win32::{
    Foundation::{HWND, RPC_E_CHANGED_MODE},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
    UI::{
        Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
            IUIAutomationValuePattern, TreeScope_Descendants, UIA_TextPatternId,
            UIA_ValuePatternId,
        },
        WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow},
    },
};

use crate::trust::{
    ProcessTrust, TRUSTED_WEIXIN_PATH, assess_window_trust_for_executable,
    is_valid_weixin_executable_path,
};

const INPUT_AUTOMATION_ID: &str = "chat_input_field";
const CHAT_NAME_AUTOMATION_ID: &str = "current_chat_name_label";
const GROUP_TITLE_CLASS_SUFFIX: &str = "mmui::ChatTitleBarChatRoomView";
const DRAFT_PREVIEW_LIMIT: usize = 240;
const FOCUS_RECOVERY_TIMEOUT: Duration = Duration::from_millis(900);
const FOCUS_RECOVERY_RETRY: Duration = Duration::from_millis(30);

/// Cached foreground-context provider. Constructing it only allocates an in-memory snapshot;
/// use `refresh_now` or `WindowsContextMonitor::start` to request Windows metadata.
#[derive(Debug)]
pub struct WindowsContextProvider {
    current: RwLock<ChatContext>,
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

    /// Creates a provider with the optional per-user Windows path override. `None` uses the
    /// supported installation path and an invalid configured value cannot match a process.
    pub fn new_with_trusted_weixin_executable_path(configured_path: Option<&str>) -> Self {
        Self {
            current: RwLock::new(ChatContext::default()),
            trusted_executable: RwLock::new(TrustedExecutable {
                path: resolved_trusted_weixin_path(configured_path),
                generation: 0,
            }),
        }
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

        self.publish(ChatContext::default(), generation);
        let _ = self.refresh_foreground();
    }

    pub fn refresh_foreground(&self) -> ChatContext {
        // SAFETY: GetForegroundWindow has no borrowed inputs and returns an owned value handle.
        let window_handle = unsafe { GetForegroundWindow().0 as isize };
        let observed_at = SystemTime::now();
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

        let candidate = if !trust.is_trusted_weixin {
            context_from_trust(window_handle, trust, observed_at)
        } else {
            inspect_supported_weixin_window(window_handle, trust.clone(), observed_at)
                .unwrap_or_else(|_| context_from_trust(window_handle, trust, observed_at))
        };
        self.publish(candidate, trust_generation)
    }

    fn publish(&self, mut candidate: ChatContext, trust_generation: u64) -> ChatContext {
        let trusted = read_unpoisoned(&self.trusted_executable);
        if trusted.generation != trust_generation {
            return read_unpoisoned(&self.current).clone();
        }
        let mut current = write_unpoisoned(&self.current);
        candidate.generation = if same_observation(&current, &candidate) {
            current.generation
        } else {
            current.generation.saturating_add(1)
        };
        *current = candidate.clone();
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
}

fn resolved_trusted_weixin_path(configured_path: Option<&str>) -> Option<PathBuf> {
    match configured_path {
        None => Some(PathBuf::from(TRUSTED_WEIXIN_PATH)),
        Some(value) => {
            let path = PathBuf::from(value.trim().trim_matches('"').trim());
            is_valid_weixin_executable_path(&path).then_some(path)
        }
    }
}

impl ChatContextProvider for WindowsContextProvider {
    fn current(&self) -> ChatContext {
        read_unpoisoned(&self.current).clone()
    }

    fn refresh_now(&self) -> PlatformResult<ChatContext> {
        Ok(self.refresh_foreground())
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

/// A deliberately simple polling monitor. It is started explicitly by the application and does
/// not own input hooks or issue synthetic input. A future UIA event implementation may replace
/// this without changing the platform API.
pub struct WindowsContextMonitor {
    provider: Arc<WindowsContextProvider>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    poll_interval: Duration,
}

impl WindowsContextMonitor {
    pub fn new(provider: Arc<WindowsContextProvider>) -> Self {
        Self {
            provider,
            stopping: Arc::new(AtomicBool::new(false)),
            worker: None,
            poll_interval: Duration::from_millis(75),
        }
    }

    pub fn start(&mut self) -> PlatformResult<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        self.stopping.store(false, Ordering::Release);
        let provider = Arc::clone(&self.provider);
        let stopping = Arc::clone(&self.stopping);
        let interval = self.poll_interval;
        self.worker = Some(
            thread::Builder::new()
                .name("wsg-context-monitor".to_owned())
                .spawn(move || {
                    while !stopping.load(Ordering::Acquire) {
                        let _ = provider.refresh_foreground();
                        thread::sleep(interval);
                    }
                })
                .map_err(|error| {
                    PlatformError::new("context-monitor-start-failed", error.to_string())
                })?,
        );
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for WindowsContextMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn inspect_supported_weixin_window(
    window_handle: isize,
    trust: ProcessTrust,
    observed_at: SystemTime,
) -> PlatformResult<ChatContext> {
    with_automation(|automation| {
        let root = element_from_handle(automation, window_handle)?;
        let editor = find_by_automation_id_suffix(automation, &root, INPUT_AUTOMATION_ID)?;
        let group_title = find_by_class_name_suffix(automation, &root, GROUP_TITLE_CLASS_SUFFIX)?;
        let title_root = group_title.as_ref().unwrap_or(&root);
        let title_element =
            find_by_automation_id_suffix(automation, title_root, CHAT_NAME_AUTOMATION_ID)?;
        let chat_title = title_element.as_ref().and_then(read_name);
        let target_kind = if group_title.is_some() {
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

        Ok(ChatContext {
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
        })
    })
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

fn find_by_automation_id_suffix(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    suffix: &str,
) -> PlatformResult<Option<IUIAutomationElement>> {
    find_descendant(automation, root, |element| {
        read_automation_id(element).is_some_and(|value| value == suffix || value.ends_with(suffix))
    })
}

fn find_by_class_name_suffix(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    suffix: &str,
) -> PlatformResult<Option<IUIAutomationElement>> {
    find_descendant(automation, root, |element| {
        read_class_name(element).is_some_and(|value| value == suffix || value.ends_with(suffix))
    })
}

fn find_descendant(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    predicate: impl Fn(&IUIAutomationElement) -> bool,
) -> PlatformResult<Option<IUIAutomationElement>> {
    // SAFETY: both COM proxies are valid for this synchronous query.
    let condition = unsafe { automation.CreateTrueCondition() }
        .map_err(|error| PlatformError::new("uia-condition-failed", error.to_string()))?;
    // SAFETY: root and condition are live COM proxies for the duration of this query.
    let elements = unsafe { root.FindAll(TreeScope_Descendants, &condition) }
        .map_err(|error| PlatformError::new("uia-query-failed", error.to_string()))?;
    // SAFETY: element array is live for the duration of the iteration.
    let length = unsafe { elements.Length() }
        .map_err(|error| PlatformError::new("uia-query-failed", error.to_string()))?;

    for index in 0..length {
        // SAFETY: `index` is bounded by the just-read array length.
        let element = match unsafe { elements.GetElement(index) } {
            Ok(element) => element,
            Err(_) => continue,
        };
        if predicate(&element) {
            return Ok(Some(element));
        }
    }
    Ok(None)
}

fn read_automation_id(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: property access is a synchronous COM call on a live element proxy.
    unsafe { element.CurrentAutomationId() }
        .ok()
        .map(|value| value.to_string())
}

fn read_class_name(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: property access is a synchronous COM call on a live element proxy.
    unsafe { element.CurrentClassName() }
        .ok()
        .map(|value| value.to_string())
}

fn read_name(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: property access is a synchronous COM call on a live element proxy.
    unsafe { element.CurrentName() }
        .ok()
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
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

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{TRUSTED_WEIXIN_PATH, normalize_draft_preview, resolved_trusted_weixin_path};
    use std::path::PathBuf;

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
    fn invalid_external_path_override_disables_trust_instead_of_falling_back() {
        assert_eq!(
            resolved_trusted_weixin_path(None),
            Some(PathBuf::from(TRUSTED_WEIXIN_PATH))
        );
        assert_eq!(
            resolved_trusted_weixin_path(Some(r"D:\Apps\Weixin\other.exe")),
            None
        );
        assert_eq!(resolved_trusted_weixin_path(Some("Weixin.exe")), None);
    }
}
