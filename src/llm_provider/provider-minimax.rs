use crate::botty_brain::BrainConfig;
use crate::llm_provider::{
    LlmProvider, ProviderContentPart, ProviderMessage, ProviderRequest, ProviderResponse,
    ProviderTextResponse, ProviderToolDefinition, ProviderToolUse,
};
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::io;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M2.1";
const MINIMAX_REQUEST_MAX_TOKENS: u64 = 64_000;

pub struct MinimaxProvider {
    endpoint: String,
    apikey: String,
    model: String,
    protocol: MinimaxProtocol,
}

#[derive(Clone, Copy)]
enum MinimaxProtocol {
    Anthropic,
    OpenAi,
}

impl MinimaxProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        let endpoint = config.endpoint.trim().to_string();
        let protocol = detect_protocol(endpoint.as_str());
        Self {
            endpoint: normalize_endpoint(endpoint.as_str(), protocol),
            apikey: config.apikey.trim().to_string(),
            model: default_minimax_model(config.model_name()).to_string(),
            protocol,
        }
    }
}

impl LlmProvider for MinimaxProvider {
    fn build_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest> {
        match self.protocol {
            MinimaxProtocol::Anthropic => {
                self.build_anthropic_request(system_prompt, messages, tools)
            }
            MinimaxProtocol::OpenAi => self.build_openai_request(system_prompt, messages),
        }
    }

    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse> {
        match self.protocol {
            MinimaxProtocol::Anthropic => parse_anthropic_response(response_body),
            MinimaxProtocol::OpenAi => parse_openai_response(response_body),
        }
    }
}

impl MinimaxProvider {
    fn build_anthropic_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest> {
        let serialized_messages = build_anthropic_messages(messages)?;
        let mut payload = json!({
            "model": self.model,
            "system": system_prompt,
            "max_tokens": MINIMAX_REQUEST_MAX_TOKENS,
            "messages": serialized_messages,
        });

        if !tools.is_empty() {
            payload["tools"] = Value::Array(build_tools(tools)?);
            payload["tool_choice"] = json!({ "type": "auto" });
        }

        Ok(ProviderRequest {
            url: self.endpoint.clone(),
            headers: vec![
                ("x-api-key".to_string(), self.apikey.clone()),
                (
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                ),
            ],
            payload: serde_json::to_string(&payload).map_err(|err| {
                io::Error::other(format!("serialize minimax payload failed: {err}"))
            })?,
        })
    }

    fn build_openai_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
    ) -> io::Result<ProviderRequest> {
        let mut serialized_messages = vec![json!({
            "role": "system",
            "content": system_prompt,
        })];
        for message in messages {
            serialized_messages.push(json!({
                "role": openai_role(message),
                "content": flatten_openai_message_content(message),
            }));
        }

        let payload = json!({
            "model": self.model,
            "messages": serialized_messages,
            "max_tokens": MINIMAX_REQUEST_MAX_TOKENS,
        });

        Ok(ProviderRequest {
            url: self.endpoint.clone(),
            headers: vec![(
                "Authorization".to_string(),
                format!("Bearer {}", self.apikey),
            )],
            payload: serde_json::to_string(&payload).map_err(|err| {
                io::Error::other(format!("serialize minimax openai payload failed: {err}"))
            })?,
        })
    }
}

fn detect_protocol(endpoint: &str) -> MinimaxProtocol {
    let normalized = endpoint.trim().trim_end_matches('/').to_ascii_lowercase();
    if normalized.ends_with("/v1/messages")
        || normalized.ends_with("/anthropic")
        || normalized.contains("/anthropic/")
    {
        MinimaxProtocol::Anthropic
    } else {
        MinimaxProtocol::OpenAi
    }
}

fn normalize_endpoint(endpoint: &str, protocol: MinimaxProtocol) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    match protocol {
        MinimaxProtocol::Anthropic => {
            if trimmed.ends_with("/v1/messages") {
                trimmed.to_string()
            } else if trimmed.ends_with("/anthropic") || trimmed.contains("/anthropic/") {
                format!("{trimmed}/v1/messages")
            } else {
                trimmed.to_string()
            }
        }
        MinimaxProtocol::OpenAi => {
            if trimmed.ends_with("/v1") {
                format!("{trimmed}/chat/completions")
            } else {
                trimmed.to_string()
            }
        }
    }
}

fn build_anthropic_messages(messages: &[ProviderMessage]) -> io::Result<Vec<Value>> {
    let mut serialized = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        match &messages[index] {
            ProviderMessage::UserText(text) => {
                serialized.push(json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": text }],
                }));
                index += 1;
            }
            ProviderMessage::User { parts } => {
                serialized.push(json!({
                    "role": "user",
                    "content": build_anthropic_user_content(parts),
                }));
                index += 1;
            }
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

                let mut results = Vec::new();
                index += 1;
                while index < messages.len() {
                    match &messages[index] {
                        ProviderMessage::UserToolResult {
                            tool_use_id,
                            content,
                        } => {
                            results.push(json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": content,
                            }));
                            index += 1;
                        }
                        _ => break,
                    }
                }

                if !results.is_empty() {
                    serialized.push(json!({
                        "role": "user",
                        "content": results,
                    }));
                }
            }
            ProviderMessage::UserToolResult { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "tool result must follow an assistant tool use",
                ));
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

