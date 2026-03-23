use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::llm_provider::provider_anthropic::AnthropicProvider;
use crate::llm_provider::provider_glm::GlmProvider;
use crate::llm_provider::provider_minimax::MinimaxProvider;
use crate::llm_provider::provider_openai::OpenAiProvider;
use crate::llm_provider::{
    detect_provider, LlmProvider, ProviderKind, ProviderMessage, ProviderResponse,
    ProviderToolDefinition,
};

const AI_PROVIDER_REQUEST_MAX_RETRIES: usize = 3;
const AI_PROVIDER_RETRY_ON_HTTP_400: bool = true;

#[derive(Clone)]
pub struct BrainConfig {
    pub profile_name: String,
    pub endpoint: String,
    pub apikey: String,
    pub model: String,
    pub debug_enabled: bool,
    pub vision_enabled: bool,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            profile_name: String::new(),
            endpoint: String::new(),
            apikey: String::new(),
            model: String::new(),
            debug_enabled: false,
            vision_enabled: false,
        }
    }
}

impl BrainConfig {
    pub fn model_name(&self) -> &str {
        self.model.trim()
    }
}

pub struct BottyBrain {
    config: BrainConfig,
}

impl BottyBrain {
    pub fn from_setup() -> io::Result<Self> {
        Ok(Self {
            config: load_active_brain_config()?,
        })
    }

    pub fn from_image_setup() -> io::Result<Self> {
        Ok(Self {
            config: load_image_brain_config()?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "暂不支持图像识别，请配置支持图像的 provider。",
                )
            })?,
        })
    }

    pub fn think(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderResponse> {
        if self.config.endpoint.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AI provider endpoint is not configured. Please update your setup.",
            ));
        }
        if self.config.apikey.is_empty() && endpoint_requires_apikey(self.config.endpoint.as_str())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AI provider API key is not configured. Please update your setup.",
            ));
        }

        let provider: Box<dyn LlmProvider> = match detect_provider(&self.config) {
            ProviderKind::Glm => Box::new(GlmProvider::from_config(&self.config)),
            ProviderKind::Anthropic => Box::new(AnthropicProvider::from_config(&self.config)),
            ProviderKind::Minimax => Box::new(MinimaxProvider::from_config(&self.config)),
            ProviderKind::OpenAi => Box::new(OpenAiProvider::from_config(&self.config)),
        };
        let request = provider.build_request(system_prompt, messages, tools)?;
        self.log_debug("request-url", &request.url)?;
        self.log_debug("request", &request.payload)?;

        let response_body = self.execute_provider_request(&request)?;

        provider.parse_response(&response_body)
    }

    fn execute_provider_request(
        &self,
        request: &crate::llm_provider::ProviderRequest,
    ) -> io::Result<String> {
        let mut last_error: Option<io::Error> = None;

        for attempt in 0..=AI_PROVIDER_REQUEST_MAX_RETRIES {
            let header_path = temp_provider_header_path();
            let mut command = Command::new("curl");
            command
                .arg("--fail-with-body")
                .arg("-sS")
                .arg("-X")
                .arg("POST")
                .arg(&request.url)
                .arg("-H")
                .arg("Content-Type: application/json")
                .arg("-D")
                .arg(&header_path);

            for (name, value) in &request.headers {
                command.arg("-H").arg(format!("{name}: {value}"));
            }

            match command.arg("-d").arg(&request.payload).output() {
                Ok(output) => {
                    let response_headers = read_provider_header_file(&header_path);
                    let _ = fs::remove_file(&header_path);
                    let response_body = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let response_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let trace_id = extract_trace_id_from_headers(response_headers.as_deref())
                        .map(str::to_string)
                        .or_else(|| extract_trace_id_from_body(&response_body))
                        .unwrap_or_else(|| "None".to_string());
                    self.log_debug("response-trace_id", &format!("trace_id={trace_id}"))?;

                    if !response_body.is_empty() {
                        self.log_debug("response", &response_body)?;
                    }
                    if !response_error.is_empty() {
                        self.log_debug("response-stderr", &response_error)?;
                    }

                    if output.status.success() {
                        return Ok(response_body);
                    }

                    let retryable = should_retry_provider_error(
                        response_error.as_str(),
                        response_body.as_str(),
                    );
                    last_error = Some(io::Error::other(classify_provider_error(
                        response_error.as_str(),
                        response_body.as_str(),
                    )));

                    if !retryable {
                        break;
                    }
                }
                Err(err) => {
                    let _ = fs::remove_file(&header_path);
                    let retryable = should_retry_provider_error(err.to_string().as_str(), "");
                    last_error = Some(io::Error::new(
                        err.kind(),
                        format!("failed to execute curl for AI provider request: {err}"),
                    ));

                    if !retryable {
                        break;
                    }
                }
            }

            if attempt < AI_PROVIDER_REQUEST_MAX_RETRIES {
                let attempt_num = attempt + 1;
                self.log_debug(
                    "retry",
                    &format!(
                        "ai provider request attempt {} failed, retrying (max retries={})",
                        attempt_num, AI_PROVIDER_REQUEST_MAX_RETRIES
                    ),
                )?;
                thread::sleep(Duration::from_millis(400 * attempt_num as u64));
            }
        }

        Err(last_error.unwrap_or_else(|| io::Error::other("AI provider request failed")))
    }

    fn log_debug(&self, direction: &str, content: &str) -> io::Result<()> {
        log_debug_line_if_enabled(direction, content, None)
    }
}

