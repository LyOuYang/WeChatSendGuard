use serde::Serialize;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

const MAX_EXPORT_LOG_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEnvironment<'a> {
    pub application_version: &'a str,
    pub operating_system: String,
    pub architecture: &'a str,
    pub weixin_version: Option<String>,
    pub trusted_weixin_executable: String,
    pub auto_check_updates: bool,
    pub ignored_update_version: Option<&'a str>,
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
            "WeChatSendGuard 诊断包\r\n\r\n包含：本地审计与诊断日志，以及不含个人路径的环境摘要。\r\n不包含：消息正文、草稿、联系人/群聊名称、设置文件、微信数据或截图。\r\n"
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

    use super::{DiagnosticEnvironment, redact_windows_path, write_diagnostic_archive};

    #[test]
    fn diagnostic_path_redaction_hides_the_current_user_directory() {
        assert_eq!(
            redact_windows_path(r"C:\Users\dylan\AppData\Local\WeChat\Weixin.exe"),
            r"C:\…\Weixin.exe"
        );
        assert_eq!(redact_windows_path("Weixin.exe"), "已配置");
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
            operating_system: "Windows 11".into(),
            architecture: "x86_64",
            weixin_version: Some("4.0.0.1".into()),
            trusted_weixin_executable: r"C:\…\Weixin.exe".into(),
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
