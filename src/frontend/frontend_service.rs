use crate::botty_boss;
use crate::io as botty_io;
use crate::io::transport::TransportPlugin;
use serde_json;
use std::fs;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const DEFAULT_BROWSER_USER_DATA_DIR: &str = "~/.mylittlebotty/app/browser/user_dir";
const DEFAULT_AI_PROFILE_NAME: &str = "default";

pub const COMMANDS: [&str; 10] = [
    "/setup",
    "/restart-server",
    "/new",
    "/remember",
    "/set-guy-env",
    "/list-guy-env",
    "/sub-agent",
    "/create-skill",
    "/exit",
    "/quit",
];
pub const CHATBOT_PROVIDERS: [&str; 3] = ["telegram", "feishu", "weixin"];
const CHAT_META_PREFIX: &str = "__botty_meta__";
const CONTROL_PREFIX: &str = "__botty_control__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupFieldId {
    AiProfiles,
    AgentProvider,
    AgentCodexCommand,
    AgentClaudeCommand,
    BrowserChromeCommand,
    BrowserChromeHeadless,
    BrowserChromeUserDataDir,
    BrowserChromeMaxTabs,
    WorkDir,
    ChatbotProvider,
}

impl SetupFieldId {
    pub const ALL: [SetupFieldId; 10] = [
        SetupFieldId::AiProfiles,
        SetupFieldId::AgentProvider,
        SetupFieldId::AgentCodexCommand,
        SetupFieldId::AgentClaudeCommand,
        SetupFieldId::BrowserChromeCommand,
        SetupFieldId::BrowserChromeHeadless,
        SetupFieldId::BrowserChromeUserDataDir,
        SetupFieldId::BrowserChromeMaxTabs,
        SetupFieldId::WorkDir,
        SetupFieldId::ChatbotProvider,
    ];

    pub fn from_index(index: usize) -> Self {
        Self::ALL
            .get(index)
            .copied()
            .unwrap_or(SetupFieldId::AiProfiles)
    }

    pub fn label(self) -> &'static str {
        match self {
            SetupFieldId::AiProfiles => "AI profiles",
            SetupFieldId::AgentProvider => "agent provider",
            SetupFieldId::AgentCodexCommand => "codex command",
            SetupFieldId::AgentClaudeCommand => "claude command",
            SetupFieldId::BrowserChromeCommand => "browser chrome command",
            SetupFieldId::BrowserChromeHeadless => "browser chrome headless",
            SetupFieldId::BrowserChromeUserDataDir => "browser chrome user data dir",
            SetupFieldId::BrowserChromeMaxTabs => "browser chrome max tabs",
            SetupFieldId::WorkDir => "work dir",
            SetupFieldId::ChatbotProvider => "chatbot providers",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(self, SetupFieldId::BrowserChromeHeadless)
    }