fn parse_anthropic_response(response_body: &str) -> io::Result<ProviderResponse> {
    let response: Value = serde_json::from_str(response_body).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse minimax anthropic response failed: {err}"),
        )
    })?;
    if let Some(error_message) = extract_minimax_error(&response) {
        return Err(io::Error::other(error_message));
    }

    let content = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "minimax anthropic response missing content array",
            )
        })?;

    let mut tool_uses = Vec::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let id = block
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "tool_use missing id"))?;
            let name = block.get("name").and_then(Value::as_str).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "tool_use missing name")
            })?;
            let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
            tool_uses.push(ProviderToolUse {
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
            });
        }
    }

    if !tool_uses.is_empty() {
        return Ok(ProviderResponse::ToolUses(tool_uses));
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
            "minimax anthropic response missing text or tool_use",
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

fn parse_openai_response(response_body: &str) -> io::Result<ProviderResponse> {
    let response: Value = serde_json::from_str(response_body).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse minimax openai response failed: {err}"),
        )
    })?;
    if let Some(error_message) = extract_minimax_error(&response) {
        return Err(io::Error::other(error_message));
    }

    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "minimax openai response missing message",
            )
        })?;

    let text = extract_openai_text(message).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "minimax openai response missing content",
        )
    })?;

    Ok(ProviderResponse::Text(ProviderTextResponse {
        text,
        thinking: extract_openai_thinking(message),
    }))
}

fn extract_minimax_error(response: &Value) -> Option<String> {
    let error = response.get("error")?;
    let error_type = error.get("type").and_then(Value::as_str).unwrap_or("error");
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    let request_id = response
        .get("request_id")
        .and_then(Value::as_str)
        .or_else(|| response.get("trace_id").and_then(Value::as_str));

    let trimmed = message.trim();
    if trimmed.is_empty() {
        if let Some(id) = request_id {
            return Some(format!("minimax api error ({error_type}, request_id={id})"));
        }
        return Some(format!("minimax api error ({error_type})"));
    }

    if let Some(id) = request_id {
        Some(format!(
            "minimax api error ({error_type}, request_id={id}): {trimmed}"
        ))
    } else {
        Some(format!("minimax api error ({error_type}): {trimmed}"))
    }
}

fn extract_openai_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }

    let parts = content.as_array()?;
    let texts: Vec<&str> = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("content").and_then(Value::as_str))
        })
        .collect();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn extract_openai_thinking(message: &Value) -> Option<String> {
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
        let trimmed = reasoning.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(reasoning) = message.get("reasoning").and_then(Value::as_str) {
        let trimmed = reasoning.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let parts = message.get("content").and_then(Value::as_array)?;
    let reasonings: Vec<&str> = parts
        .iter()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("reasoning") | Some("thinking")
            )
        })
        .filter_map(|part| {
            part.get("thinking")
                .and_then(Value::as_str)
                .or_else(|| part.get("text").and_then(Value::as_str))
                .or_else(|| part.get("content").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if reasonings.is_empty() {
        None
    } else {
        Some(reasonings.join("\n"))
    }
}

fn openai_role(message: &ProviderMessage) -> &'static str {
    match message {
        ProviderMessage::UserText(_)
        | ProviderMessage::User { .. }
        | ProviderMessage::UserToolResult { .. } => "user",
        ProviderMessage::AssistantToolUse { .. } => "assistant",
    }
}

fn flatten_openai_message_content(message: &ProviderMessage) -> Value {
    match message {
        ProviderMessage::UserText(text) => Value::String(text.clone()),
        ProviderMessage::User { parts } => Value::Array(
            parts
                .iter()
                .map(|part| match part {
                    ProviderContentPart::Text(text) => json!({
                        "type": "text",
                        "text": text,
                    }),
                    ProviderContentPart::ImageBase64 { media_type, data } => json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{media_type};base64,{data}")
                        },
                    }),
                    ProviderContentPart::ImageFilePath { path, .. } => json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("file://{path}")
                        },
                    }),
                })
                .collect(),
        ),
        ProviderMessage::UserToolResult {
            tool_use_id,
            content,
        } => Value::String(format!("tool_result {tool_use_id}: {content}")),
        ProviderMessage::AssistantToolUse {
            assistant_content_json,
        } => Value::String(assistant_content_json.clone()),
    }
}

fn build_anthropic_user_content(parts: &[ProviderContentPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| match part {
            ProviderContentPart::Text(text) => json!({
                "type": "text",
                "text": text,
            }),
            ProviderContentPart::ImageBase64 { media_type, data } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                },
            }),
            ProviderContentPart::ImageFilePath { media_type, path } => {
                let data = fs::read(path)
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                    .unwrap_or_default();
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    },
                })
            }
        })
        .collect()
}

fn default_minimax_model(model: &str) -> &str {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        DEFAULT_MINIMAX_MODEL
    } else {
        trimmed
    }
}
