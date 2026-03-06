use crate::botty_brain::BrainConfig;
use crate::llm_provider::{
    LlmProvider, ProviderMessage, ProviderRequest, ProviderResponse, ProviderTextResponse,
    ProviderToolDefinition,
};
use serde_json::{json, Value};
use std::io;

const DEFAULT_OPENAI_MODEL: &str = "MiniMax-M2.1";

pub struct OpenAiProvider {
    endpoint: String,
    apikey: String,
    model: String,
}

impl OpenAiProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        Self {
            endpoint: normalize_endpoint(config.endpoint.as_str()),
            apikey: config.apikey.trim().to_string(),
            model: config.model_name().to_string(),
        }
    }
}

impl LlmProvider for OpenAiProvider {
    fn build_request(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        _tools: &[ProviderToolDefinition],
    ) -> io::Result<ProviderRequest> {
        let mut serialized_messages = vec![json!({
            "role": "system",
            "content": system_prompt,
        })];
        for message in messages {
            serialized_messages.push(json!({
                "role": openai_role(message),
                "content": flatten_message_content(message),
            }));
        }

        let payload = json!({
            "model": default_openai_model(self.model.as_str()),
            "messages": serialized_messages,
        });

        Ok(ProviderRequest {
            url: self.endpoint.clone(),
            headers: auth_headers(self.apikey.as_str()),
            payload: serde_json::to_string(&payload).map_err(|err| {
                io::Error::other(format!("serialize openai payload failed: {err}"))
            })?,
        })
    }

    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse> {
        let response: Value = serde_json::from_str(response_body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse openai response failed: {err}"),
            )
        })?;
        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "openai response missing message",
                )
            })?;
        let text = extract_openai_text(message).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "openai response missing content",
            )
        })?;
        Ok(ProviderResponse::Text(ProviderTextResponse {
            text,
            thinking: extract_openai_thinking(message),
        }))
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

fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        return format!("{trimmed}/chat/completions");
    }
    trimmed.to_string()
}

fn openai_role(message: &ProviderMessage) -> &'static str {
    match message {
        ProviderMessage::UserText(_) | ProviderMessage::UserToolResult { .. } => "user",
        ProviderMessage::AssistantToolUse { .. } => "assistant",
    }
}

fn flatten_message_content(message: &ProviderMessage) -> String {
    match message {
        ProviderMessage::UserText(text) => text.clone(),
        ProviderMessage::UserToolResult {
            tool_use_id,
            content,
        } => format!("tool_result {tool_use_id}: {content}"),
        ProviderMessage::AssistantToolUse {
            assistant_content_json,
        } => assistant_content_json.clone(),
    }
}

fn auth_headers(apikey: &str) -> Vec<(String, String)> {
    if apikey.is_empty() {
        return Vec::new();
    }

    vec![("Authorization".to_string(), format!("Bearer {apikey}"))]
}

fn default_openai_model(model: &str) -> &str {
    if model.is_empty() {
        DEFAULT_OPENAI_MODEL
    } else {
        model
    }
}
