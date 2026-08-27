use std::time::SystemTime;
use uuid::Uuid;

/// Minimal, privacy-preserving audit data. Platform storage decides how to serialize time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub timestamp: SystemTime,
    pub protected_chat_id: Option<Uuid>,
    pub event_type: String,
    pub result: String,
}

impl AuditEntry {
    pub fn new(
        timestamp: SystemTime,
        protected_chat_id: Option<Uuid>,
        event_type: impl Into<String>,
        result: impl Into<String>,
    ) -> Self {
        Self {
            timestamp,
            protected_chat_id,
            event_type: event_type.into(),
            result: result.into(),
        }
    }
}
