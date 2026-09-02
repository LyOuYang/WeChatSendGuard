use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;
#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::{collections::VecDeque, io::Read, path::PathBuf, time::UNIX_EPOCH};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    time::SystemTime,
};
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

const MAX_EXPORT_LOG_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEnvironment<'a> {
    pub application_version: &'a str,
    pub weixin_version: Option<String>,
    pub exported_at_local: String,
    pub audit_schema_version: u32,
    pub audit_session_id: String,
    pub operating_system: String,
    pub architecture: &'a str,
    pub trusted_weixin_identity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weixin_installation: Option<WeixinInstallationDiagnostics>,
    pub auto_check_updates: bool,
    pub ignored_update_version: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryIdentityDiagnostics {
    pub file_name: String,
    pub present: bool,
    pub file_version: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified_unix_milliseconds: Option<u128>,
    pub sha256: Option<String>,
    pub fingerprint_status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinInstallationDiagnostics {
    pub configured: bool,
    pub executable: Option<BinaryIdentityDiagnostics>,
    pub dll_scan_status: String,
    pub dll_entries_scanned: usize,
    pub dll_candidate_count: usize,
    pub dll_beside_executable: bool,
    pub selected_dll_relative_depth: Option<usize>,
    pub selected_dll: Option<BinaryIdentityDiagnostics>,
    pub dll_candidates: Vec<DllCandidateDiagnostics>,
    pub loaded_module_scan_status: String,
    pub loaded_weixin_dll_beside_executable: Option<bool>,
    pub loaded_weixin_dll_relative_depth: Option<usize>,
    pub loaded_weixin_dll_matches_selected_candidate: Option<bool>,
    pub loaded_weixin_dll: Option<BinaryIdentityDiagnostics>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DllCandidateDiagnostics {
    pub relative_depth: usize,
    pub beside_executable: bool,
    pub identity: BinaryIdentityDiagnostics,
}

pub fn local_time_now() -> String {
    let local: DateTime<Local> = SystemTime::now().into();
    local.to_rfc3339_opts(SecondsFormat::Millis, false)
}

#[cfg(windows)]
pub fn collect_weixin_installation_diagnostics(
    configured_executable: Option<&str>,
    process_id: u32,
) -> WeixinInstallationDiagnostics {
    const MAX_SCAN_DEPTH: usize = 3;
    const MAX_SCAN_ENTRIES: usize = 512;

    let Some(executable_path) = configured_executable.map(PathBuf::from) else {
        return WeixinInstallationDiagnostics {
            configured: false,
            executable: None,
            dll_scan_status: "not-configured".to_owned(),
            dll_entries_scanned: 0,
            dll_candidate_count: 0,
            dll_beside_executable: false,
            selected_dll_relative_depth: None,
            selected_dll: None,
            dll_candidates: Vec::new(),
            loaded_module_scan_status: "process-unavailable".to_owned(),
            loaded_weixin_dll_beside_executable: None,
            loaded_weixin_dll_relative_depth: None,
            loaded_weixin_dll_matches_selected_candidate: None,
            loaded_weixin_dll: None,
        };
    };
    let executable = Some(binary_identity(&executable_path));
    let Some(root) = executable_path.parent() else {
        return WeixinInstallationDiagnostics {
            configured: true,
            executable,
            dll_scan_status: "install-root-unavailable".to_owned(),
            dll_entries_scanned: 0,
            dll_candidate_count: 0,
            dll_beside_executable: false,
            selected_dll_relative_depth: None,
            selected_dll: None,
            dll_candidates: Vec::new(),
            loaded_module_scan_status: "process-unavailable".to_owned(),
            loaded_weixin_dll_beside_executable: None,
            loaded_weixin_dll_relative_depth: None,
            loaded_weixin_dll_matches_selected_candidate: None,
            loaded_weixin_dll: None,
        };
    };

    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut candidates = Vec::new();
    let mut entries_scanned = 0usize;
    let mut scan_failed = false;
    while let Some((directory, depth)) = queue.pop_front() {
        if entries_scanned >= MAX_SCAN_ENTRIES {
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                scan_failed = true;
                continue;
            }
        };
        for entry in entries.filter_map(Result::ok) {
            entries_scanned = entries_scanned.saturating_add(1);
            if entries_scanned > MAX_SCAN_ENTRIES {
                break;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                scan_failed = true;
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Weixin.dll"))
            {
                candidates.push((path, depth));
            } else if file_type.is_dir() && depth < MAX_SCAN_DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    candidates.sort_by_key(|(path, depth)| (*depth, path.clone()));
    let dll_beside_executable = candidates.iter().any(|(_, depth)| *depth == 0);
    let selected_dll_relative_depth = candidates.first().map(|(_, depth)| *depth);
    let selected_dll = candidates.first().map(|(path, _)| binary_identity(path));
    let dll_candidates = candidates
        .iter()
        .take(16)
        .map(|(path, depth)| DllCandidateDiagnostics {
            relative_depth: *depth,
            beside_executable: *depth == 0,
            identity: binary_identity(path),
        })
        .collect();
    let (loaded_module_scan_status, loaded_path) = loaded_weixin_module_path(process_id);
    let loaded_weixin_dll_beside_executable = loaded_path
        .as_ref()
        .map(|path| paths_equal(path.parent(), Some(root)));
    let loaded_weixin_dll_relative_depth = loaded_path
        .as_ref()
        .and_then(|path| relative_parent_depth(root, path));
    let loaded_weixin_dll_matches_selected_candidate = loaded_path.as_ref().map(|loaded_path| {
        candidates
            .first()
            .is_some_and(|(selected_path, _)| paths_equal(Some(loaded_path), Some(selected_path)))
    });
    let loaded_weixin_dll = loaded_path.as_ref().map(|path| binary_identity(path));
    let dll_scan_status = if entries_scanned >= MAX_SCAN_ENTRIES {
        "entry-limit-reached"
    } else if scan_failed {
        "partial-read-failure"
    } else {
        "complete"
    };
    WeixinInstallationDiagnostics {
        configured: true,
        executable,
        dll_scan_status: dll_scan_status.to_owned(),
        dll_entries_scanned: entries_scanned.min(MAX_SCAN_ENTRIES),
        dll_candidate_count: candidates.len(),
        dll_beside_executable,
        selected_dll_relative_depth,
        selected_dll,
        dll_candidates,
        loaded_module_scan_status,
        loaded_weixin_dll_beside_executable,
        loaded_weixin_dll_relative_depth,
        loaded_weixin_dll_matches_selected_candidate,
        loaded_weixin_dll,
    }
}

#[cfg(windows)]
fn loaded_weixin_module_path(process_id: u32) -> (String, Option<PathBuf>) {
    use std::mem::size_of;
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_NO_MORE_FILES},
            System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
            },
        },
        core::HRESULT,
    };

    if process_id == 0 {
        return ("process-unavailable".to_owned(), None);
    }
    // SAFETY: the snapshot API receives a process identifier observed from the foreground window.
    let snapshot = match unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id)
    } {
        Ok(snapshot) => snapshot,
        Err(_) => return ("snapshot-failed".to_owned(), None),
    };
    let mut entry = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..MODULEENTRY32W::default()
    };
    // SAFETY: entry has the required size and remains writable for the synchronous enumeration.
    if unsafe { Module32FirstW(snapshot, &mut entry) }.is_err() {
        // SAFETY: snapshot is a valid owned snapshot handle and is closed exactly once here.
        let _ = unsafe { CloseHandle(snapshot) };
        return ("enumeration-failed".to_owned(), None);
    }

    let mut found = None;
    let mut terminal_status = "not-found";
    loop {
        if wide_string(&entry.szModule).eq_ignore_ascii_case("Weixin.dll") {
            found = Some(PathBuf::from(wide_string(&entry.szExePath)));
            terminal_status = "found";
            break;
        }
        // SAFETY: snapshot and entry remain valid throughout this loop.
        match unsafe { Module32NextW(snapshot, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => break,
            Err(_) => {
                terminal_status = "enumeration-failed";
                break;
            }
        }
    }
    // SAFETY: snapshot is a valid owned snapshot handle and is closed exactly once here.
    let _ = unsafe { CloseHandle(snapshot) };
    (terminal_status.to_owned(), found)
}

