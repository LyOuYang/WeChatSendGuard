#![forbid(unsafe_code)]
//! OS boundary contracts for WeChatSendGuard.
//!
//! Production platform adapters implement these traits. The built-in test doubles
//! deliberately have no process discovery, UI Automation, hook, or input-injection path.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Mutex, MutexGuard},
    time::SystemTime,
};
use wechat_send_guard_core::{AuditEntry, ChatContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    pub code: &'static str,
    pub message: String,
}

impl PlatformError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PlatformError {}

pub type PlatformResult<T> = Result<T, PlatformError>;

/// Content-free evidence from the most recent foreground recognition attempt. Platform adapters
/// expose this only for local diagnostics; it must never contain a chat title, draft, full path,
/// screenshot, or other user content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextDiagnostics {
    pub observation_id: u64,
    pub window_handle: isize,
    pub process_id: u32,
    pub process_path_available: bool,
    pub is_trusted_weixin: bool,
    pub requires_elevation: bool,
    pub query_status: String,
    pub error_code: Option<String>,
    pub root_available: bool,
    pub root_class_name: Option<String>,
    pub root_control_type: Option<i32>,
    pub root_child_count: Option<i32>,
    pub provider_kind: String,
    pub tree_query_status: String,
    pub tree_error_code: Option<String>,
    pub tree_descendant_count: Option<usize>,
    pub tree_sampled_count: Option<usize>,
    pub tree_sample_truncated: bool,
    pub tree_control_type_counts: Option<String>,
    pub tree_automation_id_readable_count: Option<usize>,
    pub tree_automation_id_nonempty_count: Option<usize>,
    pub tree_class_name_readable_count: Option<usize>,
    pub tree_class_name_nonempty_count: Option<usize>,
    pub tree_property_read_failure_count: Option<usize>,
    pub editor_found: bool,
    pub editor_query_status: String,
    pub editor_query_error_code: Option<String>,
    pub editor_candidate_count: Option<usize>,
    pub chat_title_element_found: bool,
    pub chat_title_query_status: String,
    pub chat_title_query_error_code: Option<String>,
    pub chat_title_candidate_count: Option<usize>,
    pub chat_title_readable: bool,
    pub group_title_found: bool,
    pub group_title_query_status: String,
    pub group_title_query_error_code: Option<String>,
    pub group_title_candidate_count: Option<usize>,
    pub chat_branch_found: bool,
    pub editor_focused: bool,
    pub send_button_inspected: bool,
    pub toolbar_count: Option<usize>,
    pub send_button_candidate_count: Option<usize>,
    pub send_button_state: String,
    pub scan_duration_milliseconds: u128,
}

impl Default for ContextDiagnostics {
    fn default() -> Self {
        Self {
            observation_id: 0,
            window_handle: 0,
            process_id: 0,
            process_path_available: false,
            is_trusted_weixin: false,
            requires_elevation: false,
            query_status: "not-observed".to_owned(),
            error_code: None,
            root_available: false,
            root_class_name: None,
            root_control_type: None,
            root_child_count: None,
            provider_kind: "unavailable".to_owned(),
            tree_query_status: "not-queried".to_owned(),
            tree_error_code: None,
            tree_descendant_count: None,
            tree_sampled_count: None,
            tree_sample_truncated: false,
            tree_control_type_counts: None,
            tree_automation_id_readable_count: None,
            tree_automation_id_nonempty_count: None,
            tree_class_name_readable_count: None,
            tree_class_name_nonempty_count: None,
            tree_property_read_failure_count: None,
            editor_found: false,
            editor_query_status: "not-queried".to_owned(),
            editor_query_error_code: None,
            editor_candidate_count: None,
            chat_title_element_found: false,
            chat_title_query_status: "not-queried".to_owned(),
            chat_title_query_error_code: None,
            chat_title_candidate_count: None,
            chat_title_readable: false,
            group_title_found: false,
            group_title_query_status: "not-queried".to_owned(),
            group_title_query_error_code: None,
            group_title_candidate_count: None,
            chat_branch_found: false,
            editor_focused: false,
            send_button_inspected: false,
            toolbar_count: None,
            send_button_candidate_count: None,
            send_button_state: "not-observed".to_owned(),
            scan_duration_milliseconds: 0,
        }
    }
}

