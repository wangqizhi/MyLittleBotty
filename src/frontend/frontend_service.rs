use crate::botty_boss;
use crate::io::transport::TransportPlugin;
use serde_json;
use std::env;
use std::fs;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub const COMMANDS: [&str; 6] = [
    "/setup",
    "/restart-server",
    "/new",
    "/remember",
    "/exit",
    "/quit",
];
pub const CHATBOT_PROVIDERS: [&str; 2] = ["telegram", "feishu"];
const CHAT_META_PREFIX: &str = "__botty_meta__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupFieldId {
    AiProviderEndpoint,
    AiProviderApikey,
    AiProviderModel,
    AiProviderDebug,
    ChatbotProvider,
    TelegramEnabled,
    FeishuEnabled,
    TelegramPollSeconds,
    TelegramWhitelistUserIds,
    FeishuChatId,
}

impl SetupFieldId {
    pub const ALL: [SetupFieldId; 10] = [
        SetupFieldId::AiProviderEndpoint,
        SetupFieldId::AiProviderApikey,
        SetupFieldId::AiProviderModel,
        SetupFieldId::AiProviderDebug,
        SetupFieldId::ChatbotProvider,
        SetupFieldId::TelegramEnabled,
        SetupFieldId::FeishuEnabled,
        SetupFieldId::TelegramPollSeconds,
        SetupFieldId::TelegramWhitelistUserIds,
        SetupFieldId::FeishuChatId,
    ];

    pub fn from_index(index: usize) -> Self {
        Self::ALL
            .get(index)
            .copied()
            .unwrap_or(SetupFieldId::AiProviderEndpoint)
    }