#[cfg(windows)]
fn wide_string(value: &[u16]) -> String {
    let length = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..length])
}

#[cfg(windows)]
fn paths_equal(first: Option<&Path>, second: Option<&Path>) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => first
            .to_string_lossy()
            .eq_ignore_ascii_case(&second.to_string_lossy()),
        _ => false,
    }
}

#[cfg(windows)]
fn relative_parent_depth(root: &Path, file: &Path) -> Option<usize> {
    let relative = file.strip_prefix(root).ok()?;
    relative.parent().map(|parent| parent.components().count())
}

#[cfg(windows)]
fn binary_identity(path: &Path) -> BinaryIdentityDiagnostics {
    let metadata = fs::metadata(path).ok();
    let sha256 = sha256_file(path).ok();
    BinaryIdentityDiagnostics {
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_owned(),
        present: metadata.is_some(),
        file_version: windows_file_version(path),
        size_bytes: metadata.as_ref().map(fs::Metadata::len),
        modified_unix_milliseconds: metadata
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis()),
        fingerprint_status: if sha256.is_some() {
            "ok".to_owned()
        } else if path.exists() {
            "read-failed".to_owned()
        } else {
            "not-found".to_owned()
        },
        sha256,
    }
}

#[cfg(windows)]
fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(windows)]
pub fn windows_file_version(path: &Path) -> Option<String> {
    use std::{ffi::c_void, mem::size_of, os::windows::ffi::OsStrExt};
    use windows::{
        Win32::Storage::FileSystem::{
            GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
        },
        core::PCWSTR,
    };

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: path is NUL-terminated and retained for the entire synchronous version query.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: data is a valid mutable buffer of exactly the queried version-info size.
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(path.as_ptr()),
            None,
            size,
            data.as_mut_ptr().cast::<c_void>(),
        )
        .ok()?;
    }
    let mut fixed: *mut c_void = std::ptr::null_mut();
    let mut length = 0u32;
    let root = [0u16];
    // SAFETY: data remains alive and root is a NUL-terminated sub-block selector for this call.
    if !unsafe {
        VerQueryValueW(
            data.as_ptr().cast::<c_void>(),
            PCWSTR(root.as_ptr()),
            &mut fixed,
            &mut length,
        )
    }
    .as_bool()
        || fixed.is_null()
        || length < size_of::<VS_FIXEDFILEINFO>() as u32
    {
        return None;
    }
    // SAFETY: VerQueryValueW returned a buffer with at least a VS_FIXEDFILEINFO structure.
    let fixed = unsafe { &*(fixed.cast::<VS_FIXEDFILEINFO>()) };
    Some(format!(
        "{}.{}.{}.{}",
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xffff,
        fixed.dwFileVersionLS >> 16,
        fixed.dwFileVersionLS & 0xffff
    ))
}