impl ContextDiagnostics {
    /// Converts the fixed diagnostic fields to the audit allow-list vocabulary. The caller passes
    /// the cached context and decision time so a race or stale observation is explicit in the log.
    pub fn audit_details(
        &self,
        context: &ChatContext,
        decision_time: SystemTime,
    ) -> BTreeMap<String, String> {
        let mut details = BTreeMap::from([
            ("observationId".to_owned(), self.observation_id.to_string()),
            (
                "diagnosticWindowMatchesContext".to_owned(),
                (self.window_handle == context.window_handle).to_string(),
            ),
            (
                "foregroundProcessId".to_owned(),
                self.process_id.to_string(),
            ),
            (
                "processPathAvailable".to_owned(),
                self.process_path_available.to_string(),
            ),
            (
                "trustedWeixin".to_owned(),
                self.is_trusted_weixin.to_string(),
            ),
            (
                "requiresElevation".to_owned(),
                self.requires_elevation.to_string(),
            ),
            ("uiaStatus".to_owned(), self.query_status.clone()),
            (
                "uiaRootAvailable".to_owned(),
                self.root_available.to_string(),
            ),
            ("uiaProviderKind".to_owned(), self.provider_kind.clone()),
            (
                "uiaTreeQueryStatus".to_owned(),
                self.tree_query_status.clone(),
            ),
            (
                "uiaTreeSampleTruncated".to_owned(),
                self.tree_sample_truncated.to_string(),
            ),
            ("editorFound".to_owned(), self.editor_found.to_string()),
            (
                "editorQueryStatus".to_owned(),
                self.editor_query_status.clone(),
            ),
            (
                "chatTitleElementFound".to_owned(),
                self.chat_title_element_found.to_string(),
            ),
            (
                "chatTitleQueryStatus".to_owned(),
                self.chat_title_query_status.clone(),
            ),
            (
                "chatTitleReadable".to_owned(),
                self.chat_title_readable.to_string(),
            ),
            (
                "groupTitleFound".to_owned(),
                self.group_title_found.to_string(),
            ),
            (
                "groupTitleQueryStatus".to_owned(),
                self.group_title_query_status.clone(),
            ),
            (
                "chatBranchFound".to_owned(),
                self.chat_branch_found.to_string(),
            ),
            ("editorFocused".to_owned(), self.editor_focused.to_string()),
            (
                "sendButtonInspected".to_owned(),
                self.send_button_inspected.to_string(),
            ),
            ("sendButtonState".to_owned(), self.send_button_state.clone()),
            (
                "uiaScanDurationMilliseconds".to_owned(),
                self.scan_duration_milliseconds.to_string(),
            ),
            (
                "contextCompatibilityAvailable".to_owned(),
                context.is_compatibility_available.to_string(),
            ),
            (
                "contextGeneration".to_owned(),
                context.generation.to_string(),
            ),
        ]);
        if let Some(value) = &self.error_code {
            details.insert("uiaErrorCode".to_owned(), value.clone());
        }
        if let Some(value) = &self.tree_error_code {
            details.insert("uiaTreeErrorCode".to_owned(), value.clone());
        }
        if let Some(value) = &self.editor_query_error_code {
            details.insert("editorQueryErrorCode".to_owned(), value.clone());
        }
        if let Some(value) = &self.chat_title_query_error_code {
            details.insert("chatTitleQueryErrorCode".to_owned(), value.clone());
        }
        if let Some(value) = &self.group_title_query_error_code {
            details.insert("groupTitleQueryErrorCode".to_owned(), value.clone());
        }
        if let Some(value) = &self.root_class_name {
            details.insert("uiaRootClassName".to_owned(), value.clone());
        }
        if let Some(value) = self.root_control_type {
            details.insert("uiaRootControlType".to_owned(), value.to_string());
        }
        if let Some(value) = self.root_child_count {
            details.insert("uiaRootChildCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.tree_descendant_count {
            details.insert("uiaTreeDescendantCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.tree_sampled_count {
            details.insert("uiaTreeSampledCount".to_owned(), value.to_string());
        }
        if let Some(value) = &self.tree_control_type_counts {
            details.insert("uiaTreeControlTypeCounts".to_owned(), value.clone());
        }
        if let Some(value) = self.tree_automation_id_readable_count {
            details.insert(
                "uiaTreeAutomationIdReadableCount".to_owned(),
                value.to_string(),
            );
        }
        if let Some(value) = self.tree_automation_id_nonempty_count {
            details.insert(
                "uiaTreeAutomationIdNonemptyCount".to_owned(),
                value.to_string(),
            );
        }
        if let Some(value) = self.tree_class_name_readable_count {
            details.insert(
                "uiaTreeClassNameReadableCount".to_owned(),
                value.to_string(),
            );
        }
        if let Some(value) = self.tree_class_name_nonempty_count {
            details.insert(
                "uiaTreeClassNameNonemptyCount".to_owned(),
                value.to_string(),
            );
        }
        if let Some(value) = self.tree_property_read_failure_count {
            details.insert(
                "uiaTreePropertyReadFailureCount".to_owned(),
                value.to_string(),
            );
        }
        if let Some(value) = self.toolbar_count {
            details.insert("sendToolbarCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.editor_candidate_count {
            details.insert("editorCandidateCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.chat_title_candidate_count {
            details.insert("chatTitleCandidateCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.group_title_candidate_count {
            details.insert("groupTitleCandidateCount".to_owned(), value.to_string());
        }
        if let Some(value) = self.send_button_candidate_count {
            details.insert("sendButtonCandidateCount".to_owned(), value.to_string());
        }
        if let Some(observed_at) = context.observed_at
            && let Ok(age) = decision_time.duration_since(observed_at)
        {
            details.insert(
                "contextAgeMilliseconds".to_owned(),
                age.as_millis().to_string(),
            );
        }
        details
    }
}

/// Returns only a read-only foreground context snapshot. Implementations must never infer
/// identity from screenshots, private client data, injected code, or process memory.
pub trait ChatContextProvider: Send + Sync {
    fn current(&self) -> ChatContext;
    fn refresh_now(&self) -> PlatformResult<ChatContext>;

    fn current_diagnostics(&self) -> Option<ContextDiagnostics> {
        None
    }
}

/// Performs the slow, out-of-hook recovery step needed after a confirmation window closes.
/// Implementations must refresh the context after restoring focus; callers always ask core to
/// compare the returned snapshot before input is injected.
pub trait SendTargetPlatform: ChatContextProvider {
    fn restore_editor_focus_and_refresh(
        &self,
        expected: &ChatContext,
    ) -> PlatformResult<ChatContext>;
    fn read_draft_preview(&self, expected: &ChatContext) -> PlatformResult<Option<String>>;
}

/// Emits the one already-authorized key. The caller is responsible for asking core to
/// revalidate immediately before this method is called.
pub trait InputInjector: Send + Sync {
    fn send_enter(&self, is_numpad_enter: bool) -> PlatformResult<()>;
}

/// Starts and stops the platform's physical-input observer. The observer must remain fast,
/// use cached context on callback paths, and cannot perform confirmation or injection itself.
pub trait InputGate: Send {
    fn start(&mut self) -> PlatformResult<()>;
    fn stop(&mut self);
}

/// Per-user startup registration. Platform implementations must not require elevation.
pub trait StartupRegistration: Send + Sync {
    fn apply(&self, enabled: bool) -> PlatformResult<bool>;
}

/// Audit writes are best effort. Implementations must not make sending wait for disk I/O.
pub trait AuditLog: Send + Sync {
    fn write(&self, entry: AuditEntry);
}

/// Test-only context provider. It returns data supplied by the test and has no operating
/// system integration. Production code must use a platform-specific provider instead.
#[derive(Debug, Default)]
pub struct FakeChatContextProvider {
    current: Mutex<ChatContext>,
    draft_preview: Mutex<Option<String>>,
}

impl FakeChatContextProvider {
    pub fn new(context: ChatContext) -> Self {
        Self {
            current: Mutex::new(context),
            draft_preview: Mutex::new(None),
        }
    }

    pub fn set_current(&self, context: ChatContext) {
        *lock_unpoisoned(&self.current) = context;
    }

    pub fn set_draft_preview(&self, preview: Option<String>) {
        *lock_unpoisoned(&self.draft_preview) = preview;
    }
}

impl ChatContextProvider for FakeChatContextProvider {
    fn current(&self) -> ChatContext {
        lock_unpoisoned(&self.current).clone()
    }

    fn refresh_now(&self) -> PlatformResult<ChatContext> {
        Ok(self.current())
    }
}

impl SendTargetPlatform for FakeChatContextProvider {
    fn restore_editor_focus_and_refresh(
        &self,
        _expected: &ChatContext,
    ) -> PlatformResult<ChatContext> {
        Ok(self.current())
    }

    fn read_draft_preview(&self, _expected: &ChatContext) -> PlatformResult<Option<String>> {
        Ok(lock_unpoisoned(&self.draft_preview).clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedInput {
    pub is_numpad_enter: bool,
}

/// Test-only injector. It records an intent in memory and never calls an operating-system API.
#[derive(Debug, Default)]
pub struct RecordingInputInjector {
    sent: Mutex<Vec<RecordedInput>>,
}

impl RecordingInputInjector {
    pub fn sent(&self) -> Vec<RecordedInput> {
        lock_unpoisoned(&self.sent).clone()
    }

    pub fn clear(&self) {
        lock_unpoisoned(&self.sent).clear();
    }
}

impl InputInjector for RecordingInputInjector {
    fn send_enter(&self, is_numpad_enter: bool) -> PlatformResult<()> {
        lock_unpoisoned(&self.sent).push(RecordedInput { is_numpad_enter });
        Ok(())
    }
}

/// Test-only audit sink. It keeps entries in memory and has no file-system or network path.
#[derive(Debug, Default)]
pub struct RecordingAuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl RecordingAuditLog {
    pub fn entries(&self) -> Vec<AuditEntry> {
        lock_unpoisoned(&self.entries).clone()
    }
}

impl AuditLog for RecordingAuditLog {
    fn write(&self, entry: AuditEntry) {
        lock_unpoisoned(&self.entries).push(entry);
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::ContextDiagnostics;
    use std::time::SystemTime;
    use wechat_send_guard_core::ChatContext;

    #[test]
    fn audit_details_keep_tree_and_selector_failure_evidence() {
        let diagnostics = ContextDiagnostics {
            tree_query_status: "partial-property-read-failure".to_owned(),
            tree_error_code: Some("uia-tree-class-name-read-failed:0x80040201".to_owned()),
            tree_descendant_count: Some(37),
            tree_control_type_counts: Some("50000:4,50004:2".to_owned()),
            editor_query_status: "query-failed".to_owned(),
            editor_query_error_code: Some("uia-query-failed:0x80040201".to_owned()),
            editor_candidate_count: Some(0),
            ..ContextDiagnostics::default()
        };

        let details = diagnostics.audit_details(&ChatContext::default(), SystemTime::now());

        assert_eq!(
            details.get("uiaTreeQueryStatus").map(String::as_str),
            Some("partial-property-read-failure")
        );
        assert_eq!(
            details.get("uiaTreeDescendantCount").map(String::as_str),
            Some("37")
        );
        assert_eq!(
            details.get("editorQueryErrorCode").map(String::as_str),
            Some("uia-query-failed:0x80040201")
        );
    }
}
