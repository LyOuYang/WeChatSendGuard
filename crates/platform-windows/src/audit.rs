use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;
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
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use wechat_send_guard_core::AuditEntry;
use wechat_send_guard_platform_api::AuditLog;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_QUEUED_AUDIT_ENTRIES: usize = 256;
const MIN_RETENTION_DAYS: u32 = 1;
const MAX_RETENTION_DAYS: u32 = 30;
const MAX_LOG_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LOG_DIRECTORY_BYTES: u64 = 50 * 1024 * 1024;
const AUDIT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug)]
struct AuditMetadata {
    application_version: String,
    weixin_version: String,
    session_id: uuid::Uuid,
    process_id: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedAuditEntry<'a> {
    local_time: String,
    application_version: &'a str,
    weixin_version: &'a str,
    schema_version: u32,
    session_id: String,
    process_id: u32,
    event_type: &'a str,
    result: &'a str,
    timestamp_unix_milliseconds: u128,
    trace_id: Option<String>,
    protected_chat_id: Option<String>,
    details: std::collections::BTreeMap<String, String>,
}

enum AuditCommand {
    Entry(AuditEntry),
    Flush(SyncSender<io::Result<()>>),
    Clear(SyncSender<io::Result<()>>),
    Cleanup(SyncSender<io::Result<()>>),
}

/// Best-effort asynchronous JSON Lines writer. It stores only opaque chat IDs and an
/// allow-listed, content-free diagnostic vocabulary; it never writes a draft or chat title.
pub struct WindowsAuditLog {
    sender: Mutex<Option<SyncSender<AuditCommand>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    retention_days: Arc<AtomicU32>,
    metadata: Arc<Mutex<AuditMetadata>>,
    log_directory: PathBuf,
}

impl WindowsAuditLog {
    pub fn new(log_directory: impl Into<PathBuf>, retention_days: u32) -> io::Result<Self> {
        Self::new_with_versions(log_directory, retention_days, "unknown", None)
    }

    pub fn new_with_versions(
        log_directory: impl Into<PathBuf>,
        retention_days: u32,
        application_version: impl Into<String>,
        weixin_version: Option<String>,
    ) -> io::Result<Self> {
        let log_directory = log_directory.into();
        fs::create_dir_all(&log_directory)?;

        let retention_days = Arc::new(AtomicU32::new(clamp_retention(retention_days)));
        enforce_retention_and_capacity(
            &log_directory,
            retention_days.load(Ordering::Acquire),
            None,
        )?;
        let worker_retention = Arc::clone(&retention_days);
        let metadata = Arc::new(Mutex::new(AuditMetadata {
            application_version: application_version.into(),
            weixin_version: weixin_version.unwrap_or_else(|| "未能读取".to_owned()),
            session_id: uuid::Uuid::new_v4(),
            process_id: std::process::id(),
        }));
        let worker_metadata = Arc::clone(&metadata);
        let (sender, receiver) = sync_channel(MAX_QUEUED_AUDIT_ENTRIES);
        let worker = thread::Builder::new()
            .name("wechat-send-guard-audit".to_owned())
            .spawn({
                let log_directory = log_directory.clone();
                move || worker_loop(receiver, &log_directory, worker_retention, worker_metadata)
            })?;

        Ok(Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
            retention_days,
            metadata,
            log_directory,
        })
    }

    pub fn set_weixin_version(&self, weixin_version: Option<String>) {
        lock_unpoisoned(&self.metadata).weixin_version =
            weixin_version.unwrap_or_else(|| "未能读取".to_owned());
    }

    pub fn session_id(&self) -> uuid::Uuid {
        lock_unpoisoned(&self.metadata).session_id
    }

    pub fn log_directory(&self) -> &Path {
        &self.log_directory
    }

    pub fn set_retention_days(&self, retention_days: u32) {
        self.retention_days
            .store(clamp_retention(retention_days), Ordering::Release);
    }

    pub fn flush(&self) -> io::Result<()> {
        self.request(AuditCommand::Flush)
    }

    pub fn clear(&self) -> io::Result<()> {
        self.request(AuditCommand::Clear)
    }

    pub fn cleanup(&self) -> io::Result<()> {
        self.request(AuditCommand::Cleanup)
    }

    /// Stops the worker after already-queued entries are handled. This is intended for normal
    /// desktop-app shutdown; a full queue never blocks a low-level callback on disk I/O.
    pub fn shutdown(&self) {
        let sender = lock_unpoisoned(&self.sender).take();
        drop(sender);

        if let Some(worker) = lock_unpoisoned(&self.worker).take() {
            let _ = worker.join();
        }
    }

    fn request(
        &self,
        command: impl FnOnce(SyncSender<io::Result<()>>) -> AuditCommand,
    ) -> io::Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(1);
        let sender = lock_unpoisoned(&self.sender)
            .as_ref()
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "audit worker is stopped"))?;
        sender
            .send(command(reply_sender))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "audit worker is stopped"))?;
        reply_receiver.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "audit worker did not return a result",
            )
        })?
    }
}