    pub fn is_masked(self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct AiProviderProfile {
    pub name: String,
    pub endpoint: String,
    pub apikey: String,
    pub model: String,
    pub debug: bool,
    pub vision: bool,
}

impl Default for AiProviderProfile {
    fn default() -> Self {
        Self {
            name: DEFAULT_AI_PROFILE_NAME.to_string(),
            endpoint: String::new(),
            apikey: String::new(),
            model: String::new(),
            debug: false,
            vision: false,
        }
    }
}

#[derive(Clone)]
pub struct SetupFieldView {
    pub label: &'static str,
    pub value: String,
    pub masked: bool,
}

#[derive(Clone)]
pub struct SetupConfig {
    pub ai_provider_active: String,
    pub ai_provider_profiles: Vec<AiProviderProfile>,
    pub agent_provider: String,
    pub agent_codex_command: String,
    pub agent_claude_command: String,
    pub browser_chrome_command: String,
    pub browser_chrome_headless: bool,
    pub browser_chrome_user_data_dir: String,
    pub browser_chrome_max_tabs: usize,
    pub work_dir: String,
    pub chatbot_provider: String,
    pub chatbot_telegram_api_base: String,
    pub chatbot_telegram_apikey: String,
    pub chatbot_feishu_api_base: String,
    pub chatbot_feishu_app_id: String,
    pub chatbot_feishu_app_secret: String,
    pub chatbot_feishu_access_token: String,
    pub chatbot_telegram_enabled: bool,
    pub chatbot_feishu_enabled: bool,
    pub chatbot_telegram_whitelist_user_ids: String,
    pub chatbot_telegram_poll_interval_seconds: u64,
    pub chatbot_feishu_poll_interval_seconds: u64,
    pub chatbot_feishu_chat_id: String,
    pub chatbot_weixin_enabled: bool,
    pub chatbot_weixin_api_base: String,
    pub chatbot_weixin_cdn_base: String,
    pub chatbot_weixin_apikey: String,
    pub chatbot_weixin_account_id: String,
    pub chatbot_weixin_user_id: String,
    pub chatbot_weixin_whitelist_user_ids: String,
    pub chatbot_weixin_poll_interval_seconds: u64,
    pub chatbot_weixin_long_poll_timeout_ms: u64,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            ai_provider_active: DEFAULT_AI_PROFILE_NAME.to_string(),
            ai_provider_profiles: vec![AiProviderProfile::default()],
            agent_provider: "codex".to_string(),
            agent_codex_command: "codex".to_string(),
            agent_claude_command: "claude".to_string(),
            browser_chrome_command: String::new(),
            browser_chrome_headless: false,
            browser_chrome_user_data_dir: DEFAULT_BROWSER_USER_DATA_DIR.to_string(),
            browser_chrome_max_tabs: 10,
            work_dir: botty_io::default_work_dir_display(),
            chatbot_provider: "telegram".to_string(),
            chatbot_telegram_api_base: "https://api.telegram.org".to_string(),
            chatbot_telegram_apikey: String::new(),
            chatbot_feishu_api_base: "https://open.feishu.cn/open-apis".to_string(),
            chatbot_feishu_app_id: String::new(),
            chatbot_feishu_app_secret: String::new(),
            chatbot_feishu_access_token: String::new(),
            chatbot_telegram_enabled: true,
            chatbot_feishu_enabled: false,
            chatbot_telegram_whitelist_user_ids: String::new(),
            chatbot_telegram_poll_interval_seconds: 1,
            chatbot_feishu_poll_interval_seconds: 1,
            chatbot_feishu_chat_id: String::new(),
            chatbot_weixin_enabled: false,
            chatbot_weixin_api_base: "https://ilinkai.weixin.qq.com".to_string(),
            chatbot_weixin_cdn_base: "https://novac2c.cdn.weixin.qq.com/c2c".to_string(),
            chatbot_weixin_apikey: String::new(),
            chatbot_weixin_account_id: String::new(),
            chatbot_weixin_user_id: String::new(),
            chatbot_weixin_whitelist_user_ids: String::new(),
            chatbot_weixin_poll_interval_seconds: 1,
            chatbot_weixin_long_poll_timeout_ms: 35_000,
        }
    }
}

impl SetupConfig {
    pub fn selected_provider_index(&self) -> usize {
        CHATBOT_PROVIDERS
            .iter()
            .position(|provider| *provider == self.chatbot_provider)
            .unwrap_or(0)
    }

    pub fn fields(&self) -> Vec<SetupFieldView> {
        SetupFieldId::ALL
            .iter()
            .copied()
            .map(|id| SetupFieldView {
                label: id.label(),
                value: self.field_value(id),
                masked: id.is_masked(),
            })
            .collect()
    }

    pub fn field_value(&self, field: SetupFieldId) -> String {
        match field {
            SetupFieldId::AiProfiles => self.ai_profile_summary(),
            SetupFieldId::AgentProvider => self.agent_provider.clone(),
            SetupFieldId::AgentCodexCommand => self.agent_codex_command.clone(),
            SetupFieldId::AgentClaudeCommand => self.agent_claude_command.clone(),
            SetupFieldId::BrowserChromeCommand => self.browser_chrome_command.clone(),
            SetupFieldId::BrowserChromeHeadless => {
                if self.browser_chrome_headless {
                    "[x] true".to_string()
                } else {
                    "[ ] false".to_string()
                }
            }
            SetupFieldId::BrowserChromeUserDataDir => self.browser_chrome_user_data_dir.clone(),
            SetupFieldId::BrowserChromeMaxTabs => self.browser_chrome_max_tabs.to_string(),
            SetupFieldId::WorkDir => self.work_dir.clone(),
            SetupFieldId::ChatbotProvider => self.chatbot_provider_summary(),
        }
    }

    pub fn editable_value(&self, field: SetupFieldId) -> String {
        match field {
            SetupFieldId::AiProfiles => String::new(),
            SetupFieldId::AgentProvider => self.agent_provider.clone(),
            SetupFieldId::AgentCodexCommand => self.agent_codex_command.clone(),
            SetupFieldId::AgentClaudeCommand => self.agent_claude_command.clone(),
            SetupFieldId::BrowserChromeCommand => self.browser_chrome_command.clone(),
            SetupFieldId::BrowserChromeHeadless => String::new(),
            SetupFieldId::BrowserChromeUserDataDir => self.browser_chrome_user_data_dir.clone(),
            SetupFieldId::BrowserChromeMaxTabs => self.browser_chrome_max_tabs.to_string(),
            SetupFieldId::WorkDir => self.work_dir.clone(),
            SetupFieldId::ChatbotProvider => self.chatbot_provider.clone(),
        }
    }