pub fn write_diagnostic_archive(
    destination: &Path,
    log_directory: &Path,
    environment: &DiagnosticEnvironment<'_>,
) -> Result<(), String> {
    let output =
        File::create(destination).map_err(|error| format!("无法创建诊断压缩包：{error}"))?;
    let mut archive = ZipWriter::new(output);
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

    archive
        .start_file("environment.json", options)
        .map_err(zip_error)?;
    let environment = serde_json::to_vec_pretty(environment)
        .map_err(|error| format!("无法写入诊断环境信息：{error}"))?;
    archive.write_all(&environment).map_err(io_error)?;

    archive
        .start_file("README.txt", options)
        .map_err(zip_error)?;
    archive
        .write_all(
            "WeChatSendGuard 诊断包\r\n\r\n包含：本地时间、软件/微信版本、进程会话、脱敏安装布局与文件指纹，以及不含用户内容的 UI 自动化诊断。\r\n不包含：消息正文、草稿、联系人/群聊名称、完整个人路径、设置文件、微信数据或截图。\r\n"
                .as_bytes(),
        )
        .map_err(io_error)?;

    let mut exported = 0u64;
    for path in diagnostic_log_files(log_directory)? {
        let metadata = fs::metadata(&path).map_err(io_error)?;
        if metadata.len() > MAX_EXPORT_LOG_BYTES.saturating_sub(exported) {
            break;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        archive
            .start_file(format!("logs/{name}"), options)
            .map_err(zip_error)?;
        let mut input = File::open(&path).map_err(io_error)?;
        io::copy(&mut input, &mut archive).map_err(io_error)?;
        exported = exported.saturating_add(metadata.len());
    }

    archive.finish().map_err(zip_error)?;
    Ok(())
}

#[cfg(windows)]
pub fn redact_windows_path(path: &str) -> String {
    let normalized = path.replace('/', "\\");
    let parts = normalized
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return "已配置".to_owned();
    }
    let prefix = parts.first().copied().unwrap_or_default();
    let tail = parts.last().copied().unwrap_or_default();
    if prefix.ends_with(':') {
        format!("{prefix}\\…\\{tail}")
    } else {
        "已配置".to_owned()
    }
}