impl AuditLog for WindowsAuditLog {
    fn write(&self, entry: AuditEntry) {
        let sender = lock_unpoisoned(&self.sender);
        if let Some(sender) = sender.as_ref() {
            // Diagnostic writes must never delay a low-level physical input callback.
            let _ = sender.try_send(AuditCommand::Entry(entry));
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

fn worker_loop(
    receiver: std::sync::mpsc::Receiver<AuditCommand>,
    log_directory: &Path,
    retention_days: Arc<AtomicU32>,
    metadata: Arc<Mutex<AuditMetadata>>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            AuditCommand::Entry(entry) => {
                let days = retention_days.load(Ordering::Acquire);
                let _ = append_audit_entry(log_directory, &entry, days, &metadata);
            }
            AuditCommand::Flush(reply) => {
                let _ = reply.send(Ok(()));
            }
            AuditCommand::Clear(reply) => {
                let _ = reply.send(clear_audit_logs(log_directory));
            }
            AuditCommand::Cleanup(reply) => {
                let days = retention_days.load(Ordering::Acquire);
                let _ = reply.send(enforce_retention_and_capacity(log_directory, days, None));
            }
        }
    }
}

fn append_audit_entry(
    log_directory: &Path,
    entry: &AuditEntry,
    retention_days: u32,
    metadata: &Mutex<AuditMetadata>,
) -> io::Result<()> {
    fs::create_dir_all(log_directory)?;
    let log_path = next_audit_log_path(log_directory)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    file.write_all(serialize_entry(entry, metadata).as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    enforce_retention_and_capacity(log_directory, retention_days, Some(&log_path))
}

fn serialize_entry(entry: &AuditEntry, metadata: &Mutex<AuditMetadata>) -> String {
    let timestamp_unix_milliseconds = entry
        .timestamp
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let details = entry
        .details
        .iter()
        .filter(|(key, value)| is_safe_detail(key, value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let metadata = lock_unpoisoned(metadata);
    let local_time: DateTime<Local> = entry.timestamp.into();
    serde_json::to_string(&SerializedAuditEntry {
        local_time: local_time.to_rfc3339_opts(SecondsFormat::Millis, false),
        application_version: &metadata.application_version,
        weixin_version: &metadata.weixin_version,
        schema_version: AUDIT_SCHEMA_VERSION,
        session_id: metadata.session_id.to_string(),
        process_id: metadata.process_id,
        event_type: &entry.event_type,
        result: &entry.result,
        timestamp_unix_milliseconds,
        trace_id: entry.trace_id.map(|id| id.to_string()),
        protected_chat_id: entry.protected_chat_id.map(|id| id.to_string()),
        details,
    })
    .unwrap_or_else(|_| {
        "{\"eventType\":\"diagnostics\",\"result\":\"serialization-failed\"}".to_owned()
    })
}

fn is_safe_detail(key: &str, value: &str) -> bool {
    matches!(
        key,
        "source"
            | "applicationVersion"
            | "operatingSystem"
            | "architecture"
            | "weixinVersion"
            | "trustedWeixinExecutable"
            | "trustedWeixinIdentity"
            | "releaseVersion"
            | "checkMode"
            | "contextSource"
            | "requestedMinutes"
            | "ruleMode"
            | "protectionEnabled"
            | "knownChat"
            | "chatTargetKind"
            | "decisionKind"
            | "observationId"
            | "diagnosticWindowMatchesContext"
            | "foregroundProcessId"
            | "processPathAvailable"
            | "trustedWeixin"
            | "requiresElevation"
            | "uiaStatus"
            | "uiaErrorCode"
            | "uiaRootAvailable"
            | "uiaRootClassName"
            | "uiaRootControlType"
            | "uiaRootChildCount"
            | "uiaProviderKind"
            | "uiaTreeQueryStatus"
            | "uiaTreeErrorCode"
            | "uiaTreeDescendantCount"
            | "uiaTreeSampledCount"
            | "uiaTreeSampleTruncated"
            | "uiaTreeControlTypeCounts"
            | "uiaTreeAutomationIdReadableCount"
            | "uiaTreeAutomationIdNonemptyCount"
            | "uiaTreeClassNameReadableCount"
            | "uiaTreeClassNameNonemptyCount"
            | "uiaTreePropertyReadFailureCount"
            | "editorFound"
            | "editorQueryStatus"
            | "editorQueryErrorCode"
            | "editorCandidateCount"
            | "chatTitleElementFound"
            | "chatTitleQueryStatus"
            | "chatTitleQueryErrorCode"
            | "chatTitleCandidateCount"
            | "chatTitleReadable"
            | "groupTitleFound"
            | "groupTitleQueryStatus"
            | "groupTitleQueryErrorCode"
            | "groupTitleCandidateCount"
            | "chatBranchFound"
            | "editorFocused"
            | "sendButtonInspected"
            | "sendToolbarCount"
            | "sendButtonCandidateCount"
            | "sendButtonState"
            | "uiaScanDurationMilliseconds"
            | "contextCompatibilityAvailable"
            | "contextGeneration"
            | "contextAgeMilliseconds"
            | "contextMaximumAgeMilliseconds"
    ) && value.len() <= 256
        && !value.contains(['\r', '\n'])
}

fn next_audit_log_path(log_directory: &Path) -> io::Result<PathBuf> {
    let now = local_date_stamp();
    for sequence in 0..10_000u32 {
        let path = log_directory.join(audit_file_name(now, sequence));
        if !path.exists() || fs::metadata(&path)?.len() < MAX_LOG_FILE_BYTES {
            return Ok(path);
        }
    }
    Err(io::Error::other("too many audit log rotations for one day"))
}

fn local_date_stamp() -> (u16, u16, u16) {
    // SAFETY: GetLocalTime fills and returns a plain SYSTEMTIME value with no borrowed state.
    let now = unsafe { GetLocalTime() };
    (now.wYear, now.wMonth, now.wDay)
}

fn audit_file_name((year, month, day): (u16, u16, u16), sequence: u32) -> String {
    if sequence == 0 {
        format!("audit-{year:04}-{month:02}-{day:02}.jsonl")
    } else {
        format!("audit-{year:04}-{month:02}-{day:02}-{sequence:02}.jsonl")
    }
}

fn enforce_retention_and_capacity(
    log_directory: &Path,
    retention_days: u32,
    active_path: Option<&Path>,
) -> io::Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            u64::from(clamp_retention(retention_days)) * 86_400,
        ))
        .unwrap_or(UNIX_EPOCH);
    let mut files = audit_log_files(log_directory)?;
    for file in &files {
        if is_active_path(file, active_path) {
            continue;
        }
        if file_modified(file).is_some_and(|modified| modified < cutoff) {
            let _ = fs::remove_file(file);
        }
    }

    files = audit_log_files(log_directory)?;
    files.sort_by_key(|path| (file_modified(path), path.clone()));
    let mut total_bytes = files
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .sum::<u64>();
    for file in files {
        if total_bytes <= MAX_LOG_DIRECTORY_BYTES {
            break;
        }
        if is_active_path(&file, active_path) {
            continue;
        }
        if let Ok(metadata) = fs::metadata(&file)
            && fs::remove_file(&file).is_ok()
        {
            total_bytes = total_bytes.saturating_sub(metadata.len());
        }
    }
    Ok(())
}

fn clear_audit_logs(log_directory: &Path) -> io::Result<()> {
    for file in audit_log_files(log_directory)? {
        fs::remove_file(file)?;
    }
    Ok(())
}

fn audit_log_files(log_directory: &Path) -> io::Result<Vec<PathBuf>> {
    if !log_directory.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(log_directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_audit_log(path))
        .collect())
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn is_active_path(path: &Path, active_path: Option<&Path>) -> bool {
    active_path.is_some_and(|active| active == path)
}

fn is_audit_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name
        .strip_prefix("audit-")
        .and_then(|value| value.strip_suffix(".jsonl"))
    else {
        return false;
    };
    let parts = stem.split('-').collect::<Vec<_>>();
    let [year, month, day, ..] = parts.as_slice() else {
        return false;
    };
    matches!(parts.len(), 3 | 4)
        && year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
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
    use std::{
        collections::BTreeMap,
        sync::Mutex,
        time::{Duration, UNIX_EPOCH},
    };