pub fn active_and_image_providers_differ() -> io::Result<bool> {
    let selection = load_brain_profile_selection()?;
    let Some(image) = selection.image else {
        return Ok(false);
    };
    Ok(selection.active.profile_name != image.profile_name)
}

pub fn active_supports_vision() -> io::Result<bool> {
    Ok(load_brain_profile_selection()?.active.vision_enabled)
}

pub fn is_llm_connection_error(err: &io::Error) -> bool {
    is_llm_connection_error_message(&err.to_string())
}

pub fn log_debug_line_if_enabled(
    direction: &str,
    content: &str,
    user_id_override: Option<&str>,
) -> io::Result<()> {
    if !load_active_brain_config()?.debug_enabled {
        return Ok(());
    }

    append_debug_log_line(direction, content, user_id_override)
}

struct BrainProfileSelection {
    active: BrainConfig,
    image: Option<BrainConfig>,
}

fn load_active_brain_config() -> io::Result<BrainConfig> {
    Ok(load_brain_profile_selection()?.active)
}

fn load_image_brain_config() -> io::Result<Option<BrainConfig>> {
    Ok(load_brain_profile_selection()?.image)
}

fn load_brain_profile_selection() -> io::Result<BrainProfileSelection> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(BrainProfileSelection {
                active: BrainConfig::default(),
                image: None,
            });
        }
        Err(err) => return Err(err),
    };

    let mut legacy = BrainConfig::default();
    let mut active_profile = String::new();
    let mut profiles: Vec<(String, BrainConfig)> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ai.provider.active" => active_profile = value.trim().to_string(),
            "ai.provider.endpoint" | "provider.endpoint" => {
                legacy.endpoint = value.trim().to_string()
            }
            "ai.provider.apikey" | "provider.apikey" => legacy.apikey = value.trim().to_string(),
            "ai.provider.model" | "provider.model" => legacy.model = value.trim().to_string(),
            "ai.provider.debug" | "provider.debug" => legacy.debug_enabled = parse_bool(value),
            "ai.provider.vision" | "provider.vision" => legacy.vision_enabled = parse_bool(value),
            other => {
                if let Some((profile_name, field_name)) = parse_ai_profile_key(other) {
                    let profile = ensure_brain_profile_slot(&mut profiles, profile_name);
                    if profile.profile_name.is_empty() {
                        profile.profile_name = profile_name.to_string();
                    }
                    match field_name {
                        "endpoint" => profile.endpoint = value.trim().to_string(),
                        "apikey" => profile.apikey = value.trim().to_string(),
                        "model" => profile.model = value.trim().to_string(),
                        "debug" => profile.debug_enabled = parse_bool(value),
                        "vision" => profile.vision_enabled = parse_bool(value),
                        _ => {}
                    }
                }
            }
        }
    }

    if profiles.is_empty() {
        return Ok(BrainProfileSelection {
            active: legacy.clone(),
            image: if legacy.vision_enabled {
                Some(legacy)
            } else {
                None
            },
        });
    }

    let active = if active_profile.trim().is_empty() {
        profiles
            .first()
            .map(|(_, profile)| profile.clone())
            .unwrap_or_default()
    } else {
        profiles
            .iter()
            .find(|(name, _)| name == &active_profile)
            .map(|(_, profile)| profile.clone())
            .or_else(|| profiles.first().map(|(_, profile)| profile.clone()))
            .unwrap_or_default()
    };
    let image = if active.vision_enabled {
        Some(active.clone())
    } else {
        profiles
            .iter()
            .find(|(_, profile)| profile.vision_enabled)
            .map(|(_, profile)| profile.clone())
    };

    Ok(BrainProfileSelection { active, image })
}

