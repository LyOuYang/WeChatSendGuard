use crate::config::{
    AppSettings, ChatTargetKind, ProtectedChat, RuleMode, UnknownContextBehavior, normalize_title,
};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime},
};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatContext {
    pub window_handle: isize,
    pub process_id: u32,
    pub process_path: String,
    pub is_trusted_weixin: bool,
    pub requires_elevation: bool,
    pub is_compatibility_available: bool,
    pub is_message_editor_focused: bool,
    pub is_group_chat: bool,
    pub is_contact_chat: bool,
    pub chat_title: Option<String>,
    pub generation: u64,
    pub observed_at: Option<SystemTime>,
}

impl ChatContext {
    pub fn inactive() -> Self {
        Self::default()
    }

    pub fn is_known_chat(&self) -> bool {
        self.is_group_chat || self.is_contact_chat
    }

    pub fn target_kind(&self) -> Option<ChatTargetKind> {
        if self.is_group_chat {
            Some(ChatTargetKind::Group)
        } else if self.is_contact_chat {
            Some(ChatTargetKind::Contact)
        } else {
            None
        }
    }

    pub fn normalized_chat_title(&self) -> String {
        self.chat_title
            .as_deref()
            .map(normalize_title)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionDecisionKind {
    Pass,
    ConfirmProtected,
    ConfirmUnlisted,
    ConfirmUnknown,
    BlockUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionDecision {
    pub kind: ProtectionDecisionKind,
    pub protected_chat: Option<ProtectedChat>,
}

impl ProtectionDecision {
    pub const fn pass() -> Self {
        Self {
            kind: ProtectionDecisionKind::Pass,
            protected_chat: None,
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self.kind,
            ProtectionDecisionKind::ConfirmProtected
                | ProtectionDecisionKind::ConfirmUnlisted
                | ProtectionDecisionKind::ConfirmUnknown
        )
    }

    pub fn should_suppress(&self) -> bool {
        self.kind != ProtectionDecisionKind::Pass
    }
}

pub fn evaluate_protection(
    context: &ChatContext,
    settings: &AppSettings,
    bypasses: &TemporaryBypassRegistry,
    now: SystemTime,
) -> ProtectionDecision {
    if !settings.enabled
        || !context.is_trusted_weixin
        || !context.is_compatibility_available
        || !context.is_message_editor_focused
    {
        return ProtectionDecision::pass();
    }

    evaluate_chat_protection(context, settings, bypasses, now)
}

/// Evaluates the protection rule for a recognized chat without requiring the message editor to be
/// focused. The send path still uses [`evaluate_protection`] so an unfocused editor cannot trigger
/// a confirmation, while the UI can show the rule result for the currently visible chat.
pub fn evaluate_chat_protection(
    context: &ChatContext,
    settings: &AppSettings,
    bypasses: &TemporaryBypassRegistry,
    now: SystemTime,
) -> ProtectionDecision {
    if !settings.enabled || !context.is_trusted_weixin || !context.is_compatibility_available {
        return ProtectionDecision::pass();
    }

    let title = context.normalized_chat_title();
    let Some(target_kind) = context.target_kind() else {
        return unknown_context_decision(settings);
    };
    if !context.is_known_chat() || title.is_empty() {
        return unknown_context_decision(settings);
    }

    if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        let exemption = settings.exempted_chats.iter().find(|chat| {
            chat.enabled && chat.target_kind == target_kind && title_matches(chat, &title)
        });
        return if exemption.is_some() || bypasses.is_active_for_context(context, now) {
            ProtectionDecision::pass()
        } else {
            ProtectionDecision {
                kind: ProtectionDecisionKind::ConfirmUnlisted,
                protected_chat: None,
            }
        };
    }

    let protected_chat = settings.protected_chats.iter().find(|chat| {
        chat.enabled && chat.target_kind == target_kind && title_matches(chat, &title)
    });
    match protected_chat {
        Some(chat) if !bypasses.is_active(chat.id, now) => ProtectionDecision {
            kind: ProtectionDecisionKind::ConfirmProtected,
            protected_chat: Some(chat.clone()),
        },
        _ => ProtectionDecision::pass(),
    }
}

pub fn title_matches(chat: &ProtectedChat, normalized_title: &str) -> bool {
    chat.match_title == normalized_title
        || chat.aliases.iter().any(|alias| alias == normalized_title)
}

fn unknown_context_decision(settings: &AppSettings) -> ProtectionDecision {
    let kind = match settings.unknown_context_behavior {
        UnknownContextBehavior::Confirm => ProtectionDecisionKind::ConfirmUnknown,
        UnknownContextBehavior::Block => ProtectionDecisionKind::BlockUnknown,
    };
    ProtectionDecision {
        kind,
        protected_chat: None,
    }
}

#[derive(Debug, Default)]
pub struct TemporaryBypassRegistry {
    entries: Mutex<HashMap<TemporaryBypassKey, SystemTime>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TemporaryBypassKey {
    ProtectedChat(Uuid),
    CurrentContext {
        window_handle: isize,
        process_id: u32,
        target_kind: ChatTargetKind,
        normalized_title: String,
    },
}

impl TemporaryBypassRegistry {
    pub fn grant(&self, protected_chat_id: Uuid, duration: Duration, now: SystemTime) {
        self.grant_key(
            TemporaryBypassKey::ProtectedChat(protected_chat_id),
            duration,
            now,
        );
    }

    pub fn grant_for_context(&self, context: &ChatContext, duration: Duration, now: SystemTime) {
        if let Some(key) = current_context_key(context) {
            self.grant_key(key, duration, now);
        }
    }

    fn grant_key(&self, key: TemporaryBypassKey, duration: Duration, now: SystemTime) {
        assert!(
            duration > Duration::ZERO,
            "temporary bypass duration must be positive"
        );
        let mut entries = lock_unpoisoned(&self.entries);
        entries.insert(key, now + duration);
    }

    pub fn is_active(&self, protected_chat_id: Uuid, now: SystemTime) -> bool {
        self.active_expiry(&TemporaryBypassKey::ProtectedChat(protected_chat_id), now)
            .is_some()
    }

    pub fn is_active_for_context(&self, context: &ChatContext, now: SystemTime) -> bool {
        current_context_key(context)
            .as_ref()
            .is_some_and(|key| self.active_expiry(key, now).is_some())
    }

    pub fn remaining(&self, protected_chat_id: Uuid, now: SystemTime) -> Option<Duration> {
        self.active_expiry(&TemporaryBypassKey::ProtectedChat(protected_chat_id), now)
            .and_then(|expires_at| expires_at.duration_since(now).ok())
    }

    pub fn remaining_for_context(
        &self,
        context: &ChatContext,
        now: SystemTime,
    ) -> Option<Duration> {
        current_context_key(context)
            .as_ref()
            .and_then(|key| self.active_expiry(key, now))
            .and_then(|expires_at| expires_at.duration_since(now).ok())
    }

    fn active_expiry(&self, key: &TemporaryBypassKey, now: SystemTime) -> Option<SystemTime> {
        let mut entries = lock_unpoisoned(&self.entries);
        let expires_at = entries.get(key).copied()?;
        if expires_at > now {
            return Some(expires_at);
        }
        entries.remove(key);
        None
    }

    pub fn expiry(&self, protected_chat_id: Uuid, now: SystemTime) -> Option<SystemTime> {
        self.active_expiry(&TemporaryBypassKey::ProtectedChat(protected_chat_id), now)
    }

    pub fn clear(&self, protected_chat_id: Uuid) {
        lock_unpoisoned(&self.entries)
            .remove(&TemporaryBypassKey::ProtectedChat(protected_chat_id));
    }
}

fn current_context_key(context: &ChatContext) -> Option<TemporaryBypassKey> {
    let target_kind = context.target_kind()?;
    let normalized_title = context.normalized_chat_title();
    if normalized_title.is_empty() {
        return None;
    }

    Some(TemporaryBypassKey::CurrentContext {
        window_handle: context.window_handle,
        process_id: context.process_id,
        target_kind,
        normalized_title,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationOutcome {
    Confirmed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConfirmation {
    pub attempt_id: Uuid,
    /// One opaque ID ties the complete content-free diagnostic chain to this send attempt.
    pub trace_id: Uuid,
    pub original_context: ChatContext,
    pub decision: ProtectionDecision,
    pub is_numpad_enter: bool,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
    /// Ephemeral UI-only data. Callers must not persist or audit it.
    pub draft_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationResolution {
    pub should_inject: bool,
    pub reason: &'static str,
}

impl ConfirmationResolution {
    pub const ACCEPTED: Self = Self {
        should_inject: true,
        reason: "Confirmed",
    };

    fn rejected(reason: &'static str) -> Self {
        Self {
            should_inject: false,
            reason,
        }
    }
}

#[derive(Debug, Default)]
pub struct SendGuardStateMachine {
    pending: Mutex<Option<PendingConfirmation>>,
}

impl SendGuardStateMachine {
    pub fn current(&self) -> Option<PendingConfirmation> {
        lock_unpoisoned(&self.pending).clone()
    }

    pub fn try_begin(
        &self,
        context: ChatContext,
        decision: ProtectionDecision,
        is_numpad_enter: bool,
        trace_id: Uuid,
        timeout: Duration,
        now: SystemTime,
    ) -> Option<PendingConfirmation> {
        let mut current = lock_unpoisoned(&self.pending);
        if current.is_some() || !decision.requires_confirmation() {
            return None;
        }

        let pending = PendingConfirmation {
            attempt_id: Uuid::new_v4(),
            trace_id,
            original_context: context,
            decision,
            is_numpad_enter,
            created_at: now,
            expires_at: now + timeout,
            draft_preview: None,
        };
        *current = Some(pending.clone());
        Some(pending)
    }

    pub fn resolve(
        &self,
        attempt_id: Uuid,
        outcome: ConfirmationOutcome,
        revalidated_context: &ChatContext,
        now: SystemTime,
    ) -> ConfirmationResolution {
        let pending = {
            let mut current = lock_unpoisoned(&self.pending);
            match current.as_ref() {
                Some(pending) if pending.attempt_id == attempt_id => current.take(),
                _ => {
                    return ConfirmationResolution::rejected(
                        "The confirmation is no longer active.",
                    );
                }
            }
        };
        let pending = pending.expect("matching pending confirmation must exist");

        if outcome != ConfirmationOutcome::Confirmed {
            return match outcome {
                ConfirmationOutcome::Cancelled => ConfirmationResolution::rejected("Cancelled"),
                ConfirmationOutcome::TimedOut => ConfirmationResolution::rejected("TimedOut"),
                ConfirmationOutcome::Confirmed => unreachable!(),
            };
        }
        if now > pending.expires_at {
            return ConfirmationResolution::rejected("Confirmation timed out.");
        }
        if !Self::represents_same_send_target(&pending.original_context, revalidated_context) {
            return ConfirmationResolution::rejected(
                "The chat changed before confirmation completed.",
            );
        }

        ConfirmationResolution::ACCEPTED
    }

    pub fn cancel_active(&self) {
        *lock_unpoisoned(&self.pending) = None;
    }

    pub fn extend_deadline(&self, attempt_id: Uuid, duration: Duration) {
        if duration.is_zero() {
            return;
        }
        let mut current = lock_unpoisoned(&self.pending);
        if let Some(pending) = current.as_mut()
            && pending.attempt_id == attempt_id
        {
            pending.expires_at += duration;
        }
    }

    pub fn represents_same_send_target(original: &ChatContext, current: &ChatContext) -> bool {
        Self::represents_same_session(original, current) && current.is_message_editor_focused
    }

    pub fn represents_same_session(original: &ChatContext, current: &ChatContext) -> bool {
        if !current.is_trusted_weixin || !current.is_compatibility_available {
            return false;
        }
        if !original.is_known_chat()
            || !current.is_known_chat()
            || original.target_kind() != current.target_kind()
        {
            return false;
        }
        if original.window_handle != current.window_handle
            || original.process_id != current.process_id
        {
            return false;
        }

        let original_title = original.normalized_chat_title();
        !original_title.is_empty() && original_title == current.normalized_chat_title()
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
