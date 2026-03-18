use crate::botty_brain::BrainConfig;
use crate::llm_provider::{
    LlmProvider, ProviderMessage, ProviderRequest, ProviderResponse, ProviderTextResponse,
    ProviderToolDefinition, ProviderToolUse,
};
use serde_json::{json, Value};
use std::io;

const DEFAULT_GLM_MODEL: &str = "glm-4.7";
const GLM_MAX_TOKENS_DEFAULT: u64 = 96_000;
const GLM_MAX_TOKENS_5: u64 = 128_000;

pub struct GlmProvider {
    endpoint: String,
    apikey: String,
    model: String,
}

impl GlmProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        Self {
            endpoint: normalize_endpoint(config.endpoint.as_str()),
            apikey: config.apikey.trim().to_string(),
            model: config.model_name().to_string(),
        }
    }
}

impl LlmProvider for GlmProvider {
    fn build_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest> {
        let serialized_messages = build_messages(messages)?;
        let model = normalized_glm_model(self.model.as_str());
        let mut payload = json!({
            "model": model,
            "system": system_prompt,
            "max_tokens": glm_max_tokens(model.as_str()),
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
            payload: serde_json::to_string(&payload)
                .map_err(|err| io::Error::other(format!("serialize glm payload failed: {err}")))?,
        })
    }

    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse> {
        let response: Value = serde_json::from_str(response_body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse glm response failed: {err}"),
            )
        })?;
        if let Some(error_message) = extract_glm_error(&response) {
            return Err(io::Error::other(error_message));
        }
        let content = response
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "glm response missing content array")
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
                "glm response missing text or tool_use",
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
                "content": text,
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
    vec![("x-api-key".to_string(), apikey.to_string())]
}

fn normalized_glm_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return DEFAULT_GLM_MODEL.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("glm") {
        lower
    } else {
        trimmed.to_string()
    }
}

fn glm_max_tokens(model: &str) -> u64 {
    let model = model.to_ascii_lowercase();
    if model.contains("glm-5") {
        GLM_MAX_TOKENS_5
    } else {
        GLM_MAX_TOKENS_DEFAULT
    }
}

fn extract_glm_error(response: &Value) -> Option<String> {
    let error = response.get("error")?;
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        let trimmed = message.trim();
        if !trimmed.is_empty() {
            return Some(format!("glm api error: {trimmed}"));
        }
    }
    let serialized = serde_json::to_string(error).ok()?;
    Some(format!("glm api error: {serialized}"))
}
