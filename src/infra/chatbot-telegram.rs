use std::io;
use std::process::Command;

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

#[derive(Clone, Debug)]
pub struct TelegramInboundMessage {
    pub update_id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub text: String,
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
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::other(format!(
                "curl getUpdates failed: {}",
                detail.trim()
            )));
        }
        let body = String::from_utf8_lossy(&output.stdout);
        Ok(parse_telegram_updates(&body))
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
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::other(format!(
                "curl sendMessage failed: {}",
                detail.trim()
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
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::other(format!(
                "curl sendPhoto failed: {}",
                detail.trim()
            )));
        }
        Ok(())
    }
}

fn parse_telegram_updates(body: &str) -> Vec<TelegramInboundMessage> {
    let mut updates = Vec::new();
    let mut start = 0usize;

    while let Some(rel) = body[start..].find("\"update_id\"") {
        let abs = start + rel;
        let end = match body[abs + 1..].find("\"update_id\"") {
            Some(next_rel) => abs + 1 + next_rel,
            None => body.len(),
        };
        let chunk = &body[abs..end];

        if let (Some(update_id), Some(chat_id), Some(user_id), Some(text)) = (
            parse_number_field(chunk, "\"update_id\""),
            parse_number_field(chunk, "\"chat\""),
            parse_number_field(chunk, "\"from\""),
            parse_string_field(chunk, "\"text\""),
        ) {
            updates.push(TelegramInboundMessage {
                update_id,
                chat_id,
                user_id,
                text,
            });
        }

        start = end;
    }
    updates
}

fn parse_number_field(chunk: &str, field_name: &str) -> Option<i64> {
    if field_name == "\"chat\"" || field_name == "\"from\"" {
        let object_idx = chunk.find(field_name)?;
        let object_part = &chunk[object_idx..];
        let id_idx = object_part.find("\"id\"")?;
        parse_number_after_colon(&object_part[id_idx + 4..])
    } else {
        let idx = chunk.find(field_name)?;
        parse_number_after_colon(&chunk[idx + field_name.len()..])
    }
}

fn parse_number_after_colon(s: &str) -> Option<i64> {
    let colon = s.find(':')?;
    let rest = s[colon + 1..].trim_start();
    let mut end = 0usize;
    for (i, ch) in rest.char_indices() {
        if ch.is_ascii_digit() || (i == 0 && ch == '-') {
            end = i + ch.len_utf8();
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    rest[..end].parse::<i64>().ok()
}

fn parse_string_field(chunk: &str, field_name: &str) -> Option<String> {
    let idx = chunk.find(field_name)?;
    let after = &chunk[idx + field_name.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    if !value.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = value[1..].chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            return Some(out);
        }
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'u' => {
                let cp1 = parse_u16_hex_from_chars(&mut chars)?;
                if (0xD800..=0xDBFF).contains(&cp1) {
                    let backslash = chars.next()?;
                    let u = chars.next()?;
                    if backslash != '\\' || u != 'u' {
                        return None;
                    }
                    let cp2 = parse_u16_hex_from_chars(&mut chars)?;
                    if !(0xDC00..=0xDFFF).contains(&cp2) {
                        return None;
                    }
                    let high = (cp1 as u32) - 0xD800;
                    let low = (cp2 as u32) - 0xDC00;
                    let code = 0x10000 + ((high << 10) | low);
                    out.push(char::from_u32(code)?);
                } else if (0xDC00..=0xDFFF).contains(&cp1) {
                    return None;
                } else {
                    out.push(char::from_u32(cp1 as u32)?);
                }
            }
            _ => return None,
        }
    }
    None
}

fn parse_u16_hex_from_chars(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<u16> {
    let mut hex = String::with_capacity(4);
    for _ in 0..4 {
        let ch = chars.next()?;
        if !ch.is_ascii_hexdigit() {
            return None;
        }
        hex.push(ch);
    }
    u16::from_str_radix(&hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_telegram_updates;

    #[test]
    fn parses_unicode_escaped_text() {
        let body = r#"{"ok":true,"result":[{"update_id":1,"message":{"message_id":2,"from":{"id":3},"chat":{"id":4},"text":"\u4f60\u597d"}}]}"#;
        let updates = parse_telegram_updates(body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].text, "你好");
    }

    #[test]
    fn parses_surrogate_pair_text() {
        let body = r#"{"ok":true,"result":[{"update_id":1,"message":{"message_id":2,"from":{"id":3},"chat":{"id":4},"text":"\ud83d\ude80"}}]}"#;
        let updates = parse_telegram_updates(body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].text, "🚀");
    }
}