    pub fn set_field(&mut self, field: SetupFieldId, value: &str) {
        match field {
            SetupFieldId::AiProfiles => {}
            SetupFieldId::AgentProvider => self.agent_provider = value.trim().to_ascii_lowercase(),
            SetupFieldId::AgentCodexCommand => self.agent_codex_command = value.to_string(),
            SetupFieldId::AgentClaudeCommand => self.agent_claude_command = value.to_string(),
            SetupFieldId::BrowserChromeCommand => self.browser_chrome_command = value.to_string(),
            SetupFieldId::BrowserChromeHeadless => {}
            SetupFieldId::BrowserChromeUserDataDir => {
                self.browser_chrome_user_data_dir = value.to_string()
            }
            SetupFieldId::BrowserChromeMaxTabs => {
                if let Ok(max_tabs) = value.trim().parse::<usize>() {
                    self.browser_chrome_max_tabs = max_tabs;
                }
            }
            SetupFieldId::WorkDir => {
                self.work_dir = botty_io::normalize_work_dir_input(value);
            }
            SetupFieldId::ChatbotProvider => self.chatbot_provider = value.to_string(),
        }
    }

    pub fn toggle_field(&mut self, field: SetupFieldId) {
        match field {
            SetupFieldId::BrowserChromeHeadless => {
                self.browser_chrome_headless = !self.browser_chrome_headless
            }
            _ => {}
        }
    }

    pub fn active_ai_profile_index(&self) -> usize {
        self.ai_provider_profiles
            .iter()
            .position(|profile| profile.name == self.ai_provider_active)
            .unwrap_or(0)
    }

    pub fn active_ai_profile(&self) -> &AiProviderProfile {
        let index = self.active_ai_profile_index();
        &self.ai_provider_profiles[index.min(self.ai_provider_profiles.len().saturating_sub(1))]
    }

    pub fn image_ai_profile_name(&self) -> Option<&str> {
        let active = self.active_ai_profile();
        if active.vision {
            return Some(active.name.as_str());
        }

        self.ai_provider_profiles
            .iter()
            .find(|profile| profile.vision)
            .map(|profile| profile.name.as_str())
    }

    pub fn ai_profile_summary(&self) -> String {
        format!(
            "active: {} ({} profiles)",
            self.ai_provider_active,
            self.ai_provider_profiles.len()
        )
    }

    pub fn chatbot_provider_summary(&self) -> String {
        let enabled = enabled_provider_list(self);
        if enabled.is_empty() {
            "none enabled".to_string()
        } else {
            enabled
        }
    }

    pub fn activate_ai_profile_by_index(&mut self, index: usize) {
        if let Some(profile) = self.ai_provider_profiles.get(index) {
            self.ai_provider_active = profile.name.clone();
        }
    }

    pub fn upsert_ai_profile(
        &mut self,
        original_name: Option<&str>,
        mut profile: AiProviderProfile,
    ) -> io::Result<()> {
        profile.name = profile.name.trim().to_string();
        validate_ai_profile_name(profile.name.as_str())?;
        if self
            .ai_provider_profiles
            .iter()
            .any(|saved| saved.name == profile.name && Some(saved.name.as_str()) != original_name)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AI profile '{}' already exists", profile.name),
            ));
        }

        if let Some(original_name) = original_name {
            if let Some(saved) = self
                .ai_provider_profiles
                .iter_mut()
                .find(|saved| saved.name == original_name)
            {
                let renamed_active = self.ai_provider_active == original_name;
                *saved = profile.clone();
                if renamed_active {
                    self.ai_provider_active = profile.name.clone();
                }
                return Ok(());
            }
        }

        self.ai_provider_profiles.push(profile);
        self.ai_provider_profiles
            .sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Ok(())
    }

    pub fn delete_ai_profile(&mut self, index: usize) -> io::Result<()> {
        let Some(profile) = self.ai_provider_profiles.get(index) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "AI profile does not exist",
            ));
        };
        if profile.name == self.ai_provider_active {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "current AI profile is active; switch to another profile before deleting",
            ));
        }
        if self.ai_provider_profiles.len() <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one AI profile is required",
            ));
        }
        self.ai_provider_profiles.remove(index);
        Ok(())
    }

    pub fn cycle_provider(&mut self, selected_provider: &mut usize, delta: i32) {
        let len = CHATBOT_PROVIDERS.len() as i32;
        if len == 0 {
            return;
        }

        let next = (*selected_provider as i32 + delta).rem_euclid(len);
        *selected_provider = next as usize;
        self.chatbot_provider = CHATBOT_PROVIDERS[*selected_provider].to_string();
    }
}

