use crate::botty_brain::BrainConfig;
use crate::llm_provider::{
    LlmProvider, ProviderMessage, ProviderRequest, ProviderResponse, ProviderTextResponse,
    ProviderToolDefinition, ProviderToolUse,
};
use serde_json::{json, Value};
use std::io;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MODEL: &str = "MiniMax-M2.1";
const MINIMAX_REQUEST_MAX_TOKENS: u64 = 64_000;
const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 1024;

pub struct AnthropicProvider {
    endpoint: String,
    apikey: String,
    model: String,
}

impl AnthropicProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        Self {
            endpoint: normalize_endpoint(config.endpoint.as_str()),
            apikey: config.apikey.trim().to_string(),
            model: config.model_name().to_string(),
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn build_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest> {
        let serialized_messages = build_messages(messages)?;
        let mut payload = json!({
            "model": default_anthropic_model(self.model.as_str()),
            "system": system_prompt,
            "max_tokens": anthropic_max_tokens(self.model.as_str()),
            "messages": serialized_messages,
        });

        if !tools.is_empty() {
            let tool_values = build_tools(tools)?;
            payload["tools"] = Value::Array(tool_values);
            payload["tool_choice"] = json!({ "type": "auto" });
        }

        Ok(ProviderRequest {
            url: self.endpoint.clone(),
            headers: auth_headers(self.apikey.as_str()),
            payload: serde_json::to_string(&payload).map_err(|err| {
                io::Error::other(format!("serialize anthropic payload failed: {err}"))
            })?,
        })
    }

    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse> {
        let response: Value = serde_json::from_str(response_body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse anthropic response failed: {err}"),
            )
        })?;
        if let Some(error_message) = extract_anthropic_error(&response) {
            return Err(io::Error::other(error_message));
        }
        let content = response
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "anthropic response missing content array",
                )
            })?;

        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let id = block.get("id").and_then(Value::as_str).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "tool_use missing id")
                })?;
                let name = block.get("name").and_then(Value::as_str).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "tool_use missing name")
                })?;
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                return Ok(ProviderResponse::ToolUse(ProviderToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input_json: serde_json::to_string(&input).map_err(|err| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("serialize tool input failed: {err}"),
                        )
                    })?,
                    assistant_content_json: serde_json::to_string(content).map_err(|err| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("serialize assistant content failed: {err}"),
                        )
                    })?,
                }));
            }
        }

        let mut texts = Vec::new();
        let mut thinkings = Vec::new();
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
                Some("thinking") => {
                    if let Some(thinking) = block.get("thinking").and_then(Value::as_str) {
                        let trimmed = thinking.trim();
                        if !trimmed.is_empty() {
                            thinkings.push(trimmed.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        if texts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "anthropic response missing text or tool_use",
            ));
        }

        Ok(ProviderResponse::Text(ProviderTextResponse {
            text: texts.join("\n"),
            thinking: if thinkings.is_empty() {
                None
            } else {
                Some(thinkings.join("\n"))
            },
        }))
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1/messages") {
        return trimmed.to_string();
    }
    if trimmed.ends_with("/anthropic") || trimmed.contains("/anthropic/") {
        return format!("{trimmed}/v1/messages");
    }
    trimmed.to_string()
}

fn build_messages(messages: &[ProviderMessage]) -> io::Result<Vec<Value>> {
    let mut serialized = Vec::new();
    for message in messages {
        match message {
            ProviderMessage::UserText(text) => serialized.push(json!({
                "role": "user",
                "content": [{"type": "text", "text": text}],
            })),
            ProviderMessage::UserToolResult {
                tool_use_id,
                content,
            } => serialized.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                }],
            })),
            ProviderMessage::AssistantToolUse {
                assistant_content_json,
            } => {
                let content: Value =
                    serde_json::from_str(assistant_content_json).map_err(|err| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("parse assistant tool content failed: {err}"),
                        )
                    })?;
                serialized.push(json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
        }
    }
    Ok(serialized)
}

fn build_tools(tools: &[ProviderToolDefinition]) -> io::Result<Vec<Value>> {
    let mut serialized = Vec::new();
    for tool in tools {
        let input_schema: Value = serde_json::from_str(tool.input_schema_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse tool schema failed: {err}"),
            )
        })?;
        serialized.push(json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": input_schema,
        }));
    }
    Ok(serialized)
}

fn auth_headers(apikey: &str) -> Vec<(String, String)> {
    if apikey.is_empty() {
        return Vec::new();
    }

    vec![
        ("x-api-key".to_string(), apikey.to_string()),
        (
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ),
    ]
}

fn anthropic_max_tokens(model: &str) -> u64 {
    if is_minimax_model(model) {
        MINIMAX_REQUEST_MAX_TOKENS
    } else {
        DEFAULT_ANTHROPIC_MAX_TOKENS
    }
}

fn is_minimax_model(model: &str) -> bool {
    let normalized = default_anthropic_model(model).to_ascii_lowercase();
    normalized.contains("minimax")
}

fn extract_anthropic_error(response: &Value) -> Option<String> {
    let error = response.get("error")?;
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        let trimmed = message.trim();
        if !trimmed.is_empty() {
            return Some(format!("anthropic api error: {trimmed}"));
        }
    }
    let serialized = serde_json::to_string(error).ok()?;
    Some(format!("anthropic api error: {serialized}"))
}

fn default_anthropic_model(model: &str) -> &str {
    if model.is_empty() {
        DEFAULT_ANTHROPIC_MODEL
    } else {
        model
    }
}
