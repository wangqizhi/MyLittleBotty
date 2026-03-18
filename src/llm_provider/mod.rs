#[path = "provider-anthropic.rs"]
pub mod provider_anthropic;
#[path = "provider-minimax.rs"]
pub mod provider_minimax;
#[path = "provider-openai.rs"]
pub mod provider_openai;
#[path = "provider-glm.rs"]
pub mod provider_glm;

use serde::{Deserialize, Serialize};
use std::io;

use crate::botty_brain::BrainConfig;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema_json: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderToolUse {
    pub id: String,
    pub name: String,
    pub input_json: String,
    pub assistant_content_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderTextResponse {
    pub text: String,
    pub thinking: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProviderMessage {
    UserText(String),
    UserToolResult {
        tool_use_id: String,
        content: String,
    },
    AssistantToolUse {
        assistant_content_json: String,
    },
}

pub enum ProviderResponse {
    Text(ProviderTextResponse),
    ToolUse(ProviderToolUse),
}

pub struct ProviderRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub payload: String,
}

pub trait LlmProvider {
    fn build_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest>;
    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse>;
}

pub enum ProviderKind {
    Glm,
    Anthropic,
    Minimax,
    OpenAi,
}

pub fn detect_provider(config: &BrainConfig) -> ProviderKind {
    let endpoint = config
        .endpoint
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase();

    if endpoint.ends_with("/v1/messages")
        || endpoint.ends_with("/anthropic")
        || endpoint.contains("/anthropic/")
    {
        if endpoint.contains("bigmodel.cn") {
            return ProviderKind::Glm;
        }
        return ProviderKind::Anthropic;
    }

    if endpoint.contains("minimax") {
        return ProviderKind::Minimax;
    }

    ProviderKind::OpenAi
}
