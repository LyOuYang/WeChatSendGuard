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
    AppSettings, AuditEntry, ChatContext, ConfirmationOutcome, PendingConfirmation, ProtectedChat,
    ProtectionDecisionKind, SendGuardStateMachine, TemporaryBypassRegistry, evaluate_protection,
};
use wechat_send_guard_platform_api::{AuditLog, InputInjector, SendTargetPlatform};

/// The Windows foreground monitor publishes a context every 75 ms. A key callback may only
/// make a send decision from a recent snapshot of the same foreground window. Keeping this
/// bound small makes a just-switched chat fail closed while preserving a responsive hook path.
const MAX_CONTEXT_AGE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalEnter {
    pub is_numpad_enter: bool,
    pub is_injected: bool,
    pub shift_pressed: bool,
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

/// The only path from a physical key decision to a synthetic send request.
pub struct GuardService {
    settings: RwLock<AppSettings>,
    state_machine: SendGuardStateMachine,
    bypasses: TemporaryBypassRegistry,
    platform: Arc<dyn SendTargetPlatform>,
    injector: Arc<dyn InputInjector>,
    audit_log: Arc<dyn AuditLog>,
    active_confirmation: Mutex<Option<uuid::Uuid>>,
}

impl GuardService {
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

        if context.window_handle != enter.foreground_window
            || !context.is_compatibility_available
            || !context.is_message_editor_focused
        {
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

        if context_is_stale(&context, now) {
            self.write_trace(None, trace_id, "send-blocked", "stale-context", source, now);
            return EnterHandling::SuppressBlockedUnknown;
        }

        let decision = evaluate_protection(&context, &settings, &self.bypasses, now);
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
            self.write_trace(None, trace_id, "send-blocked", "unknown-chat", source, now);
            return EnterHandling::SuppressBlockedUnknown;
        }

        let timeout = Duration::from_secs(u64::from(settings.confirmation.timeout_seconds));
        match self.state_machine.try_begin(
            context,
            decision,
            enter.is_numpad_enter,
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
        if !settings.enabled || !settings.intercept_send_button {
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
        if context.window_handle != foreground_window
            || !context.is_trusted_weixin
            || !context.is_compatibility_available
            || !context.is_message_editor_focused
        {
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

        if context_is_stale(&context, now) {
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

        let decision = evaluate_protection(&context, &settings, &self.bypasses, now);
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
        match self
            .state_machine
            .try_begin(context, decision, false, trace_id, timeout, now)
        {
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
                    "send-button-blocked",
                    "confirmation-already-active",
                    source,
                    now,
                );
                EnterHandling::SuppressWhileConfirmationActive
            }
        }
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
            return pending.clone();
        }

        let mut enriched = pending.clone();
        enriched.draft_preview = self
            .platform
            .read_draft_preview(&pending.original_context)
            .ok()
            .flatten();
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

    pub fn try_grant_current_bypass(&self, minutes: u32, now: SystemTime) -> Option<ProtectedChat> {
        if !matches!(minutes, 1 | 5 | 15) {
            return None;
        }
        let context = self.platform.current();
        let decision = evaluate_protection(&context, &self.settings(), &self.bypasses, now);
        let protected_chat = decision.protected_chat?;
        if decision.kind != ProtectionDecisionKind::ConfirmProtected {
            return None;
        }

        self.bypasses.grant(
            protected_chat.id,
            Duration::from_secs(u64::from(minutes) * 60),
            now,
        );
        self.write_audit(
            Some(protected_chat.id),
            None,
            "temporary-bypass",
            format!("granted-{minutes}m"),
            now,
        );
        Some(protected_chat)
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
        self.audit_log.write(
            AuditEntry::new(timestamp, protected_chat_id, event_type, result)
                .with_trace_id(trace_id)
                .with_details([("source", source)]),
        );
    }
}

fn context_is_stale(context: &ChatContext, now: SystemTime) -> bool {
    let Some(observed_at) = context.observed_at else {
        // Test doubles and future platforms without a cache timestamp remain usable. Production
        // Windows contexts always carry a timestamp from the foreground monitor.
        return false;
    };

    now.duration_since(observed_at)
        .map_or(true, |age| age > MAX_CONTEXT_AGE)
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
