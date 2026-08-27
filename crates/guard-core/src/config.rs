use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::HashSet,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_CONFIRMATION_PHRASE: &str = "确认发送";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfirmationMode {
    Click,
    #[default]
    Hold,
    Phrase,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnknownContextBehavior {
    #[default]
    Confirm,
    Block,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatTargetKind {
    #[default]
    Group,
    Contact,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuleMode {
    #[default]
    ProtectListed,
    ConfirmUnlessExcluded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfirmationSettings {
    pub mode: ConfirmationMode,
    pub hold_milliseconds: u32,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub phrase: String,
    pub timeout_seconds: u32,
}

impl Default for ConfirmationSettings {
    fn default() -> Self {
        Self {
            mode: ConfirmationMode::Hold,
            hold_milliseconds: 800,
            phrase: DEFAULT_CONFIRMATION_PHRASE.to_owned(),
            timeout_seconds: 10,
        }
    }
}

impl ConfirmationSettings {
    pub fn sanitize(mut self) -> Self {
        self.hold_milliseconds = self.hold_milliseconds.clamp(500, 3_000);
        self.timeout_seconds = self.timeout_seconds.clamp(1, 30);
        self.phrase = if self.phrase.trim().is_empty() {
            DEFAULT_CONFIRMATION_PHRASE.to_owned()
        } else {
            self.phrase.trim().to_owned()
        };
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtectedChat {
    pub id: Uuid,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub display_name: String,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub match_title: String,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub aliases: Vec<String>,
    pub enabled: bool,
    pub target_kind: ChatTargetKind,
}

impl Default for ProtectedChat {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            display_name: String::new(),
            match_title: String::new(),
            aliases: Vec::new(),
            enabled: true,
            target_kind: ChatTargetKind::Group,
        }
    }
}

impl ProtectedChat {
    pub fn display_name_with_kind(&self) -> String {
        let prefix = match self.target_kind {
            ChatTargetKind::Contact => "[联系人]",
            ChatTargetKind::Group => "[群聊]",
        };
        format!("{prefix} {}", self.display_name)
    }

    pub fn sanitize(mut self) -> Self {
        if self.id.is_nil() {
            self.id = Uuid::new_v4();
        }

        self.match_title = normalize_title(&self.match_title);
        self.display_name = if self.display_name.trim().is_empty() {
            self.match_title.clone()
        } else {
            self.display_name.trim().to_owned()
        };

        let mut aliases = Vec::with_capacity(self.aliases.len());
        let mut seen = HashSet::with_capacity(self.aliases.len());
        for alias in self.aliases {
            let alias = normalize_title(&alias);
            if !alias.is_empty() && alias != self.match_title && seen.insert(alias.clone()) {
                aliases.push(alias);
            }
        }
        self.aliases = aliases;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub enabled: bool,
    pub rule_mode: RuleMode,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub protected_chats: Vec<ProtectedChat>,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub exempted_chats: Vec<ProtectedChat>,
    #[serde(deserialize_with = "deserialize_default_on_null")]
    pub confirmation: ConfirmationSettings,
    pub unknown_context_behavior: UnknownContextBehavior,
    pub intercept_numpad_enter: bool,
    pub intercept_keyboard_enter: bool,
    #[serde(default = "default_true")]
    pub intercept_send_button: bool,
    pub shift_enter_pass_through: bool,
    pub start_with_windows: bool,
    pub log_retention_days: u32,
    /// Optional Windows adapter override. When absent, the adapter uses its built-in supported
    /// executable path instead of making the domain layer own a Windows file-system default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_weixin_executable_path: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            enabled: true,
            rule_mode: RuleMode::ProtectListed,
            protected_chats: Vec::new(),
            exempted_chats: Vec::new(),
            confirmation: ConfirmationSettings::default(),
            unknown_context_behavior: UnknownContextBehavior::Confirm,
            intercept_numpad_enter: true,
            intercept_keyboard_enter: true,
            intercept_send_button: true,
            shift_enter_pass_through: true,
            start_with_windows: false,
            log_retention_days: 7,
            trusted_weixin_executable_path: None,
        }
    }
}

impl AppSettings {
    pub fn sanitize(mut self) -> Self {
        self.schema_version = CURRENT_SCHEMA_VERSION;
        self.protected_chats = sanitize_chat_list(self.protected_chats);
        self.exempted_chats = sanitize_chat_list(self.exempted_chats);
        self.confirmation = self.confirmation.sanitize();
        self.shift_enter_pass_through = true;
        self.log_retention_days = self.log_retention_days.clamp(1, 30);
        self
    }
}

/// Preserves the title-normalization contract: trim and collapse whitespace into one ASCII space.
pub fn normalize_title(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut normalized = String::with_capacity(trimmed.len());
    let mut previous_was_whitespace = false;
    for character in trimmed.chars() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                normalized.push(' ');
            }
            previous_was_whitespace = true;
        } else {
            normalized.push(character);
            previous_was_whitespace = false;
        }
    }
    normalized
}

