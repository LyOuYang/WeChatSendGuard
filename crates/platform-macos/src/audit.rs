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

use crate::ffi;

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

pub struct MacAuditLog {
    sender: Mutex<Option<SyncSender<AuditCommand>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    retention_days: Arc<AtomicU32>,
    metadata: Arc<Mutex<AuditMetadata>>,
    log_directory: PathBuf,
}

impl MacAuditLog {
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

impl AuditLog for MacAuditLog {
    fn write(&self, entry: AuditEntry) {
        if let Some(sender) = lock_unpoisoned(&self.sender).as_ref() {
            let _ = sender.try_send(AuditCommand::Entry(entry));
        }
    }
}

impl Drop for MacAuditLog {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn default_audit_log_directory(application_support: &Path) -> PathBuf {
    application_support.join("WeChatSendGuard").join("logs")
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
    let mut year = 1970;
    let mut month = 1;
    let mut day = 1;
    // SAFETY: all three out pointers are valid stack values and the bridge retains none of them.
    let _ = unsafe { ffi::WSGMacCopyLocalDate(&mut year, &mut month, &mut day) };
    (year, month, day)
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
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("audit-") && name.ends_with(".jsonl"))
        })
        .collect())
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn is_active_path(path: &Path, active_path: Option<&Path>) -> bool {
    active_path.is_some_and(|active| active == path)
}

fn clamp_retention(days: u32) -> u32 {
    days.clamp(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{audit_file_name, is_safe_detail};

    #[test]
    fn audit_names_match_the_cross_platform_rotation_contract() {
        assert_eq!(audit_file_name((2026, 8, 29), 0), "audit-2026-08-29.jsonl");
        assert_eq!(
            audit_file_name((2026, 8, 29), 2),
            "audit-2026-08-29-02.jsonl"
        );
    }

    #[test]
    fn audit_allowlist_rejects_content_bearing_fields() {
        assert!(is_safe_detail(
            "trustedWeixinIdentity",
            "com.tencent.xinWeChat"
        ));
        assert!(is_safe_detail("contextSource", "provided-context"));
        assert!(is_safe_detail("requestedMinutes", "5"));
        assert!(is_safe_detail("ruleMode", "confirm-unless-excluded"));
        assert!(is_safe_detail("protectionEnabled", "true"));
        assert!(is_safe_detail("knownChat", "true"));
        assert!(is_safe_detail("chatTargetKind", "group"));
        assert!(is_safe_detail("decisionKind", "confirm-unlisted"));
        assert!(is_safe_detail("contextMaximumAgeMilliseconds", "5000"));
        assert!(!is_safe_detail("chatTitle", "private name"));
        assert!(!is_safe_detail("source", "line one\nline two"));
    }
}