fn diagnostic_log_files(log_directory: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    if !log_directory.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(log_directory)
        .map_err(io_error)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("audit-") && name.ends_with(".jsonl"))
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn io_error(error: io::Error) -> String {
    format!("诊断导出失败：{error}")
}

fn zip_error(error: zip::result::ZipError) -> String {
    format!("诊断压缩失败：{error}")
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use super::{DiagnosticEnvironment, write_diagnostic_archive};
    #[cfg(windows)]
    use super::{collect_weixin_installation_diagnostics, redact_windows_path};

    #[cfg(windows)]
    #[test]
    fn diagnostic_path_redaction_hides_the_current_user_directory() {
        assert_eq!(
            redact_windows_path(r"C:\Users\dylan\AppData\Local\WeChat\Weixin.exe"),
            r"C:\…\Weixin.exe"
        );
        assert_eq!(redact_windows_path("Weixin.exe"), "已配置");
    }

    #[cfg(windows)]
    #[test]
    fn installation_diagnostics_fingerprint_nested_dll_without_exporting_its_path() {
        let directory = std::env::temp_dir().join(format!(
            "WeChatSendGuard-install-diagnostics-test-{}",
            uuid::Uuid::new_v4()
        ));
        let version_directory = directory.join("4.1.13.12");
        fs::create_dir_all(&version_directory).expect("version directory should be created");
        let executable = directory.join("Weixin.exe");
        fs::write(&executable, b"test-executable").expect("test executable should be written");
        fs::write(version_directory.join("Weixin.dll"), b"test-dll")
            .expect("test dll should be written");

        let diagnostics = collect_weixin_installation_diagnostics(executable.to_str(), 0);
        assert_eq!(diagnostics.dll_candidate_count, 1);
        assert!(!diagnostics.dll_beside_executable);
        assert_eq!(diagnostics.selected_dll_relative_depth, Some(1));
        assert_eq!(diagnostics.dll_candidates.len(), 1);
        assert_eq!(diagnostics.loaded_module_scan_status, "process-unavailable");
        assert!(
            diagnostics
                .selected_dll
                .as_ref()
                .and_then(|identity| identity.sha256.as_ref())
                .is_some()
        );
        let serialized = serde_json::to_string(&diagnostics).expect("diagnostics should serialize");
        assert!(!serialized.contains(directory.to_string_lossy().as_ref()));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn diagnostic_archive_includes_only_environment_and_audit_files() {
        let directory = std::env::temp_dir().join(format!(
            "WeChatSendGuard-diagnostics-test-{}",
            uuid::Uuid::new_v4()
        ));
        let logs = directory.join("logs");
        fs::create_dir_all(&logs).expect("test log directory should be created");
        fs::write(
            logs.join("audit-2026-08-28.jsonl"),
            "{\"eventType\":\"send\"}\n",
        )
        .expect("test audit file should be written");
        let archive_path = directory.join("diagnostics.zip");
        let environment = DiagnosticEnvironment {
            application_version: "1.2.3",
            weixin_version: Some("4.0.0.1".into()),
            exported_at_local: "2026-08-31T10:50:05.364+08:00".into(),
            audit_schema_version: 2,
            audit_session_id: uuid::Uuid::nil().to_string(),
            operating_system: "Windows 11".into(),
            architecture: "x86_64",
            trusted_weixin_identity: r"C:\…\Weixin.exe".into(),
            weixin_installation: None,
            auto_check_updates: true,
            ignored_update_version: None,
        };

        write_diagnostic_archive(&archive_path, &logs, &environment)
            .expect("diagnostic archive should be written");
        let archive_file = fs::File::open(&archive_path).expect("archive should exist");
        let mut archive = zip::ZipArchive::new(archive_file).expect("archive should be readable");
        let mut environment_json = String::new();
        archive
            .by_name("environment.json")
            .expect("environment should be present")
            .read_to_string(&mut environment_json)
            .expect("environment should be readable");
        assert!(environment_json.contains("Windows 11"));
        assert!(archive.by_name("logs/audit-2026-08-28.jsonl").is_ok());
        assert!(archive.by_name("settings.json").is_err());

        let _ = fs::remove_dir_all(directory);
    }
}