pub enum RestartStatus {
    Success(String),
    Failed(String),
}

pub enum FrontendRequest {
    SendChat { message: String },
    LoadSetup,
    RestartServer,
    SaveSetup { config: SetupConfig },
    SetGuyEnv { key: String, value: String },
    ListGuyEnv,
}

pub enum FrontendResponse {
    ChatReply { reply: String },
    SetupLoaded { config: SetupConfig },
    ServerRestarted { status: RestartStatus },
    SetupSaved { result: SaveSetupResult },
    GuyEnvSet { result: GuyEnvSetResult },
    GuyEnvListed { entries: Vec<(String, String)> },
}

pub struct SaveSetupResult {
    pub config_path: PathBuf,
    pub work_dir_config_path: PathBuf,
    pub migrated_work_dir: Option<(PathBuf, PathBuf)>,
    pub restart_status: RestartStatus,
}

pub struct GuyEnvSetResult {
    pub config_path: PathBuf,
    pub applied_live: bool,
}

pub trait FrontendRpc {
    fn call(&mut self, request: FrontendRequest) -> io::Result<FrontendResponse>;

    fn send_chat(&mut self, message: &str) -> io::Result<String> {
        match self.call(FrontendRequest::SendChat {
            message: message.to_string(),
        })? {
            FrontendResponse::ChatReply { reply } => Ok(reply),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SendChat",
            )),
        }
    }

    fn load_setup(&mut self) -> io::Result<SetupConfig> {
        match self.call(FrontendRequest::LoadSetup)? {
            FrontendResponse::SetupLoaded { config } => Ok(config),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for LoadSetup",
            )),
        }
    }

    fn restart_server(&mut self) -> io::Result<RestartStatus> {
        match self.call(FrontendRequest::RestartServer)? {
            FrontendResponse::ServerRestarted { status } => Ok(status),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for RestartServer",
            )),
        }
    }

    fn save_setup(&mut self, config: &SetupConfig) -> io::Result<SaveSetupResult> {
        match self.call(FrontendRequest::SaveSetup {
            config: config.clone(),
        })? {
            FrontendResponse::SetupSaved { result } => Ok(result),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SaveSetup",
            )),
        }
    }

    fn set_guy_env(&mut self, key: &str, value: &str) -> io::Result<GuyEnvSetResult> {
        match self.call(FrontendRequest::SetGuyEnv {
            key: key.to_string(),
            value: value.to_string(),
        })? {
            FrontendResponse::GuyEnvSet { result } => Ok(result),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SetGuyEnv",
            )),
        }
    }

    fn list_guy_env(&mut self) -> io::Result<Vec<(String, String)>> {
        match self.call(FrontendRequest::ListGuyEnv)? {
            FrontendResponse::GuyEnvListed { entries } => Ok(entries),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for ListGuyEnv",
            )),
        }
    }
}

pub struct LocalFrontendRpc {
    socket_path: PathBuf,
    transport: BossSocketTransport,
}

impl LocalFrontendRpc {
    pub fn connect() -> io::Result<Self> {
        botty_boss::ensure_chat_ready()?;
        let socket_path = botty_boss::chat_socket_path();
        let transport = BossSocketTransport::connect(&socket_path)?;
        Ok(Self {
            socket_path,
            transport,
        })
    }
}