    pub fn label(self) -> &'static str {
        match self {
            SetupFieldId::AiProviderEndpoint => "AI provider endpoint",
            SetupFieldId::AiProviderApikey => "AI provider apikey",
            SetupFieldId::AiProviderModel => "AI provider model",
            SetupFieldId::AiProviderDebug => "AI provider debug",
            SetupFieldId::ChatbotProvider => "chatbot provider",
            SetupFieldId::TelegramEnabled => "telegram enabled",
            SetupFieldId::FeishuEnabled => "feishu enabled",
            SetupFieldId::TelegramPollSeconds => "telegram poll seconds",
            SetupFieldId::TelegramWhitelistUserIds => "telegram whitelist user_ids",
            SetupFieldId::FeishuChatId => "feishu chat id",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            SetupFieldId::AiProviderDebug
                | SetupFieldId::TelegramEnabled
                | SetupFieldId::FeishuEnabled
        )
    }

    pub fn is_masked(self) -> bool {
        matches!(self, SetupFieldId::AiProviderApikey)
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
    pub ai_provider_endpoint: String,
    pub ai_provider_apikey: String,
    pub ai_provider_model: String,
    pub ai_provider_debug: bool,
    pub chatbot_provider: String,
    pub chatbot_telegram_api_base: String,
    pub chatbot_telegram_apikey: String,
    pub chatbot_feishu_api_base: String,
    pub chatbot_feishu_apikey: String,
    pub chatbot_telegram_enabled: bool,
    pub chatbot_feishu_enabled: bool,
    pub chatbot_telegram_whitelist_user_ids: String,
    pub chatbot_telegram_poll_interval_seconds: u64,
    pub chatbot_feishu_poll_interval_seconds: u64,
    pub chatbot_feishu_chat_id: String,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            ai_provider_endpoint: String::new(),
            ai_provider_apikey: String::new(),
            ai_provider_model: "MiniMax-M2.1".to_string(),
            ai_provider_debug: false,
            chatbot_provider: "telegram".to_string(),
            chatbot_telegram_api_base: "https://api.telegram.org".to_string(),
            chatbot_telegram_apikey: String::new(),
            chatbot_feishu_api_base: "https://open.feishu.cn/open-apis".to_string(),
            chatbot_feishu_apikey: String::new(),
            chatbot_telegram_enabled: true,
            chatbot_feishu_enabled: false,
            chatbot_telegram_whitelist_user_ids: String::new(),
            chatbot_telegram_poll_interval_seconds: 1,
            chatbot_feishu_poll_interval_seconds: 1,
            chatbot_feishu_chat_id: String::new(),
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
            SetupFieldId::AiProviderEndpoint => self.ai_provider_endpoint.clone(),
            SetupFieldId::AiProviderApikey => self.ai_provider_apikey.clone(),
            SetupFieldId::AiProviderModel => self.ai_provider_model.clone(),
            SetupFieldId::AiProviderDebug => {
                if self.ai_provider_debug {
                    "[x] true".to_string()
                } else {
                    "[ ] false".to_string()
                }
            }
            SetupFieldId::ChatbotProvider => self.chatbot_provider.clone(),
            SetupFieldId::TelegramEnabled => {
                if self.chatbot_telegram_enabled {
                    "[x] true".to_string()
                } else {
                    "[ ] false".to_string()
                }
            }
            SetupFieldId::FeishuEnabled => {
                if self.chatbot_feishu_enabled {
                    "[x] true".to_string()
                } else {
                    "[ ] false".to_string()
                }
            }
            SetupFieldId::TelegramPollSeconds => {
                self.chatbot_telegram_poll_interval_seconds.to_string()
            }
            SetupFieldId::TelegramWhitelistUserIds => {
                self.chatbot_telegram_whitelist_user_ids.clone()
            }
            SetupFieldId::FeishuChatId => self.chatbot_feishu_chat_id.clone(),
        }
    }

    pub fn editable_value(&self, field: SetupFieldId) -> String {
        match field {
            SetupFieldId::AiProviderEndpoint => self.ai_provider_endpoint.clone(),
            SetupFieldId::AiProviderApikey => self.ai_provider_apikey.clone(),
            SetupFieldId::AiProviderModel => self.ai_provider_model.clone(),
            SetupFieldId::AiProviderDebug => String::new(),
            SetupFieldId::ChatbotProvider => self.chatbot_provider.clone(),
            SetupFieldId::TelegramPollSeconds => {
                self.chatbot_telegram_poll_interval_seconds.to_string()
            }
            SetupFieldId::TelegramWhitelistUserIds => {
                self.chatbot_telegram_whitelist_user_ids.clone()
            }
            SetupFieldId::FeishuChatId => self.chatbot_feishu_chat_id.clone(),
            SetupFieldId::TelegramEnabled | SetupFieldId::FeishuEnabled => String::new(),
        }
    }

    pub fn set_field(&mut self, field: SetupFieldId, value: &str) {
        match field {
            SetupFieldId::AiProviderEndpoint => self.ai_provider_endpoint = value.to_string(),
            SetupFieldId::AiProviderApikey => self.ai_provider_apikey = value.to_string(),
            SetupFieldId::AiProviderModel => self.ai_provider_model = value.to_string(),
            SetupFieldId::AiProviderDebug => {}
            SetupFieldId::ChatbotProvider => self.chatbot_provider = value.to_string(),
            SetupFieldId::TelegramPollSeconds => {
                if let Ok(seconds) = value.trim().parse::<u64>() {
                    self.chatbot_telegram_poll_interval_seconds = seconds.max(1);
                }
            }
            SetupFieldId::TelegramWhitelistUserIds => {
                self.chatbot_telegram_whitelist_user_ids = value.to_string()
            }
            SetupFieldId::FeishuChatId => self.chatbot_feishu_chat_id = value.to_string(),
            SetupFieldId::TelegramEnabled | SetupFieldId::FeishuEnabled => {}
        }
    }

    pub fn toggle_field(&mut self, field: SetupFieldId) {
        match field {
            SetupFieldId::AiProviderDebug => self.ai_provider_debug = !self.ai_provider_debug,
            SetupFieldId::TelegramEnabled => {
                self.chatbot_telegram_enabled = !self.chatbot_telegram_enabled
            }
            SetupFieldId::FeishuEnabled => {
                self.chatbot_feishu_enabled = !self.chatbot_feishu_enabled
            }
            _ => {}
        }
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

    pub fn provider_apikey(&self, selected_provider: usize) -> &str {
        match CHATBOT_PROVIDERS
            .get(selected_provider)
            .copied()
            .unwrap_or("telegram")
        {
            "telegram" => self.chatbot_telegram_apikey.as_str(),
            "feishu" => self.chatbot_feishu_apikey.as_str(),
            _ => "",
        }
    }

    pub fn set_provider_apikey(&mut self, selected_provider: usize, apikey: &str) {
        match CHATBOT_PROVIDERS
            .get(selected_provider)
            .copied()
            .unwrap_or("telegram")
        {
            "telegram" => self.chatbot_telegram_apikey = apikey.to_string(),
            "feishu" => self.chatbot_feishu_apikey = apikey.to_string(),
            _ => {}
        }
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
}

pub enum FrontendResponse {
    ChatReply { reply: String },
    SetupLoaded { config: SetupConfig },
    ServerRestarted { status: RestartStatus },
    SetupSaved { result: SaveSetupResult },
}

pub struct SaveSetupResult {
    pub config_path: PathBuf,
    pub restart_status: RestartStatus,
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
                save_setup_config(&config)?;
                let restart_status = match botty_boss::restart_all_report() {
                    Ok(lines) => RestartStatus::Success(lines.join("\n")),
                    Err(err) => RestartStatus::Failed(format!("Auto restart failed: {err}")),
                };

                Ok(FrontendResponse::SetupSaved {
                    result: SaveSetupResult {
                        config_path: path,
                        restart_status,
                    },
                })
            }
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
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(SetupConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = SetupConfig::default();
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
            "ai.provider.endpoint" => config.ai_provider_endpoint = value.to_string(),
            "ai.provider.apikey" => config.ai_provider_apikey = value.to_string(),
            "ai.provider.model" => config.ai_provider_model = value.to_string(),
            "ai.provider.debug" => config.ai_provider_debug = parse_bool(value),
            "provider.endpoint" => config.ai_provider_endpoint = value.to_string(),
            "provider.apikey" => config.ai_provider_apikey = value.to_string(),
            "provider.model" => config.ai_provider_model = value.to_string(),
            "provider.debug" => config.ai_provider_debug = parse_bool(value),
            "chatbot.provider" => apply_chatbot_provider_list(&mut config, value),
            "chatbot.telegram.api_base" => config.chatbot_telegram_api_base = value.to_string(),
            "chatbot.telegram.apikey" => config.chatbot_telegram_apikey = value.to_string(),
            "chatbot.feishu.api_base" => config.chatbot_feishu_api_base = value.to_string(),
            "chatbot.feishu.apikey" => config.chatbot_feishu_apikey = value.to_string(),
            "chatbot.apikey" => {
                if config.chatbot_provider == "feishu" {
                    config.chatbot_feishu_apikey = value.to_string();
                } else {
                    config.chatbot_telegram_apikey = value.to_string();
                }
            }
            "chatbot.telegram.enabled" => config.chatbot_telegram_enabled = parse_bool(value),
            "chatbot.feishu.enabled" => config.chatbot_feishu_enabled = parse_bool(value),
            "chatbot.telegram.whitelist_user_ids" => {
                config.chatbot_telegram_whitelist_user_ids = value.to_string()
            }
            "chatbot.feishu.chat_id" => config.chatbot_feishu_chat_id = value.to_string(),
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
            _ => {}
        }
    }

    Ok(config)
}

