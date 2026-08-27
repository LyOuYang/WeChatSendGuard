#![forbid(unsafe_code)]
//! OS boundary contracts for WeChatSendGuard.
//!
//! Production platform adapters implement these traits. The built-in test doubles
//! deliberately have no process discovery, UI Automation, hook, or input-injection path.

use std::{
    fmt,
    sync::{Mutex, MutexGuard},
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

/// Returns only a read-only foreground context snapshot. Implementations must never infer
/// identity from screenshots, private client data, injected code, or process memory.
pub trait ChatContextProvider: Send + Sync {
    fn current(&self) -> ChatContext;
    fn refresh_now(&self) -> PlatformResult<ChatContext>;
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
}

impl FakeChatContextProvider {
    pub fn new(context: ChatContext) -> Self {
        Self {
            current: Mutex::new(context),
        }
    }

    pub fn set_current(&self, context: ChatContext) {
        *lock_unpoisoned(&self.current) = context;
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
        Ok(None)
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
