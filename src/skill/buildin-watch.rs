use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const WATCH_TOOL_SCHEMA_JSON: &str = "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Path of the file to read\"}},\"required\":[\"path\"]}";
const WATCH_BLACKLIST_KEY: &str = "watch.blacklist";
const DEFAULT_BLACKLIST: &[&str] = &[
    "~/.mylittlebotty/",
    "/etc/passwd",
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/master.passwd",
    "/etc/security/passwd",
    "/etc/opasswd",
    "/private/etc/passwd",
    "/private/etc/shadow",
    "/private/etc/gshadow",
    "/private/etc/master.passwd",
    "/private/etc/security/passwd",
    "/private/etc/opasswd",
    "C:\\Windows\\System32\\config\\SAM",
    "C:\\Windows\\System32\\config\\SECURITY",
    "C:\\Windows\\System32\\config\\SYSTEM",
    "C:\\Windows\\repair\\SAM",
];

pub struct BuildinWatchSkill;

impl BuildinWatchSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinWatchSkill {
    fn name(&self) -> &'static str {
        "watch"
    }

    fn description(&self) -> &'static str {
        "Read the content of a text file from the local workspace"
    }

    fn input_schema_json(&self) -> &'static str {
        WATCH_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let path = parse_path_argument(input_json)?;
        let resolved = resolve_path(&path)?;
        ensure_path_allowed(&resolved)?;
        let metadata = fs::metadata(&resolved)?;
        if metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch skill only supports files, not directories",
            ));
        }

        let content = fs::read_to_string(&resolved)?;
        let truncated = truncate_utf8(content.as_str(), MAX_OUTPUT_BYTES);
        let mut reply = format!("FILE {}\n{}", resolved.display(), truncated);
        if truncated.len() < content.len() {
            reply.push_str("\n...[truncated]");
        }
        Ok(reply)
    }
}

fn parse_path_argument(input: &str) -> io::Result<String> {
    if let Ok(value) = serde_json::from_str::<Value>(input) {
        if let Some(path) = value.get("path").and_then(Value::as_str) {
            return Ok(path.to_string());
        }
    }
    Ok(input.trim().to_string())
}

fn resolve_path(path: &str) -> io::Result<PathBuf> {
    let expanded = expand_user_path(path);
    let candidate = expanded.as_path();
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        env::current_dir()?.join(candidate)
    };
    absolute.canonicalize()
}

fn ensure_path_allowed(path: &Path) -> io::Result<()> {
    let rules = load_watch_blacklist()?;
    if rules.iter().any(|rule| rule.matches(path)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("watch skill access denied for {}", path.display()),
        ));
    }
    Ok(())
}

fn load_watch_blacklist() -> io::Result<Vec<BlacklistRule>> {
    let content = read_watch_config_file()?;

    let entries = content
        .as_deref()
        .and_then(parse_blacklist_from_config)
        .unwrap_or_else(|| {
            DEFAULT_BLACKLIST
                .iter()
                .map(|item| item.to_string())
                .collect()
        });

    Ok(entries
        .into_iter()
        .filter_map(|entry| BlacklistRule::from_entry(&entry))
        .collect())
}

fn read_watch_config_file() -> io::Result<Option<String>> {
    for path in watch_config_candidates() {
        match fs::read_to_string(&path) {
            Ok(content) => return Ok(Some(content)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

fn parse_blacklist_from_config(content: &str) -> Option<Vec<String>> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != WATCH_BLACKLIST_KEY {
            continue;
        }
        return Some(parse_blacklist_items(value));
    }
    None
}

fn parse_blacklist_items(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BlacklistRule {
    path: PathBuf,
    prefix: bool,
}

impl BlacklistRule {
    fn from_entry(entry: &str) -> Option<Self> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return None;
        }

        let prefix = trimmed.ends_with('/') || trimmed.ends_with('\\');
        let normalized = if prefix {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if normalized.is_empty() {
            return None;
        }

        Some(Self {
            path: expand_user_path(normalized),
            prefix,
        })
    }

    fn matches(&self, candidate: &Path) -> bool {
        candidate == self.path || (self.prefix && candidate.starts_with(&self.path))
    }
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if let Some(rest) = path.strip_prefix("~\\") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

fn watch_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("watch{}.conf", runtime_suffix()))
}

fn watch_config_candidates() -> Vec<PathBuf> {
    let runtime_path = watch_config_file();
    let plain_path = botty_root_dir().join("config").join("watch.conf");
    if runtime_path == plain_path {
        vec![runtime_path]
    } else {
        vec![runtime_path, plain_path]
    }
}

fn botty_root_dir() -> PathBuf {
    home_dir().join(".mylittlebotty")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}

fn truncate_utf8(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }

    let mut end = max_bytes;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_directory_rule_matches_children() {
        let rule = BlacklistRule::from_entry("~/.mylittlebotty/").unwrap();
        let candidate = home_dir()
            .join(".mylittlebotty")
            .join("config")
            .join("setup.conf");
        assert!(rule.matches(&candidate));
    }

    #[test]
    fn blacklist_file_rule_only_matches_exact_path() {
        let rule = BlacklistRule::from_entry("/etc/passwd").unwrap();
        assert!(rule.matches(Path::new("/etc/passwd")));
        assert!(!rule.matches(Path::new("/etc/passwd.bak")));
    }

    #[test]
    fn blacklist_private_etc_passwd_on_macos() {
        let rule = BlacklistRule::from_entry("/private/etc/passwd").unwrap();
        assert!(rule.matches(Path::new("/private/etc/passwd")));
    }

    #[test]
    fn parse_config_blacklist_items() {
        let content =
            "\n# comment\nwatch.blacklist = ~/.mylittlebotty/, /tmp/secret.txt , /etc/shadow\n";
        let items = parse_blacklist_from_config(content).unwrap();
        assert_eq!(
            items,
            vec![
                "~/.mylittlebotty/".to_string(),
                "/tmp/secret.txt".to_string(),
                "/etc/shadow".to_string()
            ]
        );
    }

    #[test]
    fn resolve_path_expands_tilde() {
        let resolved = resolve_path("~/.mylittlebotty").unwrap();
        assert_eq!(resolved, home_dir().join(".mylittlebotty"));
    }
}