fn parse_ai_profile_key(key: &str) -> Option<(&str, &str)> {
    let key = key.strip_prefix("ai.provider.")?;
    let (profile_name, field_name) = key.split_once('.')?;
    if profile_name.is_empty() {
        return None;
    }
    Some((profile_name, field_name))
}

fn ensure_brain_profile_slot<'a>(
    profiles: &'a mut Vec<(String, BrainConfig)>,
    profile_name: &str,
) -> &'a mut BrainConfig {
    if let Some(index) = profiles.iter().position(|(name, _)| name == profile_name) {
        return &mut profiles[index].1;
    }
    profiles.push((profile_name.to_string(), BrainConfig::default()));
    let index = profiles.len().saturating_sub(1);
    &mut profiles[index].1
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn endpoint_requires_apikey(endpoint: &str) -> bool {
    let trimmed = endpoint.trim().to_ascii_lowercase();
    if trimmed.starts_with("http://localhost")
        || trimmed.starts_with("http://127.0.0.1")
        || trimmed.starts_with("http://[::1]")
    {
        return false;
    }
    if trimmed.starts_with("http://10.")
        || trimmed.starts_with("http://192.168.")
        || trimmed.starts_with("http://172.16.")
        || trimmed.starts_with("http://172.17.")
        || trimmed.starts_with("http://172.18.")
        || trimmed.starts_with("http://172.19.")
        || trimmed.starts_with("http://172.20.")
        || trimmed.starts_with("http://172.21.")
        || trimmed.starts_with("http://172.22.")
        || trimmed.starts_with("http://172.23.")
        || trimmed.starts_with("http://172.24.")
        || trimmed.starts_with("http://172.25.")
        || trimmed.starts_with("http://172.26.")
        || trimmed.starts_with("http://172.27.")
        || trimmed.starts_with("http://172.28.")
        || trimmed.starts_with("http://172.29.")
        || trimmed.starts_with("http://172.30.")
        || trimmed.starts_with("http://172.31.")
    {
        return false;
    }
    true
}

fn append_debug_log_line(
    direction: &str,
    content: &str,
    user_id_override: Option<&str>,
) -> io::Result<()> {
    let path = debug_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let timestamp = local_time_format("%Y-%m-%d %H:%M:%S")?;
    let role = env::var("BOTTY_GUY_ROLE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "leader".to_string());
    let user_id = user_id_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            env::var("BOTTY_CURRENT_JOB_USER_ID")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
    let sanitized = content.replace('\n', "\\n").replace('\r', "\\r");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let user_prefix = user_id
        .as_deref()
        .map(|value| format!(" user_id={value}"))
        .unwrap_or_default();
    writeln!(
        file,
        "[{timestamp}] role={role}{user_prefix} {direction}: {sanitized}"
    )?;
    Ok(())
}

fn debug_log_path() -> PathBuf {
    botty_root_dir()
        .join("log")
        .join(format!("brain-debug{}.log", runtime_suffix()))
}

fn temp_provider_header_path() -> PathBuf {
    env::temp_dir().join(format!(
        "mylittlebotty-provider-headers-{}.tmp",
        process::id()
    ))
}

fn read_provider_header_file(path: &PathBuf) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn extract_trace_id_from_headers(headers: Option<&str>) -> Option<&str> {
    let mut trace_id = None;
    for line in headers.unwrap_or_default().lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("trace_id") {
            let candidate = value.trim();
            if !candidate.is_empty() {
                trace_id = Some(candidate);
            }
        }
    }
    trace_id
}

