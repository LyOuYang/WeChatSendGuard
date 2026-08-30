use std::ffi::CStr;

use crate::ffi;

pub const TRUSTED_WECHAT_BUNDLE_ID: &str = "com.tencent.xinWeChat";
pub const TRUSTED_WECHAT_TEAM_ID: &str = "5A4RE8SF68";

pub fn trusted_wechat_identity() -> String {
    format!("{TRUSTED_WECHAT_BUNDLE_ID} · Team {TRUSTED_WECHAT_TEAM_ID}")
}

pub fn installed_wechat_version() -> Option<String> {
    let mut output = [0i8; ffi::TEXT_CAPACITY];
    // SAFETY: the bridge receives a valid writable buffer and never retains it.
    if !unsafe { ffi::WSGMacCopyInstalledWeChatVersion(output.as_mut_ptr(), output.len()) } {
        return None;
    }
    // SAFETY: the native bridge always NUL-terminates successful output within the buffer.
    Some(
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{TRUSTED_WECHAT_BUNDLE_ID, TRUSTED_WECHAT_TEAM_ID, trusted_wechat_identity};

    #[test]
    fn trusted_identity_requires_both_bundle_and_team() {
        let identity = trusted_wechat_identity();
        assert!(identity.contains(TRUSTED_WECHAT_BUNDLE_ID));
        assert!(identity.contains(TRUSTED_WECHAT_TEAM_ID));
    }
}
