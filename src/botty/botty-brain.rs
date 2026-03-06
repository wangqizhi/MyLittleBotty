use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use crate::llm_provider::provider_minimax::MinimaxProvider;
use crate::llm_provider::LlmProvider;

const LLM_PLACEHOLDER_REPLY: &str = "大模型已经回应";

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

    pub fn think(&self, input: &str) -> io::Result<String> {
        if self.config.endpoint.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ai.provider.endpoint is empty",
            ));
        }

        let provider = MinimaxProvider::from_config(&self.config);
        let request = provider.build_request(input)?;
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
            return Err(io::Error::other(format!(
                "llm request failed: {}",
                response_error
            )));
        }

        Ok(LLM_PLACEHOLDER_REPLY.to_string())
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
