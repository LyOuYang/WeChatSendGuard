#![forbid(unsafe_code)]
//! Application orchestration for the send guard.
//!
//! The service accepts only snapshots and platform contracts. It has no Windows, Slint,
//! Weixin, network, or real-input dependency, so its integration tests use safe doubles.

use std::{
    sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, SystemTime},
};
use wechat_send_guard_core::{
    AppSettings, AuditEntry, ChatContext, ChatTargetKind, ConfirmationOutcome, PendingConfirmation,
    ProtectionDecisionKind, RuleMode, SendGuardStateMachine, TemporaryBypassRegistry,
    evaluate_chat_protection, evaluate_protection, title_matches,
};
use wechat_send_guard_platform_api::{AuditLog, InputInjector, SendTargetPlatform};

/// Input callbacks consume a completed platform snapshot. The platform adapter is responsible
/// for keeping it fresh; this bound prevents a snapshot from authorizing a target after a long
/// recognition stall while still allowing a short UIA/layout rebuild to be confirmed and
/// revalidated before injection.
const MAX_CONTEXT_AGE: Duration = Duration::from_millis(2_500);
const MAX_CONTEXT_FUTURE_LEAD: Duration = Duration::from_secs(10);
/// A tray action may briefly move focus away from Weixin before the click callback runs. The
/// remembered context remains safe for this bounded interval because the resulting bypass is
/// still keyed to the exact window, process, target kind, and normalized chat title.
pub const TEMPORARY_BYPASS_CONTEXT_MAX_AGE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalEnter {
    pub is_numpad_enter: bool,
    pub is_injected: bool,
    pub shift_pressed: bool,
    /// True when Enter is combined with any recognized modifier, including Shift.
    pub modifier_pressed: bool,
    pub foreground_window: isize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnterHandling {
    PassThrough,
    SuppressBlockedUnknown,
    SuppressAndConfirm(Box<PendingConfirmation>),
    SuppressWhileConfirmationActive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResult {
    NotInjected { reason: String },
    Injected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporaryBypassGrant {
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryBypassRejection {
    InvalidDuration,
    ProtectionDisabled,
    ProtectionPaused,
    ContextStale,
    ContextUntrusted,
    ContextIncomplete,
    EditorUnfocused,
    ProtectedChatMissing,
    NoProtectionRequired,
    UnknownContext,
    BlockedUnknownContext,
}

struct BypassAuditRecord<'a> {
    protected_chat_id: Option<uuid::Uuid>,
    trace_id: uuid::Uuid,
    result: &'a str,
    requested_minutes: u32,
    settings: &'a AppSettings,
    context: &'a ChatContext,
    context_source: &'a str,
    decision: Option<ProtectionDecisionKind>,
    timestamp: SystemTime,
}

/// The only path from a physical key decision to a synthetic send request.
pub struct GuardService {
    settings: RwLock<AppSettings>,
    state_machine: SendGuardStateMachine,
    bypasses: TemporaryBypassRegistry,
    platform: Arc<dyn SendTargetPlatform>,
    injector: Arc<dyn InputInjector>,
    audit_log: Arc<dyn AuditLog>,
    active_confirmation: Mutex<Option<uuid::Uuid>>,
    pause_until: Mutex<Option<SystemTime>>,
}

impl GuardService {
    /// Temporary bypasses and pauses intentionally start empty on every service construction.
    /// They are runtime-only permissions and must never be restored from persisted settings.
    pub fn new(
        settings: AppSettings,
        platform: Arc<dyn SendTargetPlatform>,
        injector: Arc<dyn InputInjector>,
        audit_log: Arc<dyn AuditLog>,
    ) -> Self {
        Self {
            settings: RwLock::new(settings.sanitize()),
            state_machine: SendGuardStateMachine::default(),
            bypasses: TemporaryBypassRegistry::default(),
            platform,
            injector,
            audit_log,
            active_confirmation: Mutex::new(None),
            pause_until: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> AppSettings {
        read_unpoisoned(&self.settings).clone()
    }

    pub fn update_settings(&self, settings: AppSettings) {
        *write_unpoisoned(&self.settings) = settings.sanitize();
    }

    /// This method is safe for a fast keyboard-hook callback because it reads a cached
    /// platform snapshot only. It does not refresh UI Automation, show UI, or inject input.
    pub fn handle_physical_enter(&self, enter: PhysicalEnter, now: SystemTime) -> EnterHandling {
        let trace_id = uuid::Uuid::new_v4();
        let source = if enter.is_numpad_enter {
            "numpad-enter"
        } else {
            "enter"
        };
        if enter.is_injected {
            return EnterHandling::PassThrough;
        }

        let settings = self.settings();
        if !settings.enabled || self.is_paused(now) {
            return EnterHandling::PassThrough;
        }
        let context = self.platform.current();
        // The physical hook observes all applications. Keep diagnostics scoped to a cached,
        // trusted Weixin context so normal typing in unrelated applications never becomes a
        // local audit trail.
        if !context.is_trusted_weixin {
            return EnterHandling::PassThrough;
        }
        if !settings.intercept_keyboard_enter
            || (enter.is_numpad_enter && !settings.intercept_numpad_enter)
            || (settings.shift_enter_pass_through && enter.shift_pressed)
            || enter.modifier_pressed
        {
            self.write_trace(
                None,
                trace_id,
                "send-decision",
                "pass-through-strategy",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }

        if context.window_handle != enter.foreground_window {
            self.write_trace(
                None,
                trace_id,
                "send-decision",
                "pass-through-context",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }

        if !context.is_compatibility_available {
            // A trusted Weixin window with an incomplete UIA snapshot is still a possible send
            // target. Passing the physical Enter through here would reintroduce the exact
            // fail-open path seen during three-pane layout rebuilds, so consume it until the
            // monitor publishes a usable snapshot.
            self.write_trace(
                None,
                trace_id,
                "send-blocked",
                "context-unavailable",
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }
        if !context.is_message_editor_focused {
            // The editor is known but another Weixin control owns focus. Enter is not a message
            // send in that state, so preserve the client's normal behavior.
            self.write_trace(
                None,
                trace_id,
                "send-decision",
                "pass-through-context",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }

        let stale = context_is_stale(&context, now);
        let decision = evaluate_protection(&context, &settings, &self.bypasses, now);
        if stale && !decision.should_suppress() {
            // A stale snapshot cannot authorize a pass-through decision: the user may have
            // switched to a protected chat since the last observation. Consume this Enter until
            // the monitor catches up instead of allowing a native send through the race window.
            self.write_trace(None, trace_id, "send-blocked", "stale-context", source, now);
            return EnterHandling::SuppressBlockedUnknown;
        }
        if !decision.should_suppress() {
            self.write_trace(
                None,
                trace_id,
                "send-decision",
                "pass-through-rule",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }
        if decision.kind == ProtectionDecisionKind::BlockUnknown {
            self.write_trace(
                None,
                trace_id,
                "send-blocked",
                if stale {
                    "stale-context"
                } else {
                    "unknown-chat"
                },
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }

        let timeout = Duration::from_secs(u64::from(settings.confirmation.timeout_seconds));
        if stale {
            self.write_trace(
                None,
                trace_id,
                "send-blocked",
                "stale-context-confirmation",
                source,
                now,
            );
        }
        self.begin_confirmation(ConfirmationRequest {
            context,
            decision,
            is_numpad_enter: enter.is_numpad_enter,
            trace_id,
            source,
            timeout,
            now,
        })
    }

    fn begin_confirmation(&self, request: ConfirmationRequest<'_>) -> EnterHandling {
        let ConfirmationRequest {
            context,
            decision,
            is_numpad_enter,
            trace_id,
            source,
            timeout,
            now,
        } = request;
        match self.state_machine.try_begin(
            context,
            decision,
            is_numpad_enter,
            trace_id,
            timeout,
            now,
        ) {
            Some(pending) => {
                *lock_unpoisoned(&self.active_confirmation) = Some(pending.attempt_id);
                self.write_trace(
                    pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                    pending.trace_id,
                    "confirmation",
                    "requested",
                    source,
                    now,
                );
                EnterHandling::SuppressAndConfirm(Box::new(pending))
            }
            None => {
                self.write_trace(
                    None,
                    trace_id,
                    "send-blocked",
                    "confirmation-already-active",
                    source,
                    now,
                );
                EnterHandling::SuppressWhileConfirmationActive
            }
        }
    }

    /// Handles a physical click that the platform has already identified as the Weixin send
    /// button. It intentionally does not depend on the keyboard-Enter preference: the two input
    /// strategies are independent settings, but they share the same protection decision and
    /// confirmation state machine.
    pub fn handle_send_button_click(
        &self,
        foreground_window: isize,
        now: SystemTime,
    ) -> EnterHandling {
        self.handle_send_button_click_with_trace(foreground_window, uuid::Uuid::new_v4(), now)
    }

    /// Same as [`Self::handle_send_button_click`], while allowing the Windows adapter to attach
    /// its button-hit diagnosis to the same opaque diagnostic trace. The trace contains no
    /// message content or chat title.
    pub fn handle_send_button_click_with_trace(
        &self,
        foreground_window: isize,
        trace_id: uuid::Uuid,
        now: SystemTime,
    ) -> EnterHandling {
        let source = "send-button";
        let settings = self.settings();
        if !settings.enabled || self.is_paused(now) || !settings.intercept_send_button {
            self.write_trace(
                None,
                trace_id,
                "send-button-decision",
                "pass-through-strategy",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }

        let context = self.platform.current();
        if context.window_handle != foreground_window {
            self.write_trace(
                None,
                trace_id,
                "send-button-decision",
                "pass-through-context",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }

        if !context.is_trusted_weixin {
            // The platform invokes this method only after a candidate was identified in the same
            // foreground window. If recognition drops between the diagnostic and this read, keep
            // the candidate blocked instead of handing a race window to Weixin.
            self.write_trace(
                None,
                trace_id,
                "send-button-blocked",
                "context-unavailable",
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }

        // The platform has already identified a click in the send-button candidate area. If the
        // companion context fields are temporarily unavailable, fail closed for this candidate
        // instead of handing the native click to Weixin. Clearly unrelated windows still pass
        // through above.
        if !context.is_compatibility_available || !context.is_message_editor_focused {
            self.write_trace(
                None,
                trace_id,
                "send-button-blocked",
                "context-unavailable",
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }

        let stale = context_is_stale(&context, now);
        let decision = evaluate_protection(&context, &settings, &self.bypasses, now);
        if stale && !decision.should_suppress() {
            self.write_trace(
                None,
                trace_id,
                "send-button-blocked",
                "stale-context",
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }
        if !decision.should_suppress() {
            self.write_trace(
                None,
                trace_id,
                "send-button-decision",
                "pass-through-rule",
                source,
                now,
            );
            return EnterHandling::PassThrough;
        }
        if decision.kind == ProtectionDecisionKind::BlockUnknown {
            self.write_trace(
                None,
                trace_id,
                "send-button-blocked",
                "unknown-chat",
                source,
                now,
            );
            return EnterHandling::SuppressBlockedUnknown;
        }

        let timeout = Duration::from_secs(u64::from(settings.confirmation.timeout_seconds));
        if stale {
            self.write_trace(
                None,
                trace_id,
                "send-button-blocked",
                "stale-context-confirmation",
                source,
                now,
            );
        }
        self.begin_confirmation(ConfirmationRequest {
            context,
            decision,
            is_numpad_enter: false,
            trace_id,
            source,
            timeout,
            now,
        })
    }

    /// Reads an optional preview after suppression, never on the keyboard-hook path. The
    /// preview is returned only to the UI and is intentionally not logged or persisted.
    pub fn enrich_pending_confirmation(
        &self,
        pending: &PendingConfirmation,
    ) -> PendingConfirmation {
        if self
            .state_machine
            .current()
            .as_ref()
            .map(|current| current.attempt_id)
            != Some(pending.attempt_id)
        {
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "confirmation-preview",
                "stale-attempt",
                SystemTime::now(),
            );
            return pending.clone();
        }

        let mut enriched = pending.clone();
        let preview_result = match self.platform.read_draft_preview(&pending.original_context) {
            Ok(Some(preview)) => {
                enriched.draft_preview = Some(preview);
                "available"
            }
            Ok(None) => "empty",
            Err(_) => "unavailable",
        };
        self.write_audit(
            pending.decision.protected_chat.as_ref().map(|chat| chat.id),
            Some(pending.trace_id),
            "confirmation-preview",
            preview_result,
            SystemTime::now(),
        );
        enriched
    }

    /// Completes a visible confirmation. A confirmation can inject only after the platform
    /// restores the original editor and returns a core-equivalent context snapshot.
    pub fn complete_confirmation(
        &self,
        pending: &PendingConfirmation,
        outcome: ConfirmationOutcome,
        now: SystemTime,
    ) -> CompletionResult {
        if outcome != ConfirmationOutcome::Confirmed {
            let resolution = self.state_machine.resolve(
                pending.attempt_id,
                outcome,
                &ChatContext::inactive(),
                now,
            );
            self.clear_active_confirmation(pending.attempt_id);
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "confirmation",
                outcome_name(outcome),
                now,
            );
            return CompletionResult::NotInjected {
                reason: resolution.reason.to_owned(),
            };
        }

        let revalidated_context = match self
            .platform
            .restore_editor_focus_and_refresh(&pending.original_context)
        {
            Ok(context) => context,
            Err(error) => {
                self.state_machine.cancel_active();
                self.clear_active_confirmation(pending.attempt_id);
                self.write_audit(
                    pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                    Some(pending.trace_id),
                    "send",
                    "cancelled-editor-focus",
                    now,
                );
                return CompletionResult::NotInjected {
                    reason: error.to_string(),
                };
            }
        };

        // A platform adapter may return a structurally matching snapshot even while its
        // recognition worker is stalled. Do not let that snapshot authorize injection; the final
        // observation must itself be recent (test doubles without timestamps remain supported).
        if context_is_stale(&revalidated_context, now) {
            self.state_machine.cancel_active();
            self.clear_active_confirmation(pending.attempt_id);
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "send",
                "cancelled-stale-revalidation",
                now,
            );
            return CompletionResult::NotInjected {
                reason: "The target context was still stale after confirmation.".to_owned(),
            };
        }

        let resolution = self.state_machine.resolve(
            pending.attempt_id,
            ConfirmationOutcome::Confirmed,
            &revalidated_context,
            now,
        );
        self.clear_active_confirmation(pending.attempt_id);
        if !resolution.should_inject {
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "send",
                "cancelled-context-changed",
                now,
            );
            return CompletionResult::NotInjected {
                reason: resolution.reason.to_owned(),
            };
        }

        if self.is_paused(now) {
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "send",
                "cancelled-protection-paused",
                now,
            );
            return CompletionResult::NotInjected {
                reason: "Send protection is temporarily paused.".to_owned(),
            };
        }

        match self.injector.send_enter(pending.is_numpad_enter) {
            Ok(()) => {
                self.write_audit(
                    pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                    Some(pending.trace_id),
                    "send",
                    "injected",
                    now,
                );
                CompletionResult::Injected
            }
            Err(error) => {
                self.write_audit(
                    pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                    Some(pending.trace_id),
                    "send",
                    "injection-failed",
                    now,
                );
                CompletionResult::NotInjected {
                    reason: error.to_string(),
                }
            }
        }
    }

    pub fn cancel_for_context_change(
        &self,
        observed_context: &ChatContext,
        confirmation_owns_foreground: bool,
        original_window_still_exists: bool,
        now: SystemTime,
    ) -> bool {
        let Some(pending) = self.state_machine.current() else {
            return false;
        };

        if observed_context.is_trusted_weixin
            && SendGuardStateMachine::represents_same_session(
                &pending.original_context,
                observed_context,
            )
        {
            return false;
        }
        if !observed_context.is_trusted_weixin
            && confirmation_owns_foreground
            && original_window_still_exists
        {
            return false;
        }

        self.state_machine.cancel_active();
        self.clear_active_confirmation(pending.attempt_id);
        self.write_audit(
            pending.decision.protected_chat.as_ref().map(|chat| chat.id),
            Some(pending.trace_id),
            "confirmation",
            "cancelled-context-changed",
            now,
        );
        true
    }

    pub fn try_grant_current_bypass(
        &self,
        minutes: u32,
        now: SystemTime,
    ) -> Result<TemporaryBypassGrant, TemporaryBypassRejection> {
        let context = self.platform.current();
        self.try_grant_bypass_for_context_with_source(context, minutes, now, "foreground-snapshot")
    }

    pub fn try_grant_bypass_for_context(
        &self,
        context: ChatContext,
        minutes: u32,
        now: SystemTime,
    ) -> Result<TemporaryBypassGrant, TemporaryBypassRejection> {
        self.try_grant_bypass_for_context_with_source(context, minutes, now, "provided-context")
    }

    /// Returns the remaining runtime-only bypass for the currently recognized chat, if any.
    /// The lookup is scoped to the same identity used by the send decision and is never backed by
    /// persisted settings.
    pub fn temporary_bypass_remaining_for_context(
        &self,
        context: &ChatContext,
        now: SystemTime,
    ) -> Option<Duration> {
        if context.requires_elevation
            || !context.is_trusted_weixin
            || !context.is_compatibility_available
            || !context.is_known_chat()
        {
            return None;
        }

        let settings = self.settings();
        if !settings.enabled {
            return None;
        }
        match settings.rule_mode {
            RuleMode::ConfirmUnlessExcluded => self.bypasses.remaining_for_context(context, now),
            RuleMode::ProtectListed => {
                let target_kind = context.target_kind()?;
                let title = context.normalized_chat_title();
                settings
                    .protected_chats
                    .iter()
                    .find(|chat| {
                        chat.enabled
                            && chat.target_kind == target_kind
                            && title_matches(chat, &title)
                    })
                    .and_then(|chat| self.bypasses.remaining(chat.id, now))
            }
        }
    }

    /// Returns the rule result for the currently recognized chat without requiring editor focus.
    /// The send path remains responsible for checking focus before it can suppress input.
    pub fn current_session_requires_protection(
        &self,
        context: &ChatContext,
        now: SystemTime,
    ) -> Option<bool> {
        if context.requires_elevation
            || !context.is_trusted_weixin
            || !context.is_compatibility_available
            || !context.is_known_chat()
            || context.normalized_chat_title().is_empty()
        {
            return None;
        }

        let settings = self.settings();
        Some(evaluate_chat_protection(context, &settings, &self.bypasses, now).should_suppress())
    }

    fn try_grant_bypass_for_context_with_source(
        &self,
        context: ChatContext,
        minutes: u32,
        now: SystemTime,
        context_source: &str,
    ) -> Result<TemporaryBypassGrant, TemporaryBypassRejection> {
        let trace_id = uuid::Uuid::new_v4();
        let settings = self.settings();
        let audit_bypass = |protected_chat_id, result, decision| {
            self.write_bypass_audit(BypassAuditRecord {
                protected_chat_id,
                trace_id,
                result,
                requested_minutes: minutes,
                settings: &settings,
                context: &context,
                context_source,
                decision,
                timestamp: now,
            });
        };
        if !is_temporary_duration(minutes) {
            audit_bypass(None, "rejected-invalid-duration", None);
            return Err(TemporaryBypassRejection::InvalidDuration);
        }
        if !settings.enabled {
            audit_bypass(None, "rejected-protection-disabled", None);
            return Err(TemporaryBypassRejection::ProtectionDisabled);
        }
        if self.is_paused(now) {
            audit_bypass(None, "rejected-protection-paused", None);
            return Err(TemporaryBypassRejection::ProtectionPaused);
        }
        if context_exceeds_maximum_age(&context, now, TEMPORARY_BYPASS_CONTEXT_MAX_AGE) {
            audit_bypass(None, "rejected-context-stale", None);
            return Err(TemporaryBypassRejection::ContextStale);
        }
        if !context.is_trusted_weixin {
            audit_bypass(None, "rejected-context-untrusted", None);
            return Err(TemporaryBypassRejection::ContextUntrusted);
        }
        if !context.is_compatibility_available {
            audit_bypass(None, "rejected-context-incomplete", None);
            return Err(TemporaryBypassRejection::ContextIncomplete);
        }
        if !context.is_message_editor_focused {
            audit_bypass(None, "rejected-editor-unfocused", None);
            return Err(TemporaryBypassRejection::EditorUnfocused);
        }

        let decision = evaluate_protection(&context, &settings, &self.bypasses, now);
        let duration = Duration::from_secs(u64::from(minutes) * 60);
        let (display_name, protected_chat_id) = match decision.kind {
            ProtectionDecisionKind::ConfirmProtected => {
                let Some(protected_chat) = decision.protected_chat else {
                    audit_bypass(None, "rejected-protected-chat-missing", Some(decision.kind));
                    return Err(TemporaryBypassRejection::ProtectedChatMissing);
                };
                self.bypasses.grant(protected_chat.id, duration, now);
                (protected_chat.display_name, Some(protected_chat.id))
            }
            ProtectionDecisionKind::ConfirmUnlisted => {
                let title = context.normalized_chat_title();
                self.bypasses.grant_for_context(&context, duration, now);
                (title, None)
            }
            ProtectionDecisionKind::Pass => {
                audit_bypass(None, "rejected-no-protection-required", Some(decision.kind));
                return Err(TemporaryBypassRejection::NoProtectionRequired);
            }
            ProtectionDecisionKind::ConfirmUnknown => {
                audit_bypass(None, "rejected-unknown-context", Some(decision.kind));
                return Err(TemporaryBypassRejection::UnknownContext);
            }
            ProtectionDecisionKind::BlockUnknown => {
                audit_bypass(
                    None,
                    "rejected-blocked-unknown-context",
                    Some(decision.kind),
                );
                return Err(TemporaryBypassRejection::BlockedUnknownContext);
            }
        };
        audit_bypass(
            protected_chat_id,
            &format!("granted-{minutes}m"),
            Some(decision.kind),
        );
        Ok(TemporaryBypassGrant { display_name })
    }

    /// Temporarily disables all send protection without changing the persisted setting. The
    /// caller is responsible for disabling platform observation while this state is active.
    pub fn try_pause(&self, minutes: u32, now: SystemTime) -> bool {
        let trace_id = uuid::Uuid::new_v4();
        let settings = self.settings();
        if !is_temporary_duration(minutes) {
            self.write_pause_audit(
                trace_id,
                "rejected-invalid-duration",
                "tray-pause",
                Some(minutes),
                &settings,
                now,
            );
            return false;
        }
        if !settings.enabled {
            self.write_pause_audit(
                trace_id,
                "rejected-protection-disabled",
                "tray-pause",
                Some(minutes),
                &settings,
                now,
            );
            return false;
        }

        *lock_unpoisoned(&self.pause_until) =
            Some(now + Duration::from_secs(u64::from(minutes) * 60));
        if let Some(pending) = self.state_machine.current() {
            self.state_machine.cancel_active();
            self.clear_active_confirmation(pending.attempt_id);
            self.write_audit(
                pending.decision.protected_chat.as_ref().map(|chat| chat.id),
                Some(pending.trace_id),
                "confirmation",
                "cancelled-protection-paused",
                now,
            );
        }
        self.write_pause_audit(
            trace_id,
            &format!("granted-{minutes}m"),
            "tray-pause",
            Some(minutes),
            &settings,
            now,
        );
        true
    }

    pub fn pause_remaining(&self, now: SystemTime) -> Option<Duration> {
        let expires_at = lock_unpoisoned(&self.pause_until).as_ref().copied()?;
        expires_at
            .duration_since(now)
            .ok()
            .filter(|remaining| !remaining.is_zero())
    }

    pub fn resume_pause(&self, now: SystemTime) -> bool {
        let expires_at = lock_unpoisoned(&self.pause_until).take();
        let was_active = expires_at.is_some_and(|expires_at| expires_at > now);
        if was_active {
            let settings = self.settings();
            self.write_pause_audit(
                uuid::Uuid::new_v4(),
                "resumed-manual",
                "tray-pause-resume",
                None,
                &settings,
                now,
            );
        }
        was_active
    }

    pub fn expire_pause(&self, now: SystemTime) -> bool {
        let mut pause_until = lock_unpoisoned(&self.pause_until);
        let expired = pause_until.is_some_and(|expires_at| expires_at <= now);
        if expired {
            *pause_until = None;
        }
        drop(pause_until);
        if expired {
            let settings = self.settings();
            self.write_pause_audit(
                uuid::Uuid::new_v4(),
                "resumed-timeout",
                "pause-expiry",
                None,
                &settings,
                now,
            );
        }
        expired
    }

    pub fn is_paused(&self, now: SystemTime) -> bool {
        self.pause_remaining(now).is_some()
    }

    pub fn current_pending_confirmation(&self) -> Option<PendingConfirmation> {
        self.state_machine.current()
    }

    pub fn extend_confirmation_deadline(&self, attempt_id: uuid::Uuid, duration: Duration) {
        self.state_machine.extend_deadline(attempt_id, duration);
    }

    fn clear_active_confirmation(&self, attempt_id: uuid::Uuid) {
        let mut active = lock_unpoisoned(&self.active_confirmation);
        if *active == Some(attempt_id) {
            *active = None;
        }
    }

    fn write_audit(
        &self,
        protected_chat_id: Option<uuid::Uuid>,
        trace_id: Option<uuid::Uuid>,
        event_type: impl Into<String>,
        result: impl Into<String>,
        timestamp: SystemTime,
    ) {
        let mut entry = AuditEntry::new(timestamp, protected_chat_id, event_type, result);
        if let Some(trace_id) = trace_id {
            entry = entry.with_trace_id(trace_id);
        }
        self.audit_log.write(entry);
    }

    fn write_trace(
        &self,
        protected_chat_id: Option<uuid::Uuid>,
        trace_id: uuid::Uuid,
        event_type: impl Into<String>,
        result: impl Into<String>,
        source: &str,
        timestamp: SystemTime,
    ) {
        let result = result.into();
        let mut details =
            std::collections::BTreeMap::from([("source".to_owned(), source.to_owned())]);
        if result == "context-unavailable"
            && let Some(diagnostics) = self.platform.current_diagnostics()
        {
            let context = self.platform.current();
            details.extend(diagnostics.audit_details(&context, timestamp));
        }
        self.audit_log.write(
            AuditEntry::new(timestamp, protected_chat_id, event_type, result)
                .with_trace_id(trace_id)
                .with_details(details),
        );
    }

    fn write_bypass_audit(&self, record: BypassAuditRecord<'_>) {
        let BypassAuditRecord {
            protected_chat_id,
            trace_id,
            result,
            requested_minutes,
            settings,
            context,
            context_source,
            decision,
            timestamp,
        } = record;
        let mut details = std::collections::BTreeMap::from([
            ("source".to_owned(), "tray-bypass".to_owned()),
            ("contextSource".to_owned(), context_source.to_owned()),
            ("requestedMinutes".to_owned(), requested_minutes.to_string()),
            (
                "ruleMode".to_owned(),
                rule_mode_audit_label(settings.rule_mode).to_owned(),
            ),
            ("protectionEnabled".to_owned(), settings.enabled.to_string()),
            (
                "trustedWeixin".to_owned(),
                context.is_trusted_weixin.to_string(),
            ),
            (
                "contextCompatibilityAvailable".to_owned(),
                context.is_compatibility_available.to_string(),
            ),
            (
                "contextMaximumAgeMilliseconds".to_owned(),
                TEMPORARY_BYPASS_CONTEXT_MAX_AGE.as_millis().to_string(),
            ),
            (
                "editorFocused".to_owned(),
                context.is_message_editor_focused.to_string(),
            ),
            ("knownChat".to_owned(), context.is_known_chat().to_string()),
        ]);
        if let Some(target_kind) = context.target_kind() {
            details.insert(
                "chatTargetKind".to_owned(),
                target_kind_audit_label(target_kind).to_owned(),
            );
        }
        if let Some(decision) = decision {
            details.insert(
                "decisionKind".to_owned(),
                decision_audit_label(decision).to_owned(),
            );
        }
        if let Some(observed_at) = context.observed_at
            && let Ok(age) = timestamp.duration_since(observed_at)
        {
            details.insert(
                "contextAgeMilliseconds".to_owned(),
                age.as_millis().to_string(),
            );
        }
        self.audit_log.write(
            AuditEntry::new(timestamp, protected_chat_id, "temporary-bypass", result)
                .with_trace_id(trace_id)
                .with_details(details),
        );
    }

    fn write_pause_audit(
        &self,
        trace_id: uuid::Uuid,
        result: &str,
        source: &str,
        requested_minutes: Option<u32>,
        settings: &AppSettings,
        timestamp: SystemTime,
    ) {
        let mut details = std::collections::BTreeMap::from([
            ("source".to_owned(), source.to_owned()),
            (
                "ruleMode".to_owned(),
                rule_mode_audit_label(settings.rule_mode).to_owned(),
            ),
            ("protectionEnabled".to_owned(), settings.enabled.to_string()),
        ]);
        if let Some(requested_minutes) = requested_minutes {
            details.insert("requestedMinutes".to_owned(), requested_minutes.to_string());
        }
        self.audit_log.write(
            AuditEntry::new(timestamp, None, "protection-pause", result)
                .with_trace_id(trace_id)
                .with_details(details),
        );
    }
}

struct ConfirmationRequest<'a> {
    context: ChatContext,
    decision: wechat_send_guard_core::ProtectionDecision,
    is_numpad_enter: bool,
    trace_id: uuid::Uuid,
    source: &'a str,
    timeout: Duration,
    now: SystemTime,
}

fn context_is_stale(context: &ChatContext, now: SystemTime) -> bool {
    context_exceeds_maximum_age(context, now, MAX_CONTEXT_AGE)
}

fn context_exceeds_maximum_age(
    context: &ChatContext,
    now: SystemTime,
    maximum_age: Duration,
) -> bool {
    let Some(observed_at) = context.observed_at else {
        // Test doubles and future platforms without a cache timestamp remain usable. Production
        // Windows contexts always carry a timestamp from the foreground monitor.
        return false;
    };

    match now.duration_since(observed_at) {
        Ok(age) => age > maximum_age,
        // Confirmation captures `now` before the synchronous platform revalidation. A successful
        // refresh can therefore carry an observation timestamp a few milliseconds later than
        // `now`; that is fresher, not stale.
        Err(error) => error.duration() > MAX_CONTEXT_FUTURE_LEAD,
    }
}

fn is_temporary_duration(minutes: u32) -> bool {
    matches!(minutes, 1 | 5 | 15)
}

const fn rule_mode_audit_label(mode: RuleMode) -> &'static str {
    match mode {
        RuleMode::ProtectListed => "protect-listed",
        RuleMode::ConfirmUnlessExcluded => "confirm-unless-excluded",
    }
}

const fn target_kind_audit_label(kind: ChatTargetKind) -> &'static str {
    match kind {
        ChatTargetKind::Group => "group",
        ChatTargetKind::Contact => "contact",
    }
}

const fn decision_audit_label(kind: ProtectionDecisionKind) -> &'static str {
    match kind {
        ProtectionDecisionKind::Pass => "pass",
        ProtectionDecisionKind::ConfirmProtected => "confirm-protected",
        ProtectionDecisionKind::ConfirmUnlisted => "confirm-unlisted",
        ProtectionDecisionKind::ConfirmUnknown => "confirm-unknown",
        ProtectionDecisionKind::BlockUnknown => "block-unknown",
    }
}

fn outcome_name(outcome: ConfirmationOutcome) -> &'static str {
    match outcome {
        ConfirmationOutcome::Confirmed => "confirmed",
        ConfirmationOutcome::Cancelled => "cancelled",
        ConfirmationOutcome::TimedOut => "timedout",
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
