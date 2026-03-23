use base64::Engine;
use serde::Deserialize;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::botty_brain::{log_debug_line_if_enabled, BottyBrain};
use crate::llm_provider::{ProviderContentPart, ProviderMessage, ProviderResponse};
use crate::skill::BottySkill;

pub struct BuildinImageSkill;

const IMAGE_PROVIDER_COMPRESS_THRESHOLD_BYTES: u64 = 120 * 1024;
const IMAGE_PROVIDER_MAX_DIMENSION: u32 = 768;
const IMAGE_PROVIDER_JPEG_QUALITY: u32 = 55;

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

        log_image_request_debug(&input)?;

        let mut parts = Vec::new();
        parts.push(ProviderContentPart::Text(build_image_prompt(
            input.source.as_deref(),
            input.user_text.as_deref(),
        )));
        for image in &input.images {
            let local_path = image.local_path.as_deref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "image local_path is required")
            })?;
            let media_type = image
                .mime_type
                .clone()
                .unwrap_or_else(|| "image/jpeg".to_string());
            let prepared = prepare_image_for_provider(local_path)?;
            parts.push(ProviderContentPart::ImageBase64 {
                media_type,
                data: base64::engine::general_purpose::STANDARD.encode(prepared.bytes),
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

fn log_image_request_debug(input: &ImageSkillInput) -> io::Result<()> {
    let source = input.source.as_deref().unwrap_or("chatbot").trim();
    let user_text = input.user_text.as_deref().unwrap_or("").trim();
    let mut lines = vec![format!(
        "source={source} user_text_len={} image_count={}",
        user_text.len(),
        input.images.len()
    )];

    for (index, image) in input.images.iter().enumerate() {
        let local_path = image.local_path.as_deref().unwrap_or("").trim();
        let mime_type = image.mime_type.as_deref().unwrap_or("image/jpeg").trim();
        let file_size = if local_path.is_empty() {
            "unknown".to_string()
        } else {
            fs::metadata(local_path)
                .map(|meta| meta.len().to_string())
                .unwrap_or_else(|_| "unavailable".to_string())
        };
        lines.push(format!(
            "image[{index}] path={local_path} mime_type={mime_type} size_bytes={file_size}"
        ));
    }

    log_debug_line_if_enabled("image-request-meta", &lines.join("\n"), None)
}

struct PreparedImage {
    bytes: Vec<u8>,
}

fn prepare_image_for_provider(local_path: &str) -> io::Result<PreparedImage> {
    let metadata = fs::metadata(local_path)?;
    if metadata.len() <= IMAGE_PROVIDER_COMPRESS_THRESHOLD_BYTES {
        return Ok(PreparedImage {
            bytes: fs::read(local_path)?,
        });
    }

    let compressed_path = compressed_image_output_path(local_path);
    let output = Command::new("sips")
        .arg("-Z")
        .arg(IMAGE_PROVIDER_MAX_DIMENSION.to_string())
        .arg("-s")
        .arg("format")
        .arg("jpeg")
        .arg("--setProperty")
        .arg("formatOptions")
        .arg(IMAGE_PROVIDER_JPEG_QUALITY.to_string())
        .arg(local_path)
        .arg("--out")
        .arg(&compressed_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(io::Error::other(format!(
            "compress image for provider failed: {detail}"
        )));
    }

    let compressed_meta = fs::metadata(&compressed_path)?;
    log_debug_line_if_enabled(
        "image-compress",
        &format!(
            "source_path={} compressed_path={} original_size_bytes={} compressed_size_bytes={} max_dimension={} jpeg_quality={}",
            local_path,
            compressed_path.display(),
            metadata.len(),
            compressed_meta.len(),
            IMAGE_PROVIDER_MAX_DIMENSION,
            IMAGE_PROVIDER_JPEG_QUALITY
        ),
        None,
    )?;

    Ok(PreparedImage {
        bytes: fs::read(compressed_path)?,
    })
}

fn compressed_image_output_path(local_path: &str) -> PathBuf {
    let input = Path::new(local_path);
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("image");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    env::temp_dir().join(format!(
        "mylittlebotty-image-provider-{}-{}-{}.jpg",
        std::process::id(),
        stem,
        millis
    ))
}