impl FrontendRpc for LocalFrontendRpc {
    fn call(&mut self, request: FrontendRequest) -> io::Result<FrontendResponse> {
        match request {
            FrontendRequest::SendChat { message } => Ok(FrontendResponse::ChatReply {
                reply: request_with_reconnect(&mut self.transport, &self.socket_path, &message)?,
            }),
            FrontendRequest::LoadSetup => Ok(FrontendResponse::SetupLoaded {
                config: load_setup_config()?,
            }),
            FrontendRequest::RestartServer => {
                let status = match botty_boss::restart_all_report() {
                    Ok(lines) => RestartStatus::Success(lines.join("\n")),
                    Err(err) => RestartStatus::Failed(format!("Restart failed: {err}")),
                };
                Ok(FrontendResponse::ServerRestarted { status })
            }
            FrontendRequest::SaveSetup { config } => {
                let path = setup_config_file();
                let previous_work_dir = botty_io::effective_work_dir()?;
                let next_work_dir = botty_io::resolve_work_dir_input(&config.work_dir);
                validate_setup_config(&config)?;
                save_setup_config(&config)?;
                if previous_work_dir != next_work_dir {
                    migrate_work_dir_contents(&previous_work_dir, &next_work_dir)?;
                } else {
                    fs::create_dir_all(&next_work_dir)?;
                }
                let work_dir_config_path = botty_io::save_work_dir_setting(&config.work_dir)?;
                let restart_status = match botty_boss::restart_all_report() {
                    Ok(lines) => RestartStatus::Success(lines.join("\n")),
                    Err(err) => RestartStatus::Failed(format!("Auto restart failed: {err}")),
                };

                Ok(FrontendResponse::SetupSaved {
                    result: SaveSetupResult {
                        config_path: path,
                        work_dir_config_path,
                        migrated_work_dir: if previous_work_dir != next_work_dir {
                            Some((previous_work_dir, next_work_dir))
                        } else {
                            None
                        },
                        restart_status,
                    },
                })
            }
            FrontendRequest::SetGuyEnv { key, value } => {
                let path = guy_env_config_file();
                save_guy_env_entry(&key, &value)?;
                let applied_live = request_with_reconnect(
                    &mut self.transport,
                    &self.socket_path,
                    &format!("{CONTROL_PREFIX}set-env|{key}|{value}"),
                )
                .is_ok();
                Ok(FrontendResponse::GuyEnvSet {
                    result: GuyEnvSetResult {
                        config_path: path,
                        applied_live,
                    },
                })
            }
            FrontendRequest::ListGuyEnv => Ok(FrontendResponse::GuyEnvListed {
                entries: botty_boss::load_guy_env_map()?,
            }),
        }
    }
}

pub fn command_suggestions(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    COMMANDS
        .iter()
        .copied()
        .filter(|cmd| cmd.starts_with(input))
        .collect()
}

pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }

    let visible = value.chars().count().min(4);
    let masked_len = value.chars().count().saturating_sub(visible);
    let suffix: String = value
        .chars()
        .rev()
        .take(visible)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(masked_len), suffix)
}

fn request_with_reconnect(
    transport: &mut BossSocketTransport,
    socket_path: &PathBuf,
    message: &str,
) -> io::Result<String> {
    match transport.request(message) {
        Ok(reply) => Ok(reply),
        Err(_) => {
            *transport = BossSocketTransport::connect(socket_path)?;
            transport.request(message)
        }
    }
}

struct BossSocketTransport {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

impl BossSocketTransport {
    fn connect(path: &PathBuf) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);
        Ok(Self { reader, writer })
    }
}

impl TransportPlugin for BossSocketTransport {
    fn request(&mut self, message: &str) -> io::Result<String> {
        let payload = encode_meta_message("tui", "tui", message);
        writeln!(self.writer, "{}", encode_ipc_line(&payload)?)?;
        self.writer.flush()?;

        let mut reply = String::new();
        let bytes = self.reader.read_line(&mut reply)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Botty-Boss closed connection",
            ));
        }
        decode_ipc_line(reply.trim_end())
    }
}

fn encode_meta_message(source: &str, user_id: &str, message: &str) -> String {
    format!("{CHAT_META_PREFIX}|source={source}|user_id={user_id}|{message}")
}

fn encode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::to_string(value)
        .map_err(|err| io::Error::other(format!("encode ipc line failed: {err}")))
}

fn decode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::from_str(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decode ipc line failed: {err}"),
        )
    })
}

