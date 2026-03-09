use crate::io as botty_io;
use crate::skill::BottySkill;
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const WRITE_TOOL_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "Target file path to write. The configured work dir is always used as root, so notes/today.md or /tmp/a.txt both become paths under that work dir"
    },
    "content": {
      "type": "string",
      "description": "Text content to write into the file"
    },
    "mode": {
      "type": "string",
      "enum": ["overwrite", "append"],
      "description": "overwrite replaces the file, append adds to the end"
    }
  },
  "required": ["path", "content"]
}"#;

pub struct BuildinWriteSkill;

impl BuildinWriteSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinWriteSkill {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write text files only under the configured Botty work dir. Create category subdirectories as needed when saving notes or records."
    }

    fn input_schema_json(&self) -> &'static str {
        WRITE_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let input: Value = serde_json::from_str(input_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse write tool input json failed: {err}"),
            )
        })?;

        let path = required_string(&input, "path")?;
        let content = required_string(&input, "content")?;
        let mode = optional_string(&input, "mode").unwrap_or("overwrite");

        let workspace_root = ensure_work_dir_root()?;
        let resolved = resolve_write_path(&workspace_root, path)?;

        if resolved.exists() && fs::metadata(&resolved)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "write skill only supports files, not directories: {}",
                    resolved.display()
                ),
            ));
        }

        let parent = resolved.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("write skill path has no parent: {}", resolved.display()),
            )
        })?;
        fs::create_dir_all(parent)?;

        match mode {
            "overwrite" => fs::write(&resolved, content)?,
            "append" => append_text(&resolved, content)?,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsupported write mode: {other}"),
                ));
            }
        }

        let action = if mode == "append" {
            "Appended"
        } else {
            "Wrote"
        };
        Ok(format!(
            "{action} {} bytes to {}",
            content.len(),
            resolved.display()
        ))
    }
}

fn required_string<'a>(input: &'a Value, key: &str) -> io::Result<&'a str> {
    input.get(key).and_then(Value::as_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("write tool input requires string field `{key}`"),
        )
    })
}

fn optional_string<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn append_text(path: &Path, content: &str) -> io::Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(content.as_bytes())
}

fn ensure_work_dir_root() -> io::Result<PathBuf> {
    let root = botty_io::effective_work_dir()?;
    fs::create_dir_all(&root)?;
    root.canonicalize()
}

fn resolve_write_path(workspace_root: &Path, raw_path: &str) -> io::Result<PathBuf> {
    let workspace_root = workspace_root.canonicalize()?;
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write tool requires a non-empty path",
        ));
    }

    let relative_target = sanitize_user_path(trimmed);
    let absolute = workspace_root.join(relative_target);
    let normalized = normalize_path(&absolute);

    if normalized.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "write skill path must point to a file: {}",
                normalized.display()
            ),
        ));
    }

    let safe = canonicalize_with_missing_segments(&normalized)?;
    if !safe.starts_with(&workspace_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "write skill can only write inside configured work dir: {}",
                workspace_root.display()
            ),
        ));
    }
    Ok(safe)
}

fn canonicalize_with_missing_segments(path: &Path) -> io::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();

    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid write path: {}", path.display()),
            )
        })?;
        missing.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid write path: {}", path.display()),
                )
            })?
            .to_path_buf();
    }

    let mut resolved = existing.canonicalize()?;
    for segment in missing.iter().rev() {
        resolved.push(segment);
    }
    Ok(resolved)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolve_relative_path_inside_workspace() {
        let workspace = unique_test_dir("write-root");
        fs::create_dir_all(&workspace).unwrap();

        let resolved = resolve_write_path(&workspace, "notes/hello.txt").unwrap();
        assert_eq!(
            resolved,
            workspace
                .canonicalize()
                .unwrap()
                .join("notes")
                .join("hello.txt")
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn reject_parent_escape_outside_workspace() {
        let workspace = unique_test_dir("write-root");
        fs::create_dir_all(&workspace).unwrap();

        let resolved = resolve_write_path(&workspace, "../escape.txt").unwrap();
        assert_eq!(
            resolved,
            workspace.canonicalize().unwrap().join("escape.txt")
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn absolute_path_is_forced_under_workspace_root() {
        let workspace = unique_test_dir("write-root");
        fs::create_dir_all(&workspace).unwrap();

        let resolved = resolve_write_path(&workspace, "/tmp/outside-write.txt").unwrap();
        assert_eq!(
            resolved,
            workspace
                .canonicalize()
                .unwrap()
                .join("tmp")
                .join("outside-write.txt")
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn absolute_path_uses_same_leaf_structure_as_input() {
        let workspace = unique_test_dir("write-root");
        fs::create_dir_all(&workspace).unwrap();

        let resolved = resolve_write_path(&workspace, "/tmp/hello/world.txt").unwrap();
        assert_eq!(
            resolved,
            workspace
                .canonicalize()
                .unwrap()
                .join("tmp")
                .join("hello")
                .join("world.txt")
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn symlink_parent_cannot_escape_workspace() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let workspace = unique_test_dir("write-root");
            let outside = unique_test_dir("write-outside");
            fs::create_dir_all(&workspace).unwrap();
            fs::create_dir_all(&outside).unwrap();
            symlink(&outside, workspace.join("link-out")).unwrap();

            let err = resolve_write_path(&workspace, "link-out/escape.txt").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

            fs::remove_dir_all(workspace).unwrap();
            fs::remove_dir_all(outside).unwrap();
        }
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{nanos}-{name}"))
    }
}
