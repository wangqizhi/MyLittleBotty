use serde::Deserialize;
use std::fs;
use std::io;
use std::process::Command;

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

#[derive(Clone, Debug)]
pub struct TelegramInboundImage {
    pub local_path: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TelegramInboundMessage {
    pub update_id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub text: String,
    pub images: Vec<TelegramInboundImage>,
}

pub struct TelegramClient {
    api_base: String,
    bot_token: String,
}

impl TelegramClient {
    pub fn new(api_base: String, bot_token: String) -> Self {
        Self {
            api_base,
            bot_token,
        }
    }

    pub fn fetch_updates(&self, offset: i64) -> io::Result<Vec<TelegramInboundMessage>> {
        let url = format!(
            "{}/bot{}/getUpdates?timeout=0&offset={offset}&allowed_updates=%5B%22message%22%5D",
            self.api_base, self.bot_token
        );
        let output = Command::new("curl").arg("-fsS").arg(url).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format_curl_failure(
                "getUpdates",
                &self.api_base,
                &output,
            )));
        }
        let body = String::from_utf8_lossy(&output.stdout);
        let response: TelegramUpdatesResponse = serde_json::from_str(&body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse telegram updates failed: {err}"),
            )
        })?;
        if !response.ok {
            return Err(io::Error::other("telegram getUpdates returned ok=false"));
        }
        response.into_messages(self)
    }

    pub fn send_message(&self, chat_id: i64, text: &str) -> io::Result<()> {
        let url = format!("{}/bot{}/sendMessage", self.api_base, self.bot_token);
        let output = Command::new("curl")
            .arg("-fsS")
            .arg("-X")
            .arg("POST")
            .arg(url)
            .arg("--data-urlencode")
            .arg(format!("chat_id={chat_id}"))
            .arg("--data-urlencode")
            .arg(format!("text={text}"))
            .output()?;

        if !output.status.success() {
            return Err(io::Error::other(format_curl_failure(
                "sendMessage",
                &self.api_base,
                &output,
            )));
        }
        Ok(())
    }

    pub fn send_photo(&self, chat_id: i64, path: &str, caption: &str) -> io::Result<()> {
        let url = format!("{}/bot{}/sendPhoto", self.api_base, self.bot_token);
        let mut command = Command::new("curl");
        command
            .arg("-fsS")
            .arg("-X")
            .arg("POST")
            .arg(url)
            .arg("-F")
            .arg(format!("chat_id={chat_id}"))
            .arg("-F")
            .arg(format!("photo=@{path}"));
        if !caption.trim().is_empty() {
            command.arg("-F").arg(format!("caption={caption}"));
        }
        let output = command.output()?;

        if !output.status.success() {
            return Err(io::Error::other(format_curl_failure(
                "sendPhoto",
                &self.api_base,
                &output,
            )));
        }
        Ok(())
    }

    fn download_photo(&self, file_id: &str, update_id: i64) -> io::Result<TelegramInboundImage> {
        let file_path = self.resolve_file_path(file_id)?;
        let ext = file_path
            .rsplit('.')
            .next()
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| "jpg".to_string());
        let mime_type = Some(
            match ext.as_str() {
                "png" => "image/png",
                "webp" => "image/webp",
                "gif" => "image/gif",
                _ => "image/jpeg",
            }
            .to_string(),
        );
        let dir = std::env::temp_dir().join("mylittlebotty-chatbot-images");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{update_id}-{file_id}.{ext}"));
        let url = format!(
            "{}/file/bot{}/{}",
            self.api_base,
            self.bot_token,
            file_path.trim_start_matches('/')
        );
        let output = Command::new("curl")
            .arg("-fsS")
            .arg("-o")
            .arg(&path)
            .arg(url)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format_curl_failure(
                "download telegram photo",
                &self.api_base,
                &output,
            )));
        }
        Ok(TelegramInboundImage {
            local_path: path.to_string_lossy().to_string(),
            mime_type,
        })
    }

    fn resolve_file_path(&self, file_id: &str) -> io::Result<String> {
        let url = format!(
            "{}/bot{}/getFile?file_id={}",
            self.api_base, self.bot_token, file_id
        );
        let output = Command::new("curl").arg("-fsS").arg(url).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format_curl_failure(
                "getFile",
                &self.api_base,
                &output,
            )));
        }
        let body = String::from_utf8_lossy(&output.stdout);
        let response: TelegramGetFileResponse = serde_json::from_str(&body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse telegram getFile response failed: {err}"),
            )
        })?;
        if !response.ok {
            return Err(io::Error::other("telegram getFile returned ok=false"));
        }
        response
            .result
            .and_then(|result| result.file_path)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "telegram file_path missing"))
    }
}

#[derive(Deserialize)]
struct TelegramUpdatesResponse {
    ok: bool,
    result: Vec<TelegramUpdate>,
}

#[derive(Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Deserialize)]
struct TelegramMessage {
    chat: TelegramPeer,
    from: Option<TelegramPeer>,
    text: Option<String>,
    caption: Option<String>,
    photo: Option<Vec<TelegramPhotoSize>>,
}

#[derive(Deserialize)]
struct TelegramPeer {
    id: i64,
}

#[derive(Deserialize)]
struct TelegramPhotoSize {
    file_id: String,
    file_size: Option<i64>,
}

#[derive(Deserialize)]
struct TelegramGetFileResponse {
    ok: bool,
    result: Option<TelegramFileResult>,
}

#[derive(Deserialize)]
struct TelegramFileResult {
    file_path: Option<String>,
}

fn format_curl_failure(action: &str, api_base: &str, output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr);
    let exit_code = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    format!(
        "curl {action} failed: exit_code={exit_code}, api_base={api_base}, proxy_env=[{}], stderr={}",
        describe_proxy_env(),
        detail.trim()
    )
}

fn describe_proxy_env() -> String {
    let keys = [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
    ];
    let mut values = Vec::new();
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                values.push(format!("{key}={value}"));
            }
        }
    }
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

impl TelegramUpdatesResponse {
    fn into_messages(self, client: &TelegramClient) -> io::Result<Vec<TelegramInboundMessage>> {
        let mut messages = Vec::new();
        for update in self.result {
            let Some(message) = update.message else {
                continue;
            };
            let Some(from) = message.from else {
                continue;
            };
            let text = message
                .caption
                .or(message.text)
                .unwrap_or_default()
                .trim()
                .to_string();
            let mut images = Vec::new();
            if let Some(photo_sizes) = message.photo {
                if let Some(best) = photo_sizes
                    .into_iter()
                    .max_by_key(|item| item.file_size.unwrap_or_default())
                {
                    images.push(client.download_photo(&best.file_id, update.update_id)?);
                }
            }
            if text.is_empty() && images.is_empty() {
                continue;
            }
            messages.push(TelegramInboundMessage {
                update_id: update.update_id,
                chat_id: message.chat.id,
                user_id: from.id,
                text,
                images,
            });
        }
        Ok(messages)
    }
}
