use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use wechat_send_guard_platform_api::{PlatformError, PlatformResult, StartupRegistration};

const LAUNCH_AGENT_LABEL: &str = "io.github.lyouyang.WeChatSendGuard";

#[derive(Debug, Clone)]
pub struct MacStartupRegistration {
    executable_path: PathBuf,
    launch_agent_path: PathBuf,
}

impl MacStartupRegistration {
    pub fn new(executable_path: impl Into<PathBuf>, launch_agents: impl AsRef<Path>) -> Self {
        Self {
            executable_path: executable_path.into(),
            launch_agent_path: launch_agents
                .as_ref()
                .join(format!("{LAUNCH_AGENT_LABEL}.plist")),
        }
    }

    pub fn for_current_executable() -> PlatformResult<Self> {
        let executable_path = std::env::current_exe()
            .map_err(|error| PlatformError::new("startup-executable-path", error.to_string()))?;
        let user_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| PlatformError::new("startup-user-home", "HOME is unavailable"))?;
        Ok(Self::new(
            executable_path,
            user_home.join("Library/LaunchAgents"),
        ))
    }

    pub fn launch_agent_contents(&self) -> String {
        let executable = xml_escape(&self.executable_path.to_string_lossy());
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key><string>{LAUNCH_AGENT_LABEL}</string>\n\
  <key>ProgramArguments</key>\n\
  <array><string>{executable}</string><string>--background</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
  <key>LimitLoadToSessionType</key><string>Aqua</string>\n\
</dict>\n\
</plist>\n"
        )
    }

    fn enable(&self) -> PlatformResult<()> {
        let parent = self.launch_agent_path.parent().ok_or_else(|| {
            PlatformError::new(
                "startup-launch-agent-path",
                "LaunchAgent path has no parent",
            )
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| PlatformError::new("startup-directory-create", error.to_string()))?;
        let temporary = self.launch_agent_path.with_extension("plist.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| PlatformError::new("startup-write", error.to_string()))?;
        file.write_all(self.launch_agent_contents().as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| PlatformError::new("startup-write", error.to_string()))?;
        fs::rename(&temporary, &self.launch_agent_path)
            .map_err(|error| PlatformError::new("startup-replace", error.to_string()))?;
        Ok(())
    }

    fn disable(&self) -> PlatformResult<()> {
        match fs::remove_file(&self.launch_agent_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlatformError::new("startup-remove", error.to_string())),
        }
    }
}

impl StartupRegistration for MacStartupRegistration {
    fn apply(&self, enabled: bool) -> PlatformResult<bool> {
        if enabled {
            self.enable()?;
        } else {
            self.disable()?;
        }
        Ok(enabled)
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::MacStartupRegistration;

    #[test]
    fn launch_agent_uses_current_user_scope_and_background_argument() {
        let registration = MacStartupRegistration::new(
            "/Applications/WeChatSendGuard.app/Contents/MacOS/WeChatSendGuard",
            "/Users/test/Library/LaunchAgents",
        );
        let contents = registration.launch_agent_contents();
        assert!(contents.contains("io.github.lyouyang.WeChatSendGuard"));
        assert!(contents.contains("<string>--background</string>"));
        assert!(contents.contains("<string>Aqua</string>"));
    }

    #[test]
    fn launch_agent_escapes_executable_paths() {
        let registration = MacStartupRegistration::new(
            "/Applications/A&B.app/Contents/MacOS/A&B",
            "/tmp/LaunchAgents",
        );
        assert!(registration.launch_agent_contents().contains("A&amp;B"));
    }
}