fn load_setup_config() -> io::Result<SetupConfig> {
    let path = setup_config_file();
    let mut config = SetupConfig::default();
    config.work_dir = botty_io::load_work_dir_setting()?;
    config.ai_provider_profiles.clear();
    let mut legacy_profile = AiProviderProfile::default();
    let mut saw_legacy_ai_profile = false;
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(config),
        Err(err) => return Err(err),
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "ai.provider.active" => config.ai_provider_active = value.to_string(),
            "agent.provider" => config.agent_provider = value.to_ascii_lowercase(),
            "agent.codex.command" => config.agent_codex_command = value.to_string(),
            "agent.claude.command" => config.agent_claude_command = value.to_string(),
            "browser.chrome.command" => config.browser_chrome_command = value.to_string(),
            "browser.chrome.headless" => config.browser_chrome_headless = parse_bool(value),
            "browser.chrome.user_data_dir" => {
                config.browser_chrome_user_data_dir = value.to_string()
            }
            "browser.chrome.max_tabs" => {
                if let Ok(max_tabs) = value.parse::<usize>() {
                    config.browser_chrome_max_tabs = max_tabs;
                }
            }
            "chatbot.provider" => apply_chatbot_provider_list(&mut config, value),
            "chatbot.telegram.api_base" => config.chatbot_telegram_api_base = value.to_string(),
            "chatbot.telegram.apikey" => config.chatbot_telegram_apikey = value.to_string(),
            "chatbot.feishu.api_base" => config.chatbot_feishu_api_base = value.to_string(),
            "chatbot.feishu.app_id" => config.chatbot_feishu_app_id = value.to_string(),
            "chatbot.feishu.app_secret" => config.chatbot_feishu_app_secret = value.to_string(),
            "chatbot.feishu.apikey" => config.chatbot_feishu_access_token = value.to_string(),
            "chatbot.weixin.api_base" => config.chatbot_weixin_api_base = value.to_string(),
            "chatbot.weixin.cdn_base" => config.chatbot_weixin_cdn_base = value.to_string(),
            "chatbot.weixin.apikey" => config.chatbot_weixin_apikey = value.to_string(),
            "chatbot.weixin.account_id" => config.chatbot_weixin_account_id = value.to_string(),
            "chatbot.weixin.user_id" => config.chatbot_weixin_user_id = value.to_string(),
            "chatbot.apikey" => {
                if config.chatbot_provider == "feishu" {
                    config.chatbot_feishu_access_token = value.to_string();
                } else if config.chatbot_provider == "weixin" {
                    config.chatbot_weixin_apikey = value.to_string();
                } else {
                    config.chatbot_telegram_apikey = value.to_string();
                }
            }
            "chatbot.telegram.enabled" => config.chatbot_telegram_enabled = parse_bool(value),
            "chatbot.feishu.enabled" => config.chatbot_feishu_enabled = parse_bool(value),
            "chatbot.weixin.enabled" => config.chatbot_weixin_enabled = parse_bool(value),
            "chatbot.telegram.whitelist_user_ids" => {
                config.chatbot_telegram_whitelist_user_ids = value.to_string()
            }
            "chatbot.feishu.chat_id" => config.chatbot_feishu_chat_id = value.to_string(),
            "chatbot.weixin.whitelist_user_ids" => {
                config.chatbot_weixin_whitelist_user_ids = value.to_string()
            }
            "chatbot.telegram.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.chatbot_telegram_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.feishu.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.chatbot_feishu_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.weixin.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.chatbot_weixin_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.weixin.long_poll_timeout_ms" => {
                if let Ok(timeout_ms) = value.parse::<u64>() {
                    config.chatbot_weixin_long_poll_timeout_ms = timeout_ms.max(1_000);
                }
            }
            "ai.provider.endpoint" | "provider.endpoint" => {
                legacy_profile.endpoint = value.to_string();
                saw_legacy_ai_profile = true;
            }
            "ai.provider.apikey" | "provider.apikey" => {
                legacy_profile.apikey = value.to_string();
                saw_legacy_ai_profile = true;
            }
            "ai.provider.model" | "provider.model" => {
                legacy_profile.model = value.to_string();
                saw_legacy_ai_profile = true;
            }
            "ai.provider.debug" | "provider.debug" => {
                legacy_profile.debug = parse_bool(value);
                saw_legacy_ai_profile = true;
            }
            "ai.provider.vision" | "provider.vision" => {
                legacy_profile.vision = parse_bool(value);
                saw_legacy_ai_profile = true;
            }
            other => {
                if let Some((profile_name, field_name)) = parse_ai_profile_key(other) {
                    let profile =
                        ensure_ai_profile_slot(&mut config.ai_provider_profiles, profile_name);
                    match field_name {
                        "endpoint" => profile.endpoint = value.to_string(),
                        "apikey" => profile.apikey = value.to_string(),
                        "model" => profile.model = value.to_string(),
                        "debug" => profile.debug = parse_bool(value),
                        "vision" => profile.vision = parse_bool(value),
                        _ => {}
                    }
                }
            }
        }
    }

    if config.ai_provider_profiles.is_empty() && saw_legacy_ai_profile {
        config.ai_provider_profiles.push(legacy_profile);
    }
    if config.ai_provider_profiles.is_empty() {
        config
            .ai_provider_profiles
            .push(AiProviderProfile::default());
    }
    config
        .ai_provider_profiles
        .sort_unstable_by(|left, right| left.name.cmp(&right.name));
    if config.ai_provider_active.trim().is_empty() {
        config.ai_provider_active = config.active_ai_profile().name.clone();
    } else if !config
        .ai_provider_profiles
        .iter()
        .any(|profile| profile.name == config.ai_provider_active)
    {
        config.ai_provider_active = config.active_ai_profile().name.clone();
    }

    Ok(config)
}