    use super::{
        AuditMetadata, WindowsAuditLog, audit_file_name, audit_log_files, clamp_retention,
        is_audit_log, serialize_entry,
    };
    use uuid::Uuid;
    use wechat_send_guard_core::AuditEntry;
    use wechat_send_guard_platform_api::AuditLog;

    #[test]
    fn audit_serialization_keeps_only_allow_listed_content_free_details() {
        let mut entry = AuditEntry::new(
            UNIX_EPOCH + Duration::from_millis(42),
            None,
            "send",
            "injected",
        )
        .with_trace_id(Uuid::nil());
        entry.details = BTreeMap::from([
            ("source".to_owned(), "enter".to_owned()),
            ("contextSource".to_owned(), "provided-context".to_owned()),
            ("requestedMinutes".to_owned(), "5".to_owned()),
            ("ruleMode".to_owned(), "confirm-unless-excluded".to_owned()),
            ("protectionEnabled".to_owned(), "true".to_owned()),
            ("knownChat".to_owned(), "true".to_owned()),
            ("chatTargetKind".to_owned(), "group".to_owned()),
            ("decisionKind".to_owned(), "confirm-unlisted".to_owned()),
            (
                "contextMaximumAgeMilliseconds".to_owned(),
                "5000".to_owned(),
            ),
            ("uiaTreeQueryStatus".to_owned(), "query-failed".to_owned()),
            (
                "uiaTreeErrorCode".to_owned(),
                "uia-tree-query-failed:0x80040201".to_owned(),
            ),
            ("chatTitle".to_owned(), "绝不写入".to_owned()),
        ]);
        let metadata = Mutex::new(AuditMetadata {
            application_version: "1.3.1".to_owned(),
            weixin_version: "4.1.13.12".to_owned(),
            session_id: Uuid::nil(),
            process_id: 7,
        });
        let serialized = serialize_entry(&entry, &metadata);

        assert!(serialized.starts_with("{\"localTime\":"));
        assert!(serialized.contains("\"applicationVersion\":\"1.3.1\""));
        assert!(serialized.contains("\"weixinVersion\":\"4.1.13.12\""));
        assert!(serialized.contains("timestampUnixMilliseconds"));
        assert!(serialized.contains("traceId"));
        assert!(serialized.contains("source"));
        assert!(serialized.contains("contextSource"));
        assert!(serialized.contains("requestedMinutes"));
        assert!(serialized.contains("ruleMode"));
        assert!(serialized.contains("protectionEnabled"));
        assert!(serialized.contains("knownChat"));
        assert!(serialized.contains("chatTargetKind"));
        assert!(serialized.contains("decisionKind"));
        assert!(serialized.contains("contextMaximumAgeMilliseconds"));
        assert!(serialized.contains("uiaTreeQueryStatus"));
        assert!(serialized.contains("uia-tree-query-failed:0x80040201"));
        assert!(!serialized.contains("chatTitle"));
        assert!(!serialized.contains("draft"));
    }