fn save_setup_config(config: &SetupConfig) -> io::Result<()> {
    let path = setup_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = format!(
        "ai.provider.endpoint={}\nai.provider.apikey={}\nai.provider.model={}\nai.provider.debug={}\nchatbot.provider={}\nchatbot.telegram.api_base={}\nchatbot.telegram.apikey={}\nchatbot.feishu.api_base={}\nchatbot.feishu.apikey={}\nchatbot.telegram.enabled={}\nchatbot.feishu.enabled={}\nchatbot.telegram.whitelist_user_ids={}\nchatbot.telegram.poll_interval_seconds={}\nchatbot.feishu.poll_interval_seconds={}\nchatbot.feishu.chat_id={}\n",
        config.ai_provider_endpoint,
        config.ai_provider_apikey,
        config.ai_provider_model,
        config.ai_provider_debug,
        enabled_provider_list(config),
        config.chatbot_telegram_api_base,
        config.chatbot_telegram_apikey,
        config.chatbot_feishu_api_base,
        config.chatbot_feishu_apikey,
        config.chatbot_telegram_enabled,
        config.chatbot_feishu_enabled,
        config.chatbot_telegram_whitelist_user_ids,
        config.chatbot_telegram_poll_interval_seconds,
        config.chatbot_feishu_poll_interval_seconds,
        config.chatbot_feishu_chat_id
    );

    fs::write(path, content)
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn apply_chatbot_provider_list(config: &mut SetupConfig, value: &str) {
    config.chatbot_telegram_enabled = false;
    config.chatbot_feishu_enabled = false;

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
    list.join(",")
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}
