use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub mod input;
pub mod output;
pub mod transport;

pub fn config_root_dir() -> PathBuf {
    home_dir().join(".mylittlebotty")
}

pub fn shared_work_dir_config_file() -> PathBuf {
    config_root_dir().join("config").join("work-dir.conf")
}

pub fn default_work_dir() -> PathBuf {
    home_dir().join("opt").join("mylittlebotty-workdir")
}

pub fn default_work_dir_display() -> String {
    "~/opt/mylittlebotty-workdir".to_string()
}

pub fn load_work_dir_setting() -> io::Result<String> {
    let path = shared_work_dir_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(default_work_dir_display());
        }
        Err(err) => return Err(err),
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() == "work.dir" {
            let value = value.trim();
            if value.is_empty() {
                return Ok(default_work_dir_display());
            }
            return Ok(value.to_string());
        }
    }

    Ok(default_work_dir_display())
}

pub fn save_work_dir_setting(raw_value: &str) -> io::Result<PathBuf> {
    let path = shared_work_dir_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let value = normalize_work_dir_input(raw_value);
    fs::write(&path, format!("work.dir={value}\n"))?;
    Ok(path)
}

pub fn effective_work_dir() -> io::Result<PathBuf> {
    Ok(resolve_work_dir_input(&load_work_dir_setting()?))
}

pub fn resolve_work_dir_input(raw_value: &str) -> PathBuf {
    let value = normalize_work_dir_input(raw_value);
    if value == default_work_dir_display() {
        return default_work_dir();
    }
    expand_user_path(&value)
}

pub fn normalize_work_dir_input(raw_value: &str) -> String {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return default_work_dir_display();
    }
    trimmed.to_string()
}

pub fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir();
    }

    if let Some(rest) = value.strip_prefix("~/") {
        return home_dir().join(rest);
    }

    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        home_dir().join(path)
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
