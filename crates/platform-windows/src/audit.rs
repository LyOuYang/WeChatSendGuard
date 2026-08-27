use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU32, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::UNIX_EPOCH,
};
use wechat_send_guard_core::AuditEntry;
use wechat_send_guard_platform_api::AuditLog;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_QUEUED_AUDIT_ENTRIES: usize = 256;
const MIN_RETENTION_DAYS: u32 = 1;
const MAX_RETENTION_DAYS: u32 = 30;

/// Best-effort asynchronous JSON Lines audit writer. It deliberately stores only opaque chat
/// identifiers and operation results, never a draft or a chat title.
pub struct WindowsAuditLog {
    sender: Mutex<Option<SyncSender<AuditEntry>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    retention_days: Arc<AtomicU32>,
}

impl WindowsAuditLog {
    pub fn new(log_directory: impl Into<PathBuf>, retention_days: u32) -> io::Result<Self> {
        let log_directory = log_directory.into();
        fs::create_dir_all(&log_directory)?;

        let retention_days = Arc::new(AtomicU32::new(clamp_retention(retention_days)));
        let worker_retention = Arc::clone(&retention_days);
        let (sender, receiver) = sync_channel(MAX_QUEUED_AUDIT_ENTRIES);
        let worker = thread::Builder::new()
            .name("wechat-send-guard-audit".to_owned())
            .spawn(move || {
                while let Ok(entry) = receiver.recv() {
                    let days = worker_retention.load(Ordering::Acquire);
                    let _ = append_audit_entry(&log_directory, &entry, days);
                }
            })?;

        Ok(Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
            retention_days,
        })
    }

    pub fn set_retention_days(&self, retention_days: u32) {
        self.retention_days
            .store(clamp_retention(retention_days), Ordering::Release);
    }

    /// Stops the worker after already-queued entries are handled. This is intended for normal
    /// desktop-app shutdown; a full queue never blocks the keyboard hook on write.
    pub fn shutdown(&self) {
        let sender = lock_unpoisoned(&self.sender).take();
        drop(sender);

        if let Some(worker) = lock_unpoisoned(&self.worker).take() {
            let _ = worker.join();
        }
    }
}

impl AuditLog for WindowsAuditLog {
    fn write(&self, entry: AuditEntry) {
        let sender = lock_unpoisoned(&self.sender);
        if let Some(sender) = sender.as_ref() {
            // The audit trail is diagnostic only. Dropping an entry under sustained disk pressure
            // is preferable to delaying a low-level keyboard callback.
            let _ = sender.try_send(entry);
        }
    }
}

impl Drop for WindowsAuditLog {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn default_audit_log_directory(local_app_data: &Path) -> PathBuf {
    local_app_data.join("WeChatSendGuard").join("logs")
}

fn append_audit_entry(
    log_directory: &Path,
    entry: &AuditEntry,
    retention_days: u32,
) -> io::Result<()> {
    fs::create_dir_all(log_directory)?;
    let log_path = log_directory.join(audit_file_name());
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(serialize_entry(entry).as_bytes())?;
    file.write_all(b"\n")?;
    enforce_retention(log_directory, retention_days)
}

fn serialize_entry(entry: &AuditEntry) -> String {
    let timestamp_unix_milliseconds = entry
        .timestamp
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    json!({
        "timestampUnixMilliseconds": timestamp_unix_milliseconds,
        "protectedChatId": entry.protected_chat_id.map(|id| id.to_string()),
        "eventType": entry.event_type,
        "result": entry.result,
    })
    .to_string()
}

fn audit_file_name() -> String {
    // SAFETY: GetLocalTime fills and returns a plain SYSTEMTIME value with no borrowed state.
    let now = unsafe { GetLocalTime() };
    format!(
        "audit-{:04}-{:02}-{:02}.jsonl",
        now.wYear, now.wMonth, now.wDay
    )
}

fn enforce_retention(log_directory: &Path, retention_days: u32) -> io::Result<()> {
    let mut files = fs::read_dir(log_directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_audit_log(path))
        .collect::<Vec<_>>();
    files.sort();

    let remove_count = files
        .len()
        .saturating_sub(clamp_retention(retention_days) as usize);
    for path in files.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn is_audit_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("audit-")
                && name.ends_with(".jsonl")
                && name.len() == "audit-YYYY-MM-DD.jsonl".len()
        })
}

fn clamp_retention(retention_days: u32) -> u32 {
    retention_days.clamp(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{clamp_retention, is_audit_log, serialize_entry};
    use wechat_send_guard_core::AuditEntry;

    #[test]
    fn audit_serialization_contains_only_minimal_fields() {
        let entry = AuditEntry::new(
            UNIX_EPOCH + Duration::from_millis(42),
            None,
            "send",
            "injected",
        );
        let serialized = serialize_entry(&entry);

        assert!(serialized.contains("timestampUnixMilliseconds"));
        assert!(serialized.contains("eventType"));
        assert!(serialized.contains("result"));
        assert!(!serialized.contains("chatTitle"));
        assert!(!serialized.contains("draft"));
    }

    #[test]
    fn retention_and_file_filters_are_bounded_and_narrow() {
        assert_eq!(clamp_retention(0), 1);
        assert_eq!(clamp_retention(99), 30);
        assert!(is_audit_log(std::path::Path::new("audit-2026-08-27.jsonl")));
        assert!(!is_audit_log(std::path::Path::new("settings.json")));
    }
}
