use crate::botty_guy::{
    delegated_role_descriptions, delegated_role_exists, delegated_role_names, delegated_task_prompt,
};
use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::io;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::net::UnixStream;

const ASYNC_DELEGATION_PREFIX: &str = "__botty_async_delegate__";

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
            r#"{{"type":"object","properties":{{"role":{{"type":"string","description":"{role_desc}"}},"task":{{"type":"string","description":"Delegated task for the target role"}},"necessary_info":{{"type":"string","description":"Only the minimal context the target role needs. Do not include old chat history or summaries."}},"handoff_message":{{"type":"string","description":"Optional short natural-language message to send the user when this delegation starts. Keep it brief and conversational."}}}},"required":["role","task"]}}"#,
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
        let handoff_message = optional_string(&input, "handoff_message");
        run_delegated_guy(
            role,
            &delegated_task_prompt(role, task, necessary_info),
            handoff_message,
        )
    }
}

fn run_delegated_guy(
    role: &str,
    prompt: &str,
    handoff_message: Option<&str>,
) -> io::Result<String> {
    let parent_message_id = env::var("BOTTY_CURRENT_JOB_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::other("leader delegation missing BOTTY_CURRENT_JOB_ID"))?;
    let source = env::var("BOTTY_CURRENT_JOB_SOURCE").unwrap_or_else(|_| "leader".to_string());
    let user_id = env::var("BOTTY_CURRENT_JOB_USER_ID").unwrap_or_else(|_| "leader".to_string());

    let stream = UnixStream::connect(crate::botty_boss::chat_socket_path())?;
    let read_stream = stream.try_clone()?;
    let mut stdin = BufWriter::new(stream);
    let mut stdout = BufReader::new(read_stream);
    let control = serde_json::json!({
        "parent_message_id": parent_message_id,
        "role": role,
        "payload": prompt,
        "source": source,
        "user_id": user_id,
    });
    writeln!(
        stdin,
        "{}",
        encode_ipc_line(&format!("__botty_control__delegate|{}", control))?
    )?;
    stdin.flush()?;

    let mut response = String::new();
    let bytes = stdout.read_line(&mut response)?;
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "boss closed delegation socket before replying",
        ));
    }

    let decoded = decode_ipc_line(response.trim_end())?;
    let payload = serde_json::json!({
        "child_role": role,
        "child_message_id": decoded,
        "handoff_message": handoff_message,
    });
    Ok(format!("{ASYNC_DELEGATION_PREFIX}{payload}"))
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
