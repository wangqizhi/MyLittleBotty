pub mod provider_minimax;

use std::io;

pub struct ProviderRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub payload: String,
}

pub trait LlmProvider {
    fn build_request(&self, input: &str) -> io::Result<ProviderRequest>;
}
