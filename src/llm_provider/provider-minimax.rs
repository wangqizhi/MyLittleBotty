use crate::botty_brain::BrainConfig;
use crate::llm_provider::provider_openai::OpenAiProvider;
use crate::llm_provider::{
    LlmProvider, ProviderMessage, ProviderRequest, ProviderResponse, ProviderToolDefinition,
};
use serde_json::{self, json, Value};
use std::io;

const MINIMAX_REQUEST_MAX_TOKENS: u64 = 64_000;

pub struct MinimaxProvider {
    inner: OpenAiProvider,
}

impl MinimaxProvider {
    pub fn from_config(config: &BrainConfig) -> Self {
        Self {
            inner: OpenAiProvider::from_config(config),
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
        let mut request = self.inner.build_request(system_prompt, messages, tools)?;
        let mut payload: Value = serde_json::from_str(&request.payload).map_err(|err| {
            io::Error::other(format!("parse minimax payload failed: {err}"))
        })?;
        payload["max_tokens"] = json!(MINIMAX_REQUEST_MAX_TOKENS);
        request.payload = serde_json::to_string(&payload)
            .map_err(|err| io::Error::other(format!("serialize minimax payload failed: {err}")))?;
        Ok(request)
    }

    fn parse_response(&self, response_body: &str) -> io::Result<ProviderResponse> {
        self.inner.parse_response(response_body)
    }
}
