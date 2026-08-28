use std::{collections::BTreeMap, time::SystemTime};
use uuid::Uuid;

/// Minimal, privacy-preserving diagnostic data. Platform storage decides how to serialize time.
/// `details` must contain only explicitly allow-listed, content-free diagnostic values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub timestamp: SystemTime,
    pub trace_id: Option<Uuid>,
    pub protected_chat_id: Option<Uuid>,
    pub event_type: String,
    pub result: String,
    pub details: BTreeMap<String, String>,
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
            trace_id: None,
            protected_chat_id,
            event_type: event_type.into(),
            result: result.into(),
            details: BTreeMap::new(),
        }
    }

    pub fn with_trace_id(mut self, trace_id: Uuid) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    pub fn with_details<K, V, I>(mut self, details: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.details = details
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }
}
