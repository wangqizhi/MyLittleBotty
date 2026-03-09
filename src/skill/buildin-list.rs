use crate::io as botty_io;
use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const LIST_TOOL_SCHEMA_JSON: &str = "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Path of the directory to list\"}},\"required\":[\"path\"]}";
const LIST_BLACKLIST_KEY: &str = "list.blacklist";
const DEFAULT_BLACKLIST: &[&str] = &["~/.mylittlebotty/"];

pub struct BuildinListSkill;

impl BuildinListSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinListSkill {
    fn name(&self) -> &'static str {
        "list"
    }

    fn description(&self) -> &'static str {
        "List the content of a directory from the local workspace"
    }

    fn input_schema_json(&self) -> &'static str {
        LIST_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let path = parse_path_argument(input_json)?;
        let resolved = match resolve_path(&path) {
            Ok(resolved) => resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(format_missing_directory_result(&path));
            }
            Err(err) => return Err(err),
        };
        ensure_path_allowed(&resolved)?;
        let metadata = match fs::metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(format_missing_directory_result(&path));
            }
            Err(err) => return Err(err),
        };
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "list skill only supports directories, not files",
            ));
        }

        let mut entries = fs::read_dir(&resolved)?
            .map(|entry| {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let suffix = if file_type.is_dir() {
                    "/"
                } else if file_type.is_symlink() {
                    "@"
                } else {
                    ""
                };
                Ok(format!("{name}{suffix}"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();

        if entries.is_empty() {
            return Ok(format!("DIR {}\n(empty)", resolved.display()));
        }

        Ok(format!(
            "DIR {}\n{}",
            resolved.display(),
            entries.join("\n")
        ))
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
    let current_dir = env::current_dir()?;
    let work_dir = ensure_work_dir_root()?;
    resolve_path_with_work_dir(&current_dir, &work_dir, path)
}

fn resolve_path_with_work_dir(
    current_dir: &Path,
    work_dir: &Path,
    path: &str,
) -> io::Result<PathBuf> {
    let expanded = expand_user_path(path);
    let candidate = expanded.as_path();
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        current_dir.join(candidate)
    };
    match absolute.canonicalize() {
        Ok(resolved) => Ok(resolved),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let fallback = work_dir.join(sanitize_user_path(path));
            fallback.canonicalize()
        }
        Err(err) => Err(err),
    }
}

fn format_missing_directory_result(path: &str) -> String {
    format!(
        "LIST_MISSING_DIR {path}\nThe requested directory does not exist. Do not call list again for this path unless the user explicitly asks to inspect a different existing directory. If the user wants to save, record, or update content, continue by using write instead."
    )
}

fn ensure_path_allowed(path: &Path) -> io::Result<()> {
    let rules = load_list_blacklist()?;
    if rules.iter().any(|rule| rule.matches(path)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("list skill access denied for {}", path.display()),
        ));
    }
    Ok(())
}

fn load_list_blacklist() -> io::Result<Vec<BlacklistRule>> {
    let content = read_list_config_file()?;

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

fn read_list_config_file() -> io::Result<Option<String>> {
    for path in list_config_candidates() {
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
        if key.trim() != LIST_BLACKLIST_KEY {
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

fn ensure_work_dir_root() -> io::Result<PathBuf> {
    let root = botty_io::effective_work_dir()?;
    fs::create_dir_all(&root)?;
    root.canonicalize()
}

fn sanitize_user_path(raw_path: &str) -> PathBuf {
    let mut relative = PathBuf::new();
    for component in Path::new(raw_path).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::ParentDir => {
                relative.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    relative
}

fn list_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("list{}.conf", runtime_suffix()))
}

fn list_config_candidates() -> Vec<PathBuf> {
    let runtime_path = list_config_file();
    let plain_path = botty_root_dir().join("config").join("list.conf");
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
    fn parse_config_blacklist_items() {
        let content =
            "\n# comment\nlist.blacklist = ~/.mylittlebotty/, /tmp/secret.txt , /etc/shadow\n";
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

    #[test]
    fn expand_user_path_supports_home_symbol() {
        assert_eq!(expand_user_path("~"), home_dir());
        assert_eq!(expand_user_path("~/tmp"), home_dir().join("tmp"));
    }

    #[test]
    fn resolve_path_falls_back_to_work_dir_for_relative_path() {
        let current_dir = unique_test_dir("list-current");
        let work_dir = unique_test_dir("list-work");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        let notes_dir = work_dir.join("list-fallback-relative");
        fs::create_dir_all(&notes_dir).unwrap();

        let resolved =
            resolve_path_with_work_dir(&current_dir, &work_dir, "list-fallback-relative").unwrap();
        assert_eq!(resolved, notes_dir.canonicalize().unwrap());

        fs::remove_dir_all(current_dir).unwrap();
        fs::remove_dir_all(work_dir).unwrap();
    }

    #[test]
    fn resolve_path_falls_back_to_work_dir_for_absolute_path() {
        let current_dir = unique_test_dir("list-current");
        let work_dir = unique_test_dir("list-work");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        let dir = work_dir.join("tmp").join("list-fallback-absolute");
        fs::create_dir_all(&dir).unwrap();

        let resolved =
            resolve_path_with_work_dir(&current_dir, &work_dir, "/tmp/list-fallback-absolute")
                .unwrap();
        assert_eq!(resolved, dir.canonicalize().unwrap());

        fs::remove_dir_all(current_dir).unwrap();
        fs::remove_dir_all(work_dir).unwrap();
    }

    #[test]
    fn resolve_path_keeps_missing_fallback_directory_as_not_found() {
        let current_dir = unique_test_dir("list-current");
        let work_dir = unique_test_dir("list-work");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        let dir = work_dir.join("notes");
        let err = resolve_path_with_work_dir(&current_dir, &work_dir, "notes").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!dir.exists());

        fs::remove_dir_all(current_dir).unwrap();
        fs::remove_dir_all(work_dir).unwrap();
    }

    #[test]
    fn missing_directory_result_guides_model_to_write() {
        let result = format_missing_directory_result("notes");
        assert!(result.contains("LIST_MISSING_DIR notes"));
        assert!(result.contains("Do not call list again"));
        assert!(result.contains("continue by using write instead"));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{nanos}-{name}"))
    }
}
