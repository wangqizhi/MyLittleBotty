use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use crate::llm_provider::provider_anthropic::AnthropicProvider;
use crate::llm_provider::provider_minimax::MinimaxProvider;
use crate::llm_provider::provider_openai::OpenAiProvider;
use crate::llm_provider::{
    detect_provider, LlmProvider, ProviderKind, ProviderMessage, ProviderResponse,
    ProviderToolDefinition,
};

pub struct BrainConfig {
    pub endpoint: String,
    pub apikey: String,
    pub model: String,
    pub debug_enabled: bool,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            apikey: String::new(),
            model: String::new(),
            debug_enabled: false,
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
            config: load_brain_config()?,
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
        if self.config.apikey.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AI provider API key is not configured. Please update your setup.",
            ));
        }

        let provider: Box<dyn LlmProvider> = match detect_provider(&self.config) {
            ProviderKind::Anthropic => Box::new(AnthropicProvider::from_config(&self.config)),
            ProviderKind::Minimax => Box::new(MinimaxProvider::from_config(&self.config)),
            ProviderKind::OpenAi => Box::new(OpenAiProvider::from_config(&self.config)),
        };
        let request = provider.build_request(system_prompt, messages, tools)?;
        self.log_debug("request-url", &request.url)?;
        self.log_debug("request", &request.payload)?;

        let mut command = Command::new("curl");
        command
            .arg("-fsS")
            .arg("-X")
            .arg("POST")
            .arg(&request.url)
            .arg("-H")
            .arg("Content-Type: application/json");

        for (name, value) in &request.headers {
            command.arg("-H").arg(format!("{name}: {value}"));
        }

        let output = command.arg("-d").arg(&request.payload).output()?;
        let response_body = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let response_error = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !response_body.is_empty() {
            self.log_debug("response", &response_body)?;
        }
        if !response_error.is_empty() {
            self.log_debug("response-stderr", &response_error)?;
        }

        if !output.status.success() {
            return Err(io::Error::other(classify_provider_error(
                response_error.as_str(),
            )));
        }

        provider.parse_response(&response_body)
    }

    fn log_debug(&self, direction: &str, content: &str) -> io::Result<()> {
        if !self.config.debug_enabled {
            return Ok(());
        }

        let path = debug_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let timestamp = local_time_format("%Y-%m-%d %H:%M:%S")?;
        let sanitized = content.replace('\n', "\\n").replace('\r', "\\r");
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "[{timestamp}] {direction}: {sanitized}")?;
        Ok(())
    }
}

fn load_brain_config() -> io::Result<BrainConfig> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BrainConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = BrainConfig::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ai.provider.endpoint" | "provider.endpoint" => {
                config.endpoint = value.trim().to_string()
            }
            "ai.provider.apikey" | "provider.apikey" => config.apikey = value.trim().to_string(),
            "ai.provider.model" | "provider.model" => config.model = value.trim().to_string(),
            "ai.provider.debug" | "provider.debug" => config.debug_enabled = parse_bool(value),
            _ => {}
        }
    }

    Ok(config)
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn debug_log_path() -> PathBuf {
    botty_root_dir()
        .join("log")
        .join(format!("brain-debug{}.log", runtime_suffix()))
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

fn classify_provider_error(detail: &str) -> String {
    let trimmed = detail.trim();
    let lower = trimmed.to_ascii_lowercase();

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
