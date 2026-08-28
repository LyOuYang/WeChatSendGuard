use std::{mem::size_of, path::PathBuf};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult};
use windows::{
    Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST,
        OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    },
    core::{PCWSTR, PWSTR},
};

const FILE_BUFFER_LENGTH: usize = 32_768;
const JSON_EXTENSION: &str = "json";
const ZIP_EXTENSION: &str = "zip";

/// Opens the built-in Windows file dialog for a protected-chat export. The dialog has no
/// connection to Weixin and is only invoked from an explicit UI action.
pub fn select_protected_chat_import() -> PlatformResult<Option<PathBuf>> {
    select_file(FileDialogMode::Open, None)
}

/// Opens the built-in Windows save dialog for a protected-chat export. The caller owns writing
/// the selected path after this function returns.
pub fn select_protected_chat_export(default_file_name: &str) -> PlatformResult<Option<PathBuf>> {
    select_file(FileDialogMode::SaveJson, Some(default_file_name))
}

/// Opens the built-in Windows save dialog for a user-requested diagnostic archive.
pub fn select_diagnostic_export(default_file_name: &str) -> PlatformResult<Option<PathBuf>> {
    select_file(FileDialogMode::SaveZip, Some(default_file_name))
}

#[derive(Clone, Copy)]
enum FileDialogMode {
    Open,
    SaveJson,
    SaveZip,
}

fn select_file(
    mode: FileDialogMode,
    default_file_name: Option<&str>,
) -> PlatformResult<Option<PathBuf>> {
    let mut file_buffer = vec![0u16; FILE_BUFFER_LENGTH];
    if let Some(default_file_name) = default_file_name {
        let default_name = default_file_name.encode_utf16().collect::<Vec<_>>();
        if default_name.len() + 1 < file_buffer.len() {
            file_buffer[..default_name.len()].copy_from_slice(&default_name);
        }
    }

    let flags = match mode {
        FileDialogMode::Open => OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        FileDialogMode::SaveJson | FileDialogMode::SaveZip => {
            OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST
        }
    };
    let (filter, extension) = match mode {
        FileDialogMode::Open | FileDialogMode::SaveJson => (json_filter(), wide(JSON_EXTENSION)),
        FileDialogMode::SaveZip => (zip_filter(), wide(ZIP_EXTENSION)),
    };
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file_buffer.as_mut_ptr()),
        nMaxFile: file_buffer.len() as u32,
        lpstrDefExt: PCWSTR(extension.as_ptr()),
        Flags: flags,
        ..Default::default()
    };

    // SAFETY: OPENFILENAMEW points only to the local UTF-16 buffers retained for the complete
    // synchronous dialog call. No callback or borrowed window handle is supplied.
    let selected = unsafe {
        match mode {
            FileDialogMode::Open => GetOpenFileNameW(&mut dialog),
            FileDialogMode::SaveJson | FileDialogMode::SaveZip => GetSaveFileNameW(&mut dialog),
        }
    };
    if selected.as_bool() {
        let length = file_buffer
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(file_buffer.len());
        return Ok(Some(PathBuf::from(String::from_utf16_lossy(
            &file_buffer[..length],
        ))));
    }

    // A zero extended error represents user cancellation, not an application error.
    // SAFETY: CommDlgExtendedError reads per-thread state after the synchronous dialog call.
    let error = unsafe { CommDlgExtendedError() };
    if error.0 == 0 {
        Ok(None)
    } else {
        Err(PlatformError::new(
            "file-dialog-failed",
            format!("Windows common dialog error {}", error.0),
        ))
    }
}

fn json_filter() -> Vec<u16> {
    wide("会话配置 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0")
}

fn zip_filter() -> Vec<u16> {
    wide("诊断压缩包 (*.zip)\0*.zip\0所有文件 (*.*)\0*.*\0")
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::json_filter;

    #[test]
    fn json_filter_is_double_nul_terminated_without_opening_a_dialog() {
        let filter = json_filter();
        assert_eq!(filter[filter.len() - 1], 0);
        assert_eq!(filter[filter.len() - 2], 0);
        assert!(String::from_utf16_lossy(&filter).contains("*.json"));
    }
}