pub fn sanitize_chat_list(chats: impl IntoIterator<Item = ProtectedChat>) -> Vec<ProtectedChat> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for chat in chats {
        let chat = chat.sanitize();
        let key = (chat.target_kind, chat.match_title.clone());
        if !chat.match_title.is_empty() && seen.insert(key) {
            result.push(chat);
        }
    }

    result
}

#[derive(Debug, Clone)]
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<AppSettings> {
        if !self.path.is_file() {
            return Ok(AppSettings::default().sanitize());
        }

        let contents = fs::read(&self.path)?;
        match serde_json::from_slice::<AppSettings>(&contents) {
            Ok(settings) => Ok(settings.sanitize()),
            Err(_) => Ok(AppSettings::default().sanitize()),
        }
    }

    pub fn save(&self, settings: AppSettings) -> io::Result<()> {
        let settings = settings.sanitize();
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the settings path must have a parent directory",
            )
        })?;
        fs::create_dir_all(parent)?;

        let file_name = self.path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the settings path must include a file name",
            )
        })?;
        let temporary_path = parent.join(format!(
            ".{}.{}.tmp",
            file_name.to_string_lossy(),
            Uuid::new_v4().simple()
        ));

        let write_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)?;
            serde_json::to_writer_pretty(&mut file, &settings).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }

        if let Err(error) = fs::rename(&temporary_path, &self.path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedChatExport {
    pub schema_version: u32,
    pub protected_chats: Vec<ProtectedChat>,
}

#[derive(Debug)]
pub enum ImportError {
    InvalidJson(serde_json::Error),
    UnsupportedSchemaVersion(u32),
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid protected-chat export: {error}"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(
                    formatter,
                    "unsupported protected-chat export schema version: {version}"
                )
            }
        }
    }
}

impl std::error::Error for ImportError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtectedChatImport {
    schema_version: u32,
    #[serde(default, deserialize_with = "deserialize_default_on_null")]
    protected_chats: Vec<ProtectedChat>,
}

pub fn export_protected_chats(
    chats: impl IntoIterator<Item = ProtectedChat>,
) -> Result<String, serde_json::Error> {
    let payload = ProtectedChatExport {
        schema_version: CURRENT_SCHEMA_VERSION,
        protected_chats: sanitize_chat_list(chats),
    };
    serde_json::to_string_pretty(&payload)
}

pub fn import_protected_chats(json: &str) -> Result<Vec<ProtectedChat>, ImportError> {
    let payload: ProtectedChatImport =
        serde_json::from_str(json).map_err(ImportError::InvalidJson)?;
    if payload.schema_version != 1 && payload.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(ImportError::UnsupportedSchemaVersion(
            payload.schema_version,
        ));
    }
    Ok(sanitize_chat_list(payload.protected_chats))
}

fn deserialize_default_on_null<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}