fn save_setup_config(config: &SetupConfig) -> io::Result<()> {
    let path = setup_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = format!(
        "ai.provider.active={}\n{}agent.provider={}\nagent.codex.command={}\nagent.claude.command={}\nbrowser.chrome.command={}\nbrowser.chrome.headless={}\nbrowser.chrome.user_data_dir={}\nbrowser.chrome.max_tabs={}\nchatbot.provider={}\nchatbot.telegram.api_base={}\nchatbot.telegram.apikey={}\nchatbot.feishu.api_base={}\nchatbot.feishu.app_id={}\nchatbot.feishu.app_secret={}\nchatbot.feishu.apikey={}\nchatbot.weixin.api_base={}\nchatbot.weixin.cdn_base={}\nchatbot.weixin.apikey={}\nchatbot.weixin.account_id={}\nchatbot.weixin.user_id={}\nchatbot.telegram.enabled={}\nchatbot.feishu.enabled={}\nchatbot.weixin.enabled={}\nchatbot.telegram.whitelist_user_ids={}\nchatbot.weixin.whitelist_user_ids={}\nchatbot.telegram.poll_interval_seconds={}\nchatbot.feishu.poll_interval_seconds={}\nchatbot.weixin.poll_interval_seconds={}\nchatbot.weixin.long_poll_timeout_ms={}\nchatbot.feishu.chat_id={}\n",
        config.ai_provider_active,
        serialize_ai_profiles(&config.ai_provider_profiles),
        config.agent_provider,
        config.agent_codex_command,
        config.agent_claude_command,
        config.browser_chrome_command,
        config.browser_chrome_headless,
        config.browser_chrome_user_data_dir,
        config.browser_chrome_max_tabs,
        enabled_provider_list(config),
        config.chatbot_telegram_api_base,
        config.chatbot_telegram_apikey,
        config.chatbot_feishu_api_base,
        config.chatbot_feishu_app_id,
        config.chatbot_feishu_app_secret,
        config.chatbot_feishu_access_token,
        config.chatbot_weixin_api_base,
        config.chatbot_weixin_cdn_base,
        config.chatbot_weixin_apikey,
        config.chatbot_weixin_account_id,
        config.chatbot_weixin_user_id,
        config.chatbot_telegram_enabled,
        config.chatbot_feishu_enabled,
        config.chatbot_weixin_enabled,
        config.chatbot_telegram_whitelist_user_ids,
        config.chatbot_weixin_whitelist_user_ids,
        config.chatbot_telegram_poll_interval_seconds,
        config.chatbot_feishu_poll_interval_seconds,
        config.chatbot_weixin_poll_interval_seconds,
        config.chatbot_weixin_long_poll_timeout_ms,
        config.chatbot_feishu_chat_id
    );

    fs::write(path, content)
}

fn validate_setup_config(config: &SetupConfig) -> io::Result<()> {
    if config.ai_provider_profiles.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one AI profile is required",
        ));
    }

    for profile in &config.ai_provider_profiles {
        validate_ai_profile_name(profile.name.as_str())?;
    }

    if !config
        .ai_provider_profiles
        .iter()
        .any(|profile| profile.name == config.ai_provider_active)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "active AI profile does not exist",
        ));
    }

    if config.chatbot_telegram_enabled && config.chatbot_telegram_apikey.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "telegram is enabled but chatbot.telegram.apikey is empty",
        ));
    }

    if config.chatbot_feishu_enabled {
        let has_app_credentials = !config.chatbot_feishu_app_id.trim().is_empty()
            && !config.chatbot_feishu_app_secret.trim().is_empty();
        if !has_app_credentials {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "feishu is enabled but chatbot.feishu.app_id/app_secret is incomplete for long connection",
            ));
        }
    }

    if config.chatbot_weixin_enabled && config.chatbot_weixin_apikey.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "weixin is enabled but chatbot.weixin.apikey is empty",
        ));
    }

    Ok(())
}

fn save_guy_env_entry(key: &str, value: &str) -> io::Result<()> {
    validate_env_key(key)?;

    let mut entries = botty_boss::load_guy_env_map()?;
    if let Some((_, saved_value)) = entries.iter_mut().find(|(saved_key, _)| saved_key == key) {
        *saved_value = value.to_string();
    } else {
        entries.push((key.to_string(), value.to_string()));
    }
    botty_boss::save_guy_env_map(&entries)
}

fn validate_env_key(key: &str) -> io::Result<()> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "env key cannot be empty",
        ));
    };

    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "env key must start with A-Z, a-z, or _",
        ));
    }

    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "env key may only contain A-Z, a-z, 0-9, or _",
        ));
    }

    Ok(())
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn validate_ai_profile_name(name: &str) -> io::Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AI profile name cannot be empty",
        ));
    }
    if trimmed
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AI profile name may only contain A-Z, a-z, 0-9, - or _",
        ));
    }
    Ok(())
}

