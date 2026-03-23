use base64::Engine;
use serde::Deserialize;
use std::fs;
use std::io;

use crate::botty_brain::BottyBrain;
use crate::llm_provider::{ProviderContentPart, ProviderMessage, ProviderResponse};
use crate::skill::BottySkill;

pub struct BuildinImageSkill;

#[derive(Deserialize)]
struct ImageSkillInput {
    source: Option<String>,
    user_text: Option<String>,
    images: Vec<ImageSkillImage>,
}

#[derive(Deserialize)]
struct ImageSkillImage {
    local_path: Option<String>,
    mime_type: Option<String>,
}

impl BuildinImageSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinImageSkill {
    fn name(&self) -> &'static str {
        "image"
    }

    fn description(&self) -> &'static str {
        "Analyze inbound images with the configured image provider, summarize the image, and extract visible text before handing the result to the active provider."
    }

    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"source":{"type":"string","description":"Inbound source name such as telegram"},"user_text":{"type":"string","description":"Optional user text that came with the image"},"images":{"type":"array","description":"Inbound images to analyze","items":{"type":"object","properties":{"local_path":{"type":"string","description":"Downloaded local image path"},"mime_type":{"type":"string","description":"Image mime type"}},"required":["local_path"]}}},"required":["images"]}"#
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let input: ImageSkillInput = serde_json::from_str(input_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse image skill input failed: {err}"),
            )
        })?;
        if input.images.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image skill requires at least one image",
            ));
        }

        let mut parts = Vec::new();
        parts.push(ProviderContentPart::Text(build_image_prompt(
            input.source.as_deref(),
            input.user_text.as_deref(),
        )));
        for image in &input.images {
            let local_path = image.local_path.as_deref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "image local_path is required")
            })?;
            let bytes = fs::read(local_path)?;
            parts.push(ProviderContentPart::ImageBase64 {
                media_type: image
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "image/jpeg".to_string()),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }

        let response = BottyBrain::from_image_setup()?.think(
            "You are an image understanding assistant. Summarize what is visible in the image, extract any readable text, and connect the visual evidence to the user's likely need. Reply in the same language as the user's text when possible.",
            &[ProviderMessage::User { parts }],
            &[],
        )?;
        match response {
            ProviderResponse::Text(reply) => Ok(reply.text.trim().to_string()),
            ProviderResponse::ToolUses(_) => Err(io::Error::other(
                "image skill unexpectedly returned a tool call",
            )),
        }
    }
}

fn build_image_prompt(source: Option<&str>, user_text: Option<&str>) -> String {
    let source = source.unwrap_or("chatbot");
    let user_text = user_text.unwrap_or("").trim();
    if user_text.is_empty() {
        format!(
            "Source: {source}\nThe user only sent an image.\nPlease summarize the image content in detail, extract any visible text, and infer what the user is likely asking for."
        )
    } else {
        format!(
            "Source: {source}\nUser request: {user_text}\nPlease summarize the image content, extract any visible text, and explain which parts of the image are relevant to the user's request."
        )
    }
}
