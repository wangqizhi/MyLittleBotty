use crate::infra::chatbot_weixin::{WeixinClient, DEFAULT_API_BASE};
use clap::Args;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Args, Debug, Default)]
#[command(about = "Log in a personal Weixin account by QR code and save chatbot config")]
pub struct WeixinLoginCommand {
    #[arg(long, default_value = DEFAULT_API_BASE)]
    api_base: String,

    #[arg(long, default_value_t = 480)]
    timeout_seconds: u64,
}

impl WeixinLoginCommand {
    pub fn run(self) -> io::Result<()> {
        let client = WeixinClient::new(self.api_base.clone(), String::new());
        let start = client.start_login()?;
        println!("Scan this QR code with Weixin:");
        println!("{}", start.qrcode_url);
        println!();
        println!(
            "Waiting for confirmation for up to {} seconds...",
            self.timeout_seconds.max(1)
        );

        let deadline = Instant::now() + Duration::from_secs(self.timeout_seconds.max(1));
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "weixin login timed out",
                ));
            }

            match client.poll_login_status(&start.qrcode) {
                Ok(Some(result)) => {
                    save_weixin_login_config(
                        &result.base_url,
                        &result.bot_token,
                        &result.account_id,
                        &result.user_id,
                    )?;
                    println!("Weixin login succeeded.");
                    println!("account_id={}", result.account_id);
                    if !result.user_id.trim().is_empty() {
                        println!("paired_user_id={}", result.user_id);
                    }
                    println!("Config updated at {}", setup_config_file().display());
                    println!("Run `mylittlebotty restart` if Botty is already running.");
                    return Ok(());
                }
                Ok(None) => thread::sleep(Duration::from_secs(1)),
                Err(err) if err.kind() == io::ErrorKind::TimedOut => return Err(err),
                Err(err) => return Err(err),
            }
        }
    }
}

fn save_weixin_login_config(
    api_base: &str,
    apikey: &str,
    account_id: &str,
    user_id: &str,
) -> io::Result<()> {
    let path = setup_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();

    upsert_line(&mut lines, "chatbot.weixin.enabled", "true");
    upsert_line(&mut lines, "chatbot.weixin.api_base", api_base);
    upsert_line(&mut lines, "chatbot.weixin.apikey", apikey);
    upsert_line(&mut lines, "chatbot.weixin.account_id", account_id);
    upsert_line(&mut lines, "chatbot.weixin.user_id", user_id);
    ensure_provider_enabled(&mut lines, "weixin");

    let mut serialized = lines.join("\n");
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    fs::write(path, serialized)
}

fn ensure_provider_enabled(lines: &mut Vec<String>, provider: &str) {
    if let Some(index) = lines
        .iter()
        .position(|line| line.starts_with("chatbot.provider="))
    {
        let existing = lines[index]
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or_default();
        let mut providers: Vec<String> = existing
            .split(',')
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .map(|item| item.to_string())
            .collect();
        if !providers.iter().any(|item| item == provider) {
            providers.push(provider.to_string());
        }
        lines[index] = format!("chatbot.provider={}", providers.join(","));
    } else {
        lines.push(format!("chatbot.provider={provider}"));
    }
}

fn upsert_line(lines: &mut Vec<String>, key: &str, value: &str) {
    let next = format!("{key}={value}");
    if let Some(index) = lines
        .iter()
        .position(|line| line.starts_with(&format!("{key}=")))
    {
        lines[index] = next;
    } else {
        lines.push(next);
    }
}

fn setup_config_file() -> PathBuf {
    botty_root_dir().join("config").join(format!(
        "setup{}.conf",
        if cfg!(debug_assertions) { "-dev" } else { "" }
    ))
}

fn botty_root_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}
