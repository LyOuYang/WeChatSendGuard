use std::{ffi::CString, path::PathBuf};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult};

use crate::ffi;

pub fn select_protected_chat_import() -> PlatformResult<Option<PathBuf>> {
    let mut output = [0i8; ffi::PATH_CAPACITY];
    // SAFETY: the bridge receives a bounded output buffer and runs a synchronous AppKit panel.
    if unsafe { ffi::WSGMacSelectOpenJSON(output.as_mut_ptr(), output.len()) } {
        Ok(Some(PathBuf::from(c_buffer_to_string(&output)?)))
    } else {
        Ok(None)
    }
}

pub fn select_protected_chat_export(default_file_name: &str) -> PlatformResult<Option<PathBuf>> {
    select_save_path(default_file_name, "json")
}

pub fn select_diagnostic_export(default_file_name: &str) -> PlatformResult<Option<PathBuf>> {
    select_save_path(default_file_name, "zip")
}

fn select_save_path(default_file_name: &str, extension: &str) -> PlatformResult<Option<PathBuf>> {
    let default_file_name = CString::new(default_file_name).map_err(|_| {
        PlatformError::new(
            "file-dialog-name-invalid",
            "The default file name contains NUL.",
        )
    })?;
    let extension = CString::new(extension).expect("static extension contains no NUL");
    let mut output = [0i8; ffi::PATH_CAPACITY];
    // SAFETY: both strings are NUL-terminated and all buffers remain live for the synchronous
    // AppKit panel call.
    if unsafe {
        ffi::WSGMacSelectSavePath(
            default_file_name.as_ptr(),
            extension.as_ptr(),
            output.as_mut_ptr(),
            output.len(),
        )
    } {
        Ok(Some(PathBuf::from(c_buffer_to_string(&output)?)))
    } else {
        Ok(None)
    }
}

fn c_buffer_to_string(buffer: &[std::ffi::c_char]) -> PlatformResult<String> {
    // SAFETY: the native bridge always zeroes or NUL-terminates the fixed-size output buffer.
    let value = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|error| PlatformError::new("file-dialog-path-encoding", error.to_string()))
}