fn extract_trace_id_from_body(body: &str) -> Option<String> {
    let payload: Value = serde_json::from_str(body).ok()?;
    payload
        .get("trace_id")?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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

fn local_time_format(format: &str) -> io::Result<String> {
    let output = Command::new("date").arg(format!("+{format}")).output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn classify_provider_error(detail: &str, response_body: &str) -> String {
    let trimmed = detail.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(provider_message) = extract_provider_error_message(response_body) {
        return format!(
            "AI provider request failed. Details: {}",
            provider_message.trim()
        );
    }

    if lower.contains(" 401") || lower.contains("error: 401") || lower.contains("unauthorized") {
        return "AI provider request was rejected with 401 Unauthorized. Please check your API key."
            .to_string();
    }
    if lower.contains(" 403") || lower.contains("error: 403") || lower.contains("forbidden") {
        return "AI provider request was rejected with 403 Forbidden. Please check your API key and provider permissions."
            .to_string();
    }
    if lower.contains(" 404") || lower.contains("error: 404") {
        return "AI provider endpoint returned 404 Not Found. Please check the endpoint URL."
            .to_string();
    }
    if lower.contains("could not resolve host")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname provided")
    {
        return "AI provider endpoint could not be resolved. Please check the endpoint URL and your network."
            .to_string();
    }
    if lower.contains("failed to connect")
        || lower.contains("connection refused")
        || lower.contains("couldn't connect")
    {
        return "Could not connect to the AI provider endpoint. Please check the endpoint URL and network access."
            .to_string();
    }
    if lower.contains("operation timed out") || lower.contains("timed out") {
        return "The AI provider request timed out. Please try again or check the endpoint availability."
            .to_string();
    }
    if lower.contains("ssl") || lower.contains("certificate") {
        return "The AI provider connection failed during TLS/SSL negotiation. Please check the endpoint configuration."
            .to_string();
    }
    if trimmed.is_empty() {
        return "AI provider request failed. Please check your endpoint and API key configuration."
            .to_string();
    }

    format!("AI provider request failed. Please check your configuration. Details: {trimmed}")
}

fn should_retry_provider_error(detail: &str, response_body: &str) -> bool {
    let lower = detail.trim().to_ascii_lowercase();
    let body_lower = response_body.trim().to_ascii_lowercase();

    if body_lower.contains("(2013)") || body_lower.contains("invalid_request_error") {
        return false;
    }

    if lower.contains(" 400") || lower.contains("error: 400") {
        return AI_PROVIDER_RETRY_ON_HTTP_400;
    }
    if lower.contains(" 408")
        || lower.contains("error: 408")
        || lower.contains(" 409")
        || lower.contains("error: 409")
        || lower.contains(" 425")
        || lower.contains("error: 425")
        || lower.contains(" 429")
        || lower.contains("error: 429")
        || lower.contains("could not resolve host")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname provided")
        || lower.contains("failed to connect")
        || lower.contains("connection refused")
        || lower.contains("couldn't connect")
        || lower.contains("operation timed out")
        || lower.contains("timed out")
        || lower.contains("ssl")
        || lower.contains("certificate")
        || lower.contains("http2 framing layer")
        || lower.contains("ssl_connect")
    {
        return true;
    }

    false
}

fn extract_provider_error_message(response_body: &str) -> Option<String> {
    let payload: Value = serde_json::from_str(response_body).ok()?;
    let error = payload.get("error")?;
    let error_type = error.get("type").and_then(Value::as_str).unwrap_or("error");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let request_id = payload
        .get("request_id")
        .and_then(Value::as_str)
        .or_else(|| payload.get("trace_id").and_then(Value::as_str));

    if message.is_empty() {
        return request_id.map(|id| format!("{error_type} (request_id={id})"));
    }

    match request_id {
        Some(id) if !id.trim().is_empty() => Some(format!(
            "{error_type}: {message} (request_id={})",
            id.trim()
        )),
        _ => Some(format!("{error_type}: {message}")),
    }
}

fn is_llm_connection_error_message(detail: &str) -> bool {
    let lower = detail.trim().to_ascii_lowercase();
    lower.contains("could not connect to the ai provider endpoint")
        || lower.contains("ai provider endpoint could not be resolved")
        || lower.contains("ai provider request timed out")
        || lower.contains("tls/ssl")
        || lower.contains("failed to connect")
        || lower.contains("couldn't connect")
        || lower.contains("could not resolve host")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname provided")
        || lower.contains("timed out")
        || lower.contains("ssl")
        || lower.contains("certificate")
        || lower.contains("failed to execute curl for ai provider request")
}
