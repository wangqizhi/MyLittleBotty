use crate::io as botty_io;
use crate::skill::BottySkill;
use serde_json::Value;
use std::fs;
use std::io;
use std::path::PathBuf;

/// A user-defined custom skill loaded from ~/.mylittlebotty/skill/<name>.json
///
/// JSON format:
/// {
///   "name": "my-skill",
///   "description": "What this skill does",
///   "usage": "How to use this skill",
///   "input_schema": { "type": "object", "properties": { ... }, "required": [...] },
///   "action": "prompt",
///   "prompt_template": "Based on the following input: {{input}}, do ..."
/// }
///
/// action types:
///   - "prompt": sends input to the LLM with the prompt_template as context
///   - "script": runs a shell script under ~/.mylittlebotty/skill/scripts/<name>.sh

pub struct CustomSkill {
    pub skill_name: String,
    pub skill_description: String,
    pub input_schema_json: String,
    pub action: String,
    pub prompt_template: String,
}

impl CustomSkill {
    pub fn load_from_file(path: &PathBuf) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let value: Value = serde_json::from_str(&content).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse custom skill file failed: {err}"),
            )
        })?;

        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("custom skill file missing 'name' field: {}", path.display()),
            ));
        }

        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let _usage = value.get("usage").and_then(Value::as_str).unwrap_or("");

        let input_schema = value.get("input_schema").cloned().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "The input for this skill"
                    }
                },
                "required": ["input"]
            })
        });
        let input_schema_json = serde_json::to_string(&input_schema).map_err(|err| {
            io::Error::other(format!("serialize custom skill input_schema failed: {err}"))
        })?;

        let action = value
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("prompt")
            .to_string();
        let prompt_template = value
            .get("prompt_template")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        Ok(Self {
            skill_name: name,
            skill_description: description,
            input_schema_json,
            action,
            prompt_template,
        })
    }
}

impl BottySkill for CustomSkill {
    fn name(&self) -> &'static str {
        // Leak the string so it lives for 'static — acceptable for a small number of custom skills
        Box::leak(self.skill_name.clone().into_boxed_str())
    }

    fn description(&self) -> &'static str {
        Box::leak(self.skill_description.clone().into_boxed_str())
    }

    fn input_schema_json(&self) -> &'static str {
        Box::leak(self.input_schema_json.clone().into_boxed_str())
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        match self.action.as_str() {
            "prompt" => execute_prompt_skill(&self.prompt_template, input_json),
            "script" => execute_script_skill(&self.skill_name, input_json),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported custom skill action: {}", self.action),
            )),
        }
    }
}

fn execute_prompt_skill(prompt_template: &str, input_json: &str) -> io::Result<String> {
    let input: Value = serde_json::from_str(input_json).unwrap_or(Value::Null);
    let input_text = input
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or(input_json);

    let rendered = prompt_template.replace("{{input}}", input_text);
    Ok(format!(
        "[Custom skill prompt-based execution]\n\nPrompt:\n{rendered}\n\nInput:\n{input_text}"
    ))
}

fn execute_script_skill(skill_name: &str, input_json: &str) -> io::Result<String> {
    let script_dir = custom_skill_dir().join("scripts");
    let script_path = script_dir.join(format!("{skill_name}.sh"));

    if !script_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("custom skill script not found: {}", script_path.display()),
        ));
    }

    let output = std::process::Command::new("sh")
        .arg(&script_path)
        .env("SKILL_INPUT", input_json)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "custom skill script failed: {stderr}"
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn custom_skill_dir() -> PathBuf {
    botty_io::config_root_dir().join("skill")
}

pub fn load_all_custom_skills() -> Vec<CustomSkill> {
    let dir = custom_skill_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut skills = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        match CustomSkill::load_from_file(&path) {
            Ok(skill) => skills.push(skill),
            Err(err) => eprintln!(
                "Warning: failed to load custom skill {}: {err}",
                path.display()
            ),
        }
    }
    skills
}

pub fn save_custom_skill(
    name: &str,
    description: &str,
    usage: &str,
    action: &str,
    prompt_template: &str,
) -> io::Result<PathBuf> {
    let dir = custom_skill_dir();
    fs::create_dir_all(&dir)?;

    let file_path = dir.join(format!("{name}.json"));

    let value = serde_json::json!({
        "name": name,
        "description": description,
        "usage": usage,
        "input_schema": {
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The input for this skill"
                }
            },
            "required": ["input"]
        },
        "action": action,
        "prompt_template": prompt_template
    });

    let content = serde_json::to_string_pretty(&value)
        .map_err(|err| io::Error::other(format!("serialize custom skill failed: {err}")))?;
    fs::write(&file_path, content)?;
    Ok(file_path)
}