    #[test]
    fn retention_and_file_filters_are_bounded_and_narrow() {
        assert_eq!(clamp_retention(0), 1);
        assert_eq!(clamp_retention(99), 30);
        assert!(is_audit_log(std::path::Path::new("audit-2026-08-27.jsonl")));
        assert!(is_audit_log(std::path::Path::new(
            "audit-2026-08-27-01.jsonl"
        )));
        assert!(!is_audit_log(std::path::Path::new("settings.json")));
        assert_eq!(audit_file_name((2026, 8, 27), 0), "audit-2026-08-27.jsonl");
    }

    #[test]
    fn queued_audit_entries_flush_and_clear_in_order() {
        let directory =
            std::env::temp_dir().join(format!("WeChatSendGuard-audit-test-{}", Uuid::new_v4()));
        let log = WindowsAuditLog::new(&directory, 7).expect("audit log should start");
        log.write(AuditEntry::new(
            UNIX_EPOCH + Duration::from_secs(1),
            None,
            "send",
            "injected",
        ));
        log.flush().expect("flush should wait for queued writes");
        assert_eq!(
            audit_log_files(&directory)
                .expect("audit directory should be readable")
                .len(),
            1
        );
        log.clear().expect("clear should run after queued writes");
        assert!(
            audit_log_files(&directory)
                .expect("audit directory should be readable")
                .is_empty()
        );
        log.shutdown();
        let _ = std::fs::remove_dir_all(directory);
    }
}
