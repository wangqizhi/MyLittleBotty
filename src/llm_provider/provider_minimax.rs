use crate::botty_brain::BrainConfig;
use crate::llm_provider::{LlmProvider, ProviderRequest};
use std::io;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MODEL: &str = "MiniMax-M2.1";
const DEFAULT_OPENAI_MODEL: &str = "MiniMax-M2.1";

pub struct MinimaxProvider {
    endpoint: String,
    apikey: String,
    model: String,
}

impl MinimaxProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        Self {
            endpoint: config.endpoint.trim().to_string(),
            apikey: config.apikey.trim().to_string(),
            model: config.model_name().to_string(),
        }
    }
}

impl LlmProvider for MinimaxProvider {
    fn build_request(&self, input: &str) -> io::Result<ProviderRequest> {
        let target = RequestTarget::from_endpoint(&self.endpoint);
        Ok(ProviderRequest {
            url: target.url().to_string(),
            headers: target.auth_headers(&self.apikey),
            payload: target.build_payload(&self.model, input),
        })
    }
}

enum RequestTarget {
    Anthropic { url: String },
    OpenAi { url: String },
}

impl RequestTarget {
    fn from_endpoint(endpoint: &str) -> Self {
        let trimmed = endpoint.trim().trim_end_matches('/');
        if trimmed.ends_with("/v1/messages") {
            return Self::Anthropic {
                url: trimmed.to_string(),
            };
        }
        if trimmed.ends_with("/chat/completions") {
            return Self::OpenAi {
                url: trimmed.to_string(),
            };
        }
        if trimmed.ends_with("/anthropic") || trimmed.contains("/anthropic/") {
            return Self::Anthropic {
                url: format!("{trimmed}/v1/messages"),
            };
        }
        if trimmed.ends_with("/v1") {
            return Self::OpenAi {
                url: format!("{trimmed}/chat/completions"),
            };
        }
        Self::OpenAi {
            url: trimmed.to_string(),
        }
    }

    fn url(&self) -> &str {
        match self {
            Self::Anthropic { url } | Self::OpenAi { url } => url.as_str(),
        }
    }

    fn build_payload(&self, model: &str, input: &str) -> String {
        let escaped = escape_json_string(input);
        match self {
            Self::Anthropic { .. } => format!(
                "{{\"model\":\"{}\",\"max_tokens\":1024,\"messages\":[{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}]}}",
                default_anthropic_model(model),
                escaped
            ),
            Self::OpenAi { .. } => format!(
                "{{\"model\":\"{}\",\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}]}}",
                default_openai_model(model),
                escaped
            ),
        }
    }

    fn auth_headers(&self, apikey: &str) -> Vec<(String, String)> {
        if apikey.is_empty() {
            return Vec::new();
        }

        match self {
            Self::Anthropic { .. } => vec![
                ("x-api-key".to_string(), apikey.to_string()),
                (
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                ),
            ],
            Self::OpenAi { .. } => vec![(
                "Authorization".to_string(),
                format!("Bearer {apikey}"),
            )],
        }
    }
}

fn default_anthropic_model(model: &str) -> &str {
    if model.is_empty() {
        DEFAULT_ANTHROPIC_MODEL
    } else {
        model
    }
}

fn default_openai_model(model: &str) -> &str {
    if model.is_empty() {
        DEFAULT_OPENAI_MODEL
    } else {
        model
    }
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
