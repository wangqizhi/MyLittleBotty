use crate::io as botty_io;
use crate::skill::BottySkill;
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const SEARCH_TOOL_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "kind": {
      "type": "string",
      "enum": ["directory", "file", "content"],
      "description": "Search directories by name, files by name, or text content inside files"
    },
    "query": {
      "type": "string",
      "description": "The text to search for"
    },
    "path": {
      "type": "string",
      "description": "Root directory to search under. It is always resolved inside the configured work dir"
    },
    "max_results": {
      "type": "integer",
      "description": "Maximum number of matches to return. Default 20, hard limit 100"
    },
    "case_sensitive": {
      "type": "boolean",
      "description": "Whether matching should be case-sensitive. Default false"
    }
  },
  "required": ["kind", "query"]
}"#;
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

pub struct BuildinSearchSkill;

impl BuildinSearchSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinSearchSkill {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> &'static str {
        "Search directories, files, or file content only inside the configured Botty work dir"
    }

    fn input_schema_json(&self) -> &'static str {
        SEARCH_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let input: Value = serde_json::from_str(input_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse search tool input json failed: {err}"),
            )
        })?;

        let kind = required_string(&input, "kind")?;
        let query = required_string(&input, "query")?.trim();
        if query.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "search tool requires a non-empty query",
            ));
        }

        let raw_root = optional_string(&input, "path").unwrap_or(".");
        let max_results = optional_usize(&input, "max_results")
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS)
            .max(1);
        let case_sensitive = input
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let workspace_root = ensure_work_dir_root()?;
        let search_root = resolve_search_root(&workspace_root, raw_root)?;

        match kind {
            "directory" => search_directories(
                &workspace_root,
                &search_root,
                query,
                case_sensitive,
                max_results,
            ),
            "file" => search_files(
                &workspace_root,
                &search_root,
                query,
                case_sensitive,
                max_results,
            ),
            "content" => search_content(
                &workspace_root,
                &search_root,
                query,
                case_sensitive,
                max_results,
            ),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported search kind: {other}"),
            )),
        }
    }
}

fn required_string<'a>(input: &'a Value, key: &str) -> io::Result<&'a str> {
    input.get(key).and_then(Value::as_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("search tool input requires string field `{key}`"),
        )
    })
}

fn optional_string<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn optional_usize(input: &Value, key: &str) -> Option<usize> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn ensure_work_dir_root() -> io::Result<PathBuf> {
    let root = botty_io::effective_work_dir()?;
    fs::create_dir_all(&root)?;
    root.canonicalize()
}

fn resolve_search_root(workspace_root: &Path, raw_path: &str) -> io::Result<PathBuf> {
    let workspace_root = workspace_root.canonicalize()?;
    let trimmed = raw_path.trim();
    let joined = if trimmed.is_empty() {
        workspace_root.clone()
    } else {
        let candidate = Path::new(trimmed);
        if candidate.is_absolute() {
            normalize_path(candidate)
        } else {
            workspace_root.join(sanitize_user_path(trimmed))
        }
    };
    let normalized = normalize_path(&joined);
    let safe = normalized.canonicalize().map_err(|err| {
        if err.kind() == io::ErrorKind::NotFound {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("search root does not exist: {}", normalized.display()),
            )
        } else {
            err
        }
    })?;

    if !safe.starts_with(&workspace_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "search skill can only access inside configured work dir: {}",
                workspace_root.display()
            ),
        ));
    }

    if !safe.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("search path must be a directory: {}", safe.display()),
        ));
    }

    Ok(safe)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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

fn format_path_result(
    kind: &str,
    root: &Path,
    matches: &[PathBuf],
    limit: usize,
) -> io::Result<String> {
    if matches.is_empty() {
        return Ok(format!("SEARCH {kind} {}\n(no matches)", root.display()));
    }

    let lines = matches
        .iter()
        .take(limit)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let suffix = if matches.len() > limit {
        format!("\n...[showing first {limit} of {} matches]", matches.len())
    } else {
        String::new()
    };
    Ok(format!(
        "SEARCH {kind} {}\n{}{}",
        root.display(),
        lines.join("\n"),
        suffix
    ))
}

#[cfg(target_os = "macos")]
fn search_directories(
    workspace_root: &Path,
    search_root: &Path,
    query: &str,
    case_sensitive: bool,
    max_results: usize,
) -> io::Result<String> {
    let pattern = build_find_name_pattern(query);
    let flag = if case_sensitive { "-name" } else { "-iname" };
    let output = Command::new("/usr/bin/find")
        .arg(search_root)
        .arg("-type")
        .arg("d")
        .arg(flag)
        .arg(&pattern)
        .output()?;
    ensure_success("find", &output)?;
    let matches = collect_find_paths(workspace_root, &output.stdout)?;
    format_path_result("directory", search_root, &matches, max_results)
}