fn parse_ai_profile_key(key: &str) -> Option<(&str, &str)> {
    let key = key.strip_prefix("ai.provider.")?;
    let (profile_name, field_name) = key.split_once('.')?;
    if profile_name.is_empty() {
        return None;
    }
    Some((profile_name, field_name))
}

fn ensure_ai_profile_slot<'a>(
    profiles: &'a mut Vec<AiProviderProfile>,
    profile_name: &str,
) -> &'a mut AiProviderProfile {
    if let Some(index) = profiles
        .iter()
        .position(|profile| profile.name == profile_name)
    {
        return &mut profiles[index];
    }

    profiles.push(AiProviderProfile {
        name: profile_name.to_string(),
        ..AiProviderProfile::default()
    });
    let index = profiles.len().saturating_sub(1);
    &mut profiles[index]
}

fn serialize_ai_profiles(profiles: &[AiProviderProfile]) -> String {
    let mut lines = String::new();
    for profile in profiles {
        lines.push_str(&format!(
            "ai.provider.{}.endpoint={}\nai.provider.{}.apikey={}\nai.provider.{}.model={}\nai.provider.{}.debug={}\nai.provider.{}.vision={}\n",
            profile.name,
            profile.endpoint,
            profile.name,
            profile.apikey,
            profile.name,
            profile.model,
            profile.name,
            profile.debug,
            profile.name,
            profile.vision
        ));
    }
    lines
}

fn apply_chatbot_provider_list(config: &mut SetupConfig, value: &str) {
    config.chatbot_telegram_enabled = false;
    config.chatbot_feishu_enabled = false;
    config.chatbot_weixin_enabled = false;

    let mut first_enabled = None;
    for item in value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        match item {
            "telegram" => {
                config.chatbot_telegram_enabled = true;
                if first_enabled.is_none() {
                    first_enabled = Some("telegram");
                }
            }
            "feishu" => {
                config.chatbot_feishu_enabled = true;
                if first_enabled.is_none() {
                    first_enabled = Some("feishu");
                }
            }
            "weixin" => {
                config.chatbot_weixin_enabled = true;
                if first_enabled.is_none() {
                    first_enabled = Some("weixin");
                }
            }
            _ => {}
        }
    }

    config.chatbot_provider = first_enabled.unwrap_or("telegram").to_string();
}

fn enabled_provider_list(config: &SetupConfig) -> String {
    let mut list = Vec::new();
    if config.chatbot_telegram_enabled {
        list.push("telegram");
    }
    if config.chatbot_feishu_enabled {
        list.push("feishu");
    }
    if config.chatbot_weixin_enabled {
        list.push("weixin");
    }
    list.join(",")
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn guy_env_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("guy-env{}.conf", runtime_suffix()))
}

fn botty_root_dir() -> PathBuf {
    botty_io::config_root_dir()
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}

fn migrate_work_dir_contents(from: &PathBuf, to: &PathBuf) -> io::Result<()> {
    if !from.exists() || from == to {
        fs::create_dir_all(to)?;
        return Ok(());
    }

    if botty_io::paths_overlap(from, to) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "work dir migration does not support nested paths: {} -> {}",
                from.display(),
                to.display()
            ),
        ));
    }

    fs::create_dir_all(to)?;
    move_dir_contents(from, to)?;
    remove_empty_dir_tree(from)
}

fn move_dir_contents(from: &PathBuf, to: &PathBuf) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        move_path(&entry.path(), &target)?;
    }
    Ok(())
}

fn move_path(from: &PathBuf, to: &PathBuf) -> io::Result<()> {
    let metadata = fs::symlink_metadata(from)?;
    if metadata.is_dir() {
        if to.exists() {
            let target_metadata = fs::symlink_metadata(to)?;
            if !target_metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("migration target already exists: {}", to.display()),
                ));
            }
            fs::create_dir_all(to)?;
            move_dir_contents(from, to)?;
            fs::remove_dir(from)?;
            return Ok(());
        }

        match fs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                copy_dir_recursive(from, to)?;
                fs::remove_dir_all(from)
            }
            Err(err) => Err(err),
        }
    } else {
        if to.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("migration target already exists: {}", to.display()),
            ));
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                fs::copy(from, to)?;
                fs::remove_file(from)
            }
            Err(err) => Err(err),
        }
    }
}

fn copy_dir_recursive(from: &PathBuf, to: &PathBuf) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else {
            if target.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("migration target already exists: {}", target.display()),
                ));
            }
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn remove_empty_dir_tree(path: &PathBuf) -> io::Result<()> {
    if path.exists() {
        fs::remove_dir(path)?;
    }
    Ok(())
}
