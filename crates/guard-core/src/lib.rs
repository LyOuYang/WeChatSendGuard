#![forbid(unsafe_code)]
//! Pure domain logic for WeChatSendGuard.
//!
//! This crate has no operating-system, UI, process, network, or Weixin dependency.
//! It is safe to exercise entirely with synthetic contexts in automated tests.

pub mod audit;
pub mod config;
pub mod guard;

pub use audit::AuditEntry;
pub use config::{
    AppSettings, CURRENT_SCHEMA_VERSION, ChatTargetKind, ConfirmationMode, ConfirmationSettings,
    FileSettingsStore, ImportError, ProtectedChat, RuleMode, UnknownContextBehavior,
    export_protected_chats, import_protected_chats, normalize_title, sanitize_chat_list,
};
pub use guard::{
    ChatContext, ConfirmationOutcome, ConfirmationResolution, PendingConfirmation,
    ProtectionDecision, ProtectionDecisionKind, SendGuardStateMachine, TemporaryBypassRegistry,
    evaluate_protection,
};