#[cfg(target_os = "macos")]
fn search_files(
    workspace_root: &Path,
    search_root: &Path,
    query: &str,
    case_sensitive: bool,
    max_results: usize,
) -> io::Result<String> {
    let pattern = build_find_name_pattern(query);
    let flag = if case_sensitive { "-name" } else { "-iname" };
    let output = Command::new("/usr/bin/find")
        .arg(search_root)
        .arg("-type")
        .arg("f")
        .arg(flag)
        .arg(&pattern)
        .output()?;
    ensure_success("find", &output)?;
    let matches = collect_find_paths(workspace_root, &output.stdout)?;
    format_path_result("file", search_root, &matches, max_results)
}

#[cfg(target_os = "macos")]
fn search_content(
    workspace_root: &Path,
    search_root: &Path,
    query: &str,
    case_sensitive: bool,
    max_results: usize,
) -> io::Result<String> {
    let mut command = Command::new("/usr/bin/grep");
    command.arg("-R").arg("-n").arg("-I");
    if !case_sensitive {
        command.arg("-i");
    }
    command.arg(query).arg(search_root);

    let output = command.output()?;
    if let Some(code) = output.status.code() {
        if code > 1 {
            return Err(io::Error::other(format!(
                "grep failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    } else if !output.status.success() {
        return Err(io::Error::other("grep terminated by signal"));
    }

    let mut lines = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(formatted) = format_grep_match_line(workspace_root, line)? {
            lines.push(formatted);
        }
    }

    if lines.is_empty() {
        return Ok(format!(
            "SEARCH content {}\n(no matches)",
            search_root.display()
        ));
    }

    let suffix = if lines.len() > max_results {
        format!(
            "\n...[showing first {max_results} of {} matches]",
            lines.len()
        )
    } else {
        String::new()
    };
    Ok(format!(
        "SEARCH content {}\n{}{}",
        search_root.display(),
        lines
            .into_iter()
            .take(max_results)
            .collect::<Vec<_>>()
            .join("\n"),
        suffix
    ))
}

#[cfg(not(target_os = "macos"))]
fn search_directories(
    _workspace_root: &Path,
    _search_root: &Path,
    _query: &str,
    _case_sensitive: bool,
    _max_results: usize,
) -> io::Result<String> {
    unsupported_platform()
}

#[cfg(not(target_os = "macos"))]
fn search_files(
    _workspace_root: &Path,
    _search_root: &Path,
    _query: &str,
    _case_sensitive: bool,
    _max_results: usize,
) -> io::Result<String> {
    unsupported_platform()
}

#[cfg(not(target_os = "macos"))]
fn search_content(
    _workspace_root: &Path,
    _search_root: &Path,
    _query: &str,
    _case_sensitive: bool,
    _max_results: usize,
) -> io::Result<String> {
    unsupported_platform()
}

#[cfg(not(target_os = "macos"))]
fn unsupported_platform() -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "search skill currently supports macOS only; Linux and Windows are placeholders",
    ))
}

#[cfg(target_os = "macos")]
fn build_find_name_pattern(query: &str) -> String {
    format!("*{query}*")
}

#[cfg(target_os = "macos")]
fn ensure_success(command: &str, output: &std::process::Output) -> io::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{command} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(target_os = "macos")]
fn collect_find_paths(workspace_root: &Path, stdout: &[u8]) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let absolute = PathBuf::from(line);
        if absolute == workspace_root {
            continue;
        }
        let relative = absolute
            .strip_prefix(workspace_root)
            .map_err(|_| io::Error::other(format!("search result escaped work dir: {line}")))?;
        paths.push(relative.to_path_buf());
    }
    Ok(paths)
}

#[cfg(target_os = "macos")]
fn format_grep_match_line(workspace_root: &Path, line: &str) -> io::Result<Option<String>> {
    let Some((path_part, rest)) = line.split_once(':') else {
        return Ok(None);
    };
    let absolute = PathBuf::from(path_part);
    let relative = absolute
        .strip_prefix(workspace_root)
        .map_err(|_| io::Error::other(format!("search result escaped work dir: {line}")))?;
    Ok(Some(format!("{}:{}", relative.display(), rest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn search_root_stays_inside_workspace() {
        let workspace = unique_test_dir("search-root");
        fs::create_dir_all(workspace.join("docs")).unwrap();

        let resolved = resolve_search_root(&workspace, "../../docs").unwrap();

        assert_eq!(resolved, workspace.join("docs").canonicalize().unwrap());
    }

    #[test]
    fn search_root_rejects_missing_directory() {
        let workspace = unique_test_dir("search-missing");
        fs::create_dir_all(&workspace).unwrap();

        let err = resolve_search_root(&workspace, "missing").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn search_root_accepts_absolute_path_inside_workspace() {
        let workspace = unique_test_dir("search-absolute-inside");
        let docs = workspace.join("docs");
        fs::create_dir_all(&docs).unwrap();

        let resolved = resolve_search_root(&workspace, docs.to_str().unwrap()).unwrap();

        assert_eq!(resolved, docs.canonicalize().unwrap());
    }

    #[test]
    fn search_root_rejects_absolute_path_outside_workspace() {
        let workspace = unique_test_dir("search-absolute-outside-workspace");
        let outside = unique_test_dir("search-absolute-outside-target");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let err = resolve_search_root(&workspace, outside.to_str().unwrap()).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        std::env::temp_dir().join(format!("mylittlebotty-{label}-{millis}"))
    }
}
