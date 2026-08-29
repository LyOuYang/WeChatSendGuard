use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/LyOuYang/WeChatSendGuard/releases/latest";
const USER_AGENT: &str = "WeChatSendGuard-updater";
const NETWORK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_BUFFER_SIZE: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckResult {
    UpToDate,
    UpdateAvailable(Box<Release>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub published_at: Option<String>,
    pub installer: ReleaseAsset,
    pub checksum: ReleaseAsset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn check_for_update(current_version: &str) -> Result<UpdateCheckResult, String> {
    let current_version = parse_version(current_version)?;
    let client = client()?;
    let response = client
        .get(GITHUB_LATEST_RELEASE_URL)
        .send()
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(format!(
            "检查更新失败：GitHub 返回 HTTP {}。",
            response.status()
        ));
    }
    let release = response
        .json::<GitHubRelease>()
        .map_err(|error| format!("检查更新失败：无法读取版本信息（{error}）。"))?;
    let release = parse_release(release)?;
    if release.version > current_version {
        Ok(UpdateCheckResult::UpdateAvailable(Box::new(release)))
    } else {
        Ok(UpdateCheckResult::UpToDate)
    }
}

pub fn download_and_verify(
    release: &Release,
    temporary_directory: &Path,
    mut report_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    fs::create_dir_all(temporary_directory)
        .map_err(|error| format!("无法创建更新临时目录：{error}"))?;
    let destination = temporary_directory.join(&release.installer.name);
    let partial = destination.with_extension("part");
    let client = client()?;

    let checksum_response = client
        .get(&release.checksum.download_url)
        .send()
        .map_err(network_error)?;
    let checksum_response = require_success(checksum_response, "下载校验文件")?;
    let checksum_text = checksum_response
        .text()
        .map_err(|error| format!("无法读取校验文件：{error}"))?;
    let expected_checksum = checksum_for_asset(&checksum_text, &release.installer.name)?;

    let installer_response = client
        .get(&release.installer.download_url)
        .send()
        .map_err(network_error)?;
    let mut installer_response = require_success(installer_response, "下载安装包")?;
    let content_length = installer_response.content_length();
    let mut output =
        File::create(&partial).map_err(|error| format!("无法创建更新文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; DOWNLOAD_BUFFER_SIZE];
    let mut downloaded = 0u64;
    loop {
        let count = installer_response
            .read(&mut buffer)
            .map_err(|error| format!("下载安装包时出错：{error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("写入更新文件时出错：{error}"))?;
        hasher.update(&buffer[..count]);
        downloaded = downloaded.saturating_add(count as u64);
        report_progress(downloaded, content_length);
    }
    output
        .flush()
        .map_err(|error| format!("完成更新文件写入时出错：{error}"))?;

    let actual_checksum = format!("{:x}", hasher.finalize());
    if !actual_checksum.eq_ignore_ascii_case(&expected_checksum) {
        let _ = fs::remove_file(&partial);
        return Err("安装包校验失败，已拒绝安装。请稍后重试。".to_owned());
    }
    let _ = fs::remove_file(&destination);
    fs::rename(&partial, &destination).map_err(|error| format!("无法完成更新文件：{error}"))?;
    Ok(destination)
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(NETWORK_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| format!("无法初始化更新连接：{error}"))
}

fn require_success(response: Response, action: &str) -> Result<Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(format!(
            "{action}失败：GitHub 返回 HTTP {}。",
            response.status()
        ))
    }
}

fn parse_release(release: GitHubRelease) -> Result<Release, String> {
    if release.draft || release.prerelease {
        return Err("检查更新失败：最新发布不是正式版本。".to_owned());
    }
    let version = parse_version(&release.tag_name)?;
    let installer_name = installer_asset_name(&version);
    let checksum_name = format!("{installer_name}.sha256");
    let installer = find_asset(&release.assets, &installer_name)?;
    let checksum = find_asset(&release.assets, &checksum_name)?;
    Ok(Release {
        version: version.clone(),
        tag_name: release.tag_name,
        name: if release.name.trim().is_empty() {
            format!("WeChatSendGuard {version}")
        } else {
            release.name
        },
        body: release.body,
        published_at: release.published_at,
        installer,
        checksum,
    })
}

#[cfg(windows)]
fn installer_asset_name(version: &Version) -> String {
    format!("WeChatSendGuard-Setup-{version}.exe")
}

#[cfg(target_os = "macos")]
fn installer_asset_name(version: &Version) -> String {
    format!("WeChatSendGuard-{version}-universal.dmg")
}

fn find_asset(assets: &[GitHubAsset], name: &str) -> Result<ReleaseAsset, String> {
    let asset = assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("检查更新失败：正式发布缺少 {name}。"))?;
    let url = reqwest::Url::parse(&asset.browser_download_url)
        .map_err(|_| format!("检查更新失败：{name} 的下载地址无效。"))?;
    if url.scheme() != "https" {
        return Err(format!("检查更新失败：{name} 的下载地址不是 HTTPS。"));
    }
    Ok(ReleaseAsset {
        name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
    })
}

fn parse_version(value: &str) -> Result<Version, String> {
    Version::parse(value.trim_start_matches('v'))
        .map_err(|_| format!("检查更新失败：版本号“{value}”无效。"))
}

fn checksum_for_asset(contents: &str, asset_name: &str) -> Result<String, String> {
    let reader = BufReader::new(contents.as_bytes());
    for line in reader.lines().map_while(Result::ok) {
        let mut parts = line.split_whitespace();
        let Some(checksum) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let checksum = checksum.trim_start_matches('\u{feff}');
        if name.trim_start_matches('*') == asset_name && is_sha256(checksum) {
            return Ok(checksum.to_owned());
        }
    }
    Err(format!("校验文件不包含 {asset_name} 的 SHA-256。"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "检查更新失败：网络请求超时。".to_owned()
    } else {
        format!("检查更新失败：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GitHubAsset, GitHubRelease, checksum_for_asset, installer_asset_name, is_sha256,
        parse_release, parse_version,
    };

    #[test]
    fn versions_accept_a_release_tag_prefix() {
        assert!(parse_version("v1.2.3").is_ok());
        assert!(parse_version("1.2.3").is_ok());
        assert!(parse_version("latest").is_err());
    }

    #[test]
    fn installer_asset_matches_the_active_platform() {
        let version = parse_version("1.2.3").unwrap();
        #[cfg(windows)]
        assert_eq!(
            installer_asset_name(&version),
            "WeChatSendGuard-Setup-1.2.3.exe"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            installer_asset_name(&version),
            "WeChatSendGuard-1.2.3-universal.dmg"
        );
    }

    #[test]
    fn checksum_reader_selects_the_expected_installer() {
        let checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  WeChatSendGuard-Setup-1.2.3.exe\n";
        assert_eq!(
            checksum_for_asset(checksum, "WeChatSendGuard-Setup-1.2.3.exe").unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(checksum_for_asset(checksum, "other.exe").is_err());
        let checksum_with_bom = format!("\u{feff}{checksum}");
        assert!(checksum_for_asset(&checksum_with_bom, "WeChatSendGuard-Setup-1.2.3.exe").is_ok());
        assert!(is_sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!is_sha256("abc"));
    }

    #[test]
    fn release_parser_requires_named_https_assets() {
        let version = parse_version("v1.2.3").unwrap();
        let installer_name = installer_asset_name(&version);
        let checksum_name = format!("{installer_name}.sha256");
        let release = GitHubRelease {
            tag_name: "v1.2.3".into(),
            name: "WeChatSendGuard 1.2.3".into(),
            body: "修复发送确认".into(),
            published_at: None,
            draft: false,
            prerelease: false,
            assets: vec![
                GitHubAsset {
                    name: installer_name,
                    browser_download_url: "https://example.invalid/installer".into(),
                },
                GitHubAsset {
                    name: checksum_name,
                    browser_download_url: "https://example.invalid/installer.sha256".into(),
                },
            ],
        };
        let parsed = parse_release(release).expect("named HTTPS assets should parse");
        assert_eq!(parsed.version.to_string(), "1.2.3");

        let insecure = GitHubRelease {
            tag_name: "v1.2.3".into(),
            name: String::new(),
            body: String::new(),
            published_at: None,
            draft: false,
            prerelease: false,
            assets: vec![
                GitHubAsset {
                    name: "WeChatSendGuard-Setup-1.2.3.exe".into(),
                    browser_download_url: "http://example.invalid/installer.exe".into(),
                },
                GitHubAsset {
                    name: "WeChatSendGuard-Setup-1.2.3.exe.sha256".into(),
                    browser_download_url: "https://example.invalid/installer.exe.sha256".into(),
                },
            ],
        };
        assert!(parse_release(insecure).is_err());
    }
}
