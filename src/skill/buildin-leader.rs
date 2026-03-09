use crate::botty_guy::{
    delegated_role_descriptions, delegated_role_exists, delegated_role_names,
    delegated_task_prompt, BOTTY_GUY_ROLE_ENV,
};
use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::io;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Command, Stdio};

pub struct BuildinLeaderSkill;

impl BuildinLeaderSkill {
    pub fn new() -> Self {
        Self
    }

    fn build_schema_json() -> String {
        let descs = delegated_role_descriptions();
        let role_list: Vec<String> = descs
            .iter()
            .map(|(name, desc)| format!("{name}: {desc}"))
            .collect();
        let role_desc = format!(
            "Target Botty-Guy role. Available roles:\\n{}",
            role_list.join("\\n")
        );
        format!(
            r#"{{"type":"object","properties":{{"role":{{"type":"string","description":"{role_desc}"}},"task":{{"type":"string","description":"Delegated task for the target role"}},"necessary_info":{{"type":"string","description":"Only the minimal context the target role needs. Do not include old chat history or summaries."}}}},"required":["role","task"]}}"#,
        )
    }
}

impl BottySkill for BuildinLeaderSkill {
    fn name(&self) -> &'static str {
        "leader"
    }

    fn description(&self) -> &'static str {
        "Delegate the current task to a role-specific Botty-Guy with reduced context"
    }

    fn input_schema_json(&self) -> &'static str {
        // Leak dynamic schema so it has 'static lifetime — this is built once per leader skill instance
        Box::leak(Self::build_schema_json().into_boxed_str())
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let input: Value = serde_json::from_str(input_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse leader tool input json failed: {err}"),
            )
        })?;

        let role = required_string(&input, "role")?;
        if !delegated_role_exists(role) {
            let names = delegated_role_names();
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unsupported delegated role: {role}. available roles: {}",
                    names.join(", ")
                ),
            ));
        }

        let task = required_string(&input, "task")?;
        let necessary_info = optional_string(&input, "necessary_info").unwrap_or_default();
        run_delegated_guy(role, &delegated_task_prompt(role, task, necessary_info))
    }
}

fn run_delegated_guy(role: &str, prompt: &str) -> io::Result<String> {
    let exe = env::current_exe()?;
    let mut child = Command::new(exe)
        .arg("--guy")
        .env(BOTTY_GUY_ROLE_ENV, role)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut stdin = BufWriter::new(
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("failed to capture delegated guy stdin"))?,
    );
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture delegated guy stdout"))?,
    );

    writeln!(stdin, "{}", encode_ipc_line(prompt)?)?;
    stdin.flush()?;
    drop(stdin);

    let mut response = String::new();
    let bytes = stdout.read_line(&mut response)?;
    if bytes == 0 {
        let status = child.wait()?;
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("delegated guy exited before replying: {status}"),
        ));
    }

    let decoded = decode_ipc_line(response.trim_end())?;
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "delegated guy exited with status {status}"
        )));
    }

    parse_assistant_text(&decoded)
}

fn required_string<'a>(input: &'a Value, key: &str) -> io::Result<&'a str> {
    input.get(key).and_then(Value::as_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("leader tool input requires string field `{key}`"),
        )
    })
}

fn optional_string<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn encode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::to_string(value)
        .map_err(|err| io::Error::other(format!("encode ipc line failed: {err}")))
}

fn decode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::from_str(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decode ipc line failed: {err}"),
        )
    })
}

fn parse_assistant_text(decoded: &str) -> io::Result<String> {
    let value: Value = serde_json::from_str(decoded).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse delegated guy reply failed: {err}"),
        )
    })?;
    Ok(value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}
