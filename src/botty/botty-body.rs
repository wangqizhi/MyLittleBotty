use crate::botty_brain::BottyBrain;
use crate::botty_guy::{
    builtin_role_specs, expand_custom_role_skill_names, expand_role_skill_names,
    resolve_role_spec_or_custom, BottyGuyRoleSpec, CustomRoleConfig, ResolvedRole,
};
use crate::llm_provider::{
    ProviderMessage, ProviderResponse, ProviderToolDefinition, ProviderToolUse,
};
use crate::prompt;
use crate::skill::{build_skill, BottySkill};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEEP_MEMORY_CONTEXT_ROUNDS: usize = 10;
const REMEMBER_MAX_LINES: usize = 100;
const ROLE_EXPERIENCE_MAX_LINES: usize = 60;
const REMINDER_TRIGGER_COMMAND: &str = "/reminder-now";
const MAX_TOOL_CALL_STEPS: usize = 50;
const ASYNC_DELEGATION_PREFIX: &str = "__botty_async_delegate__";

pub struct AssistantReply {
    pub text: String,
    pub thinking: Option<String>,
    pub control: Option<AssistantControl>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantControl {
    AwaitDelegation {
        child_role: String,
        child_message_id: String,
        continuation_payload: String,
        handoff_message: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ContinuationState {
    messages: Vec<ProviderMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AsyncDelegationResult {
    child_role: String,
    child_message_id: String,
    handoff_message: Option<String>,
}

enum RoleInfo {
    Builtin(&'static BottyGuyRoleSpec),
    Custom(CustomRoleConfig),
}

pub struct BottyBody {
    brain: BottyBrain,
    role_info: RoleInfo,
    skills: Vec<Box<dyn BottySkill>>,
}

impl BottyBody {
    pub fn from_setup(role: &str) -> io::Result<Self> {
        let resolved = resolve_role_spec_or_custom(role).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown Botty-Guy role: {role}"),
            )
        })?;

        let (role_info, skills) = match resolved {
            ResolvedRole::Builtin(spec) => {
                let mut skills = Vec::new();
                for skill_name in expand_role_skill_names(spec) {
                    let skill = build_skill(skill_name).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("unknown skill in role `{}`: {skill_name}", spec.role),
                        )
                    })?;
                    skills.push(skill);
                }
                (RoleInfo::Builtin(spec), skills)
            }
            ResolvedRole::Custom(config) => {
                let mut skills = Vec::new();
                for skill_name in expand_custom_role_skill_names(&config) {
                    let skill = build_skill(&skill_name).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "unknown skill in custom role `{}`: {skill_name}",
                                config.name
                            ),
                        )
                    })?;
                    skills.push(skill);
                }
                (RoleInfo::Custom(config), skills)
            }
        };

        Ok(Self {
            brain: BottyBrain::from_setup()?,
            role_info,
            skills,
        })
    }

    pub fn think(&self, input: &str) -> io::Result<AssistantReply> {
        if let Some((name, argument)) = parse_debug_tool_call(input) {
            return Ok(AssistantReply {
                text: self.execute_tool(name, &argument)?,
                thinking: None,
                control: None,
            });
        }
        if let Some(interrupt_result) = self.try_handle_interrupt_shortcut(input)? {
            return Ok(AssistantReply {
                text: interrupt_result,
                thinking: None,
                control: None,
            });
        }
        if let Some(payload) = parse_special_command_argument(input, REMINDER_TRIGGER_COMMAND) {
            return Ok(AssistantReply {
                text: self.think_due_reminder(payload)?,
                thinking: None,
                control: None,
            });
        }
        if matches_special_command(input, "/remember") {
            return Ok(AssistantReply {
                text: self.remember_deep_memory()?,
                thinking: None,
                control: None,
            });
        }

        let tools = self.tool_definitions();
        let system_prompt = self.build_system_prompt()?;
        let conversation = [ProviderMessage::UserText(input.to_string())];
        let first_response = self.brain.think(&system_prompt, &conversation, &tools)?;

        match first_response {
            ProviderResponse::Text(reply) => Ok(AssistantReply {
                text: reply.text,
                thinking: reply.thinking,
                control: None,
            }),
            ProviderResponse::ToolUse(tool_use) => self.complete_tool_call(input, &tools, tool_use),
        }
    }

    pub fn resume_tool_call(
        &self,
        continuation_payload: &str,
        tool_result: &str,
    ) -> io::Result<AssistantReply> {
        let state: ContinuationState =
            serde_json::from_str(continuation_payload).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("parse continuation payload failed: {err}"),
                )
            })?;
        let tools = self.tool_definitions();
        let system_prompt = self.build_system_prompt()?;
        let conversation = state.messages;
        self.continue_tool_call(
            &system_prompt,
            &tools,
            conversation,
            tool_result.to_string(),
        )
    }

    fn complete_tool_call(
        &self,
        input: &str,
        tools: &[ProviderToolDefinition],
        first_tool_use: ProviderToolUse,
    ) -> io::Result<AssistantReply> {
        let system_prompt = self.build_system_prompt()?;
        let conversation = vec![ProviderMessage::UserText(input.to_string())];
        self.run_tool_call_loop(&system_prompt, tools, conversation, first_tool_use)
    }

    fn continue_tool_call(
        &self,
        system_prompt: &str,
        tools: &[ProviderToolDefinition],
        mut conversation: Vec<ProviderMessage>,
        tool_result: String,
    ) -> io::Result<AssistantReply> {
        let Some(ProviderMessage::AssistantToolUse {
            assistant_content_json,
        }) = conversation.pop()
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "continuation payload missing trailing assistant tool use",
            ));
        };
        let tool_use_id = extract_tool_use_id(&assistant_content_json)?;
        conversation.push(ProviderMessage::AssistantToolUse {
            assistant_content_json,
        });
        conversation.push(ProviderMessage::UserToolResult {
            tool_use_id,
            content: tool_result,
        });

        match self.brain.think(system_prompt, &conversation, tools)? {
            ProviderResponse::Text(reply) => Ok(AssistantReply {
                text: reply.text,
                thinking: reply.thinking,
                control: None,
            }),
            ProviderResponse::ToolUse(tool_use) => {
                self.run_tool_call_loop(system_prompt, tools, conversation, tool_use)
            }
        }
    }

    fn run_tool_call_loop(
        &self,
        system_prompt: &str,
        tools: &[ProviderToolDefinition],
        mut conversation: Vec<ProviderMessage>,
        mut tool_use: ProviderToolUse,
    ) -> io::Result<AssistantReply> {
        for _ in 0..MAX_TOOL_CALL_STEPS {
            let tool_result =
                self.execute_tool(tool_use.name.as_str(), tool_use.input_json.as_str())?;
            conversation.push(ProviderMessage::AssistantToolUse {
                assistant_content_json: tool_use.assistant_content_json,
            });
            if let Some(async_delegation) = parse_async_delegation_result(&tool_result)? {
                let continuation_payload = serde_json::to_string(&ContinuationState {
                    messages: conversation.clone(),
                })
                .map_err(|err| io::Error::other(format!("serialize continuation failed: {err}")))?;
                return Ok(AssistantReply {
                    text: String::new(),
                    thinking: None,
                    control: Some(AssistantControl::AwaitDelegation {
                        child_role: async_delegation.child_role,
                        child_message_id: async_delegation.child_message_id,
                        continuation_payload,
                        handoff_message: async_delegation.handoff_message,
                    }),
                });
            }
            conversation.push(ProviderMessage::UserToolResult {
                tool_use_id: tool_use.id,
                content: tool_result,
            });

            match self.brain.think(system_prompt, &conversation, tools)? {
                ProviderResponse::Text(reply) => {
                    return Ok(AssistantReply {
                        text: reply.text,
                        thinking: reply.thinking,
                        control: None,
                    });
                }
                ProviderResponse::ToolUse(next_tool_use) => {
                    tool_use = next_tool_use;
                }
            }
        }

        Err(io::Error::other(format!(
            "llm exceeded maximum chained tool calls ({MAX_TOOL_CALL_STEPS})"
        )))
    }

    fn execute_tool(&self, name: &str, argument: &str) -> io::Result<String> {
        for skill in &self.skills {
            if skill.name() == name {
                return skill.execute(argument);
            }
        }

        let available = self
            .skills
            .iter()
            .map(|skill| skill.name().to_string())
            .collect::<Vec<_>>();
        Ok(format!(
            "Tool `{name}` is not available in this role. Available tools: {}. Use one of those tools instead.",
            available.join(", ")
        ))
    }

    fn build_system_prompt(&self) -> io::Result<String> {
        match &self.role_info {
            RoleInfo::Builtin(spec) => {
                build_system_prompt_for_role(spec, DEEP_MEMORY_CONTEXT_ROUNDS)
            }
            RoleInfo::Custom(config) => {
                build_system_prompt_for_custom_role(config, DEEP_MEMORY_CONTEXT_ROUNDS)
            }
        }
    }

    fn tool_definitions(&self) -> Vec<ProviderToolDefinition> {
        self.skills
            .iter()
            .map(|skill| ProviderToolDefinition {
                name: skill.name(),
                description: skill.description(),
                input_schema_json: skill.input_schema_json(),
            })
            .collect()
    }

    fn current_role_name(&self) -> &str {
        match &self.role_info {
            RoleInfo::Builtin(spec) => spec.role,
            RoleInfo::Custom(config) => &config.name,
        }
    }

    fn try_handle_interrupt_shortcut(&self, input: &str) -> io::Result<Option<String>> {
        if self.current_role_name() != "leader" {
            return Ok(None);
        }
        let Some(argument) = parse_leader_interrupt_shortcut(input) else {
            return Ok(None);
        };
        Ok(Some(self.execute_tool("leader", &argument)?))
    }

    fn remember_deep_memory(&self) -> io::Result<String> {
        let summary_dir = botty_root_dir().join("memory").join("summary");
        fs::create_dir_all(&summary_dir)?;
        fs::create_dir_all(summary_dir.join("experience"))?;

        let remember_path = summary_dir.join("remember.md");
        let rec_time_path = summary_dir.join("rec.time");
        let existing_summary = read_trimmed_file(&remember_path)?;
        let rec_time = read_trimmed_file(&rec_time_path)?;

        let entries = load_deep_memory_entries()?;
        let entries = filter_entries_after_rec_time(entries, rec_time.as_deref());
        let entries = entries
            .into_iter()
            .filter(|entry| !is_control_memory_entry(entry))
            .collect::<Vec<_>>();

        if entries.is_empty() {
            return Ok("No new deep memory to remember.".to_string());
        }

        let latest_timestamp = entries
            .last()
            .map(|entry| entry.timestamp.clone())
            .ok_or_else(|| io::Error::other("remember input is unexpectedly empty"))?;
        let transcript = format_memory_transcript(&entries);
        let mut summary = extract_remember_text(
            &self.generate_remember_summary(existing_summary.as_deref(), &transcript)?,
        );
        if line_count(&summary) > REMEMBER_MAX_LINES {
            summary = extract_remember_text(&self.compress_remember_summary(&summary)?);
        }
        summary = trim_to_max_lines(&summary, REMEMBER_MAX_LINES);

        fs::write(&remember_path, ensure_trailing_newline(&summary))?;
        self.update_role_experience_memories(existing_summary.as_deref(), &transcript)?;
        fs::write(&rec_time_path, format!("{latest_timestamp}\n"))?;

        Ok("I'have remembered".to_string())
    }

    fn update_role_experience_memories(
        &self,
        remember_summary: Option<&str>,
        transcript: &str,
    ) -> io::Result<()> {
        for spec in builtin_role_specs() {
            let Some(rule) = spec.experience_memory_rule else {
                continue;
            };

            let path = role_experience_summary_path(spec.role);
            let existing_summary = read_trimmed_file(&path)?;
            let mut summary = extract_remember_text(&self.generate_role_experience_summary(
                spec.role,
                rule,
                remember_summary,
                existing_summary.as_deref(),
                transcript,
            )?);

            if line_count(&summary) > ROLE_EXPERIENCE_MAX_LINES {
                summary = extract_remember_text(
                    &self.compress_role_experience_summary(spec.role, rule, &summary)?,
                );
            }

            summary = trim_to_max_lines(&summary, ROLE_EXPERIENCE_MAX_LINES);
            fs::write(path, ensure_trailing_newline(&summary))?;
        }
        Ok(())
    }

    fn generate_remember_summary(
        &self,
        existing_summary: Option<&str>,
        transcript: &str,
    ) -> io::Result<String> {
        let output_instruction = "Write `remember.md` in Markdown.\nKeep it under 100 lines.\nKeep only key events and the user's recent important requests plus solution status.";
        let user_prompt = match existing_summary.filter(|text| !text.trim().is_empty()) {
            Some(summary) => prompt::render(
                prompt::REMEMBER_UPDATE_PROMPT,
                &[
                    ("summary", summary),
                    ("transcript", transcript),
                    ("output_instruction", output_instruction),
                ],
            ),
            None => prompt::render(
                prompt::REMEMBER_INIT_PROMPT,
                &[
                    ("transcript", transcript),
                    ("output_instruction", output_instruction),
                ],
            ),
        };
        self.run_summary_prompt(&user_prompt)
    }

    fn compress_remember_summary(&self, summary: &str) -> io::Result<String> {
        let output_instruction = "Write `remember.md` in Markdown.";
        let user_prompt = prompt::render(
            prompt::REMEMBER_COMPRESS_PROMPT,
            &[
                ("summary", summary),
                ("output_instruction", output_instruction),
            ],
        );
        self.run_summary_prompt(&user_prompt)
    }

    fn generate_role_experience_summary(
        &self,
        role_name: &str,
        rule: &str,
        remember_summary: Option<&str>,
        existing_summary: Option<&str>,
        transcript: &str,
    ) -> io::Result<String> {
        let remember_reference = remember_summary
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("(empty)");
        let role_specific_instruction = format!(
            "Target role: `{role_name}`.\nRole memory rule:\n{rule}\nCurrent global remember.md for reference:\n```md\n{remember_reference}\n```\nWrite `{role_name}-exp.md` in Markdown.\nKeep only durable, reusable facts that help this role in future tasks.\nPrefer concise field-style records.\nUse both the new transcript and the current remember.md as evidence.\nIf the transcript adds nothing new, you may still keep or refine useful facts already supported by remember.md.\nIf neither transcript nor remember.md contains useful facts for this role, keep the existing content or return an empty file."
        );
        let user_prompt = match existing_summary.filter(|text| !text.trim().is_empty()) {
            Some(summary) => prompt::render(
                prompt::REMEMBER_UPDATE_PROMPT,
                &[
                    ("summary", summary),
                    ("transcript", transcript),
                    ("output_instruction", &role_specific_instruction),
                ],
            ),
            None => prompt::render(
                prompt::REMEMBER_INIT_PROMPT,
                &[
                    ("transcript", transcript),
                    ("output_instruction", &role_specific_instruction),
                ],
            ),
        };
        self.run_summary_prompt(&user_prompt)
    }

    fn compress_role_experience_summary(
        &self,
        role_name: &str,
        rule: &str,
        summary: &str,
    ) -> io::Result<String> {
        let output_instruction = format!(
            "Rewrite `{role_name}-exp.md` in Markdown.\nRole memory rule:\n{rule}\nKeep only durable, reusable facts for this role.\nPrefer concise field-style records."
        );
        let user_prompt = prompt::render(
            prompt::REMEMBER_COMPRESS_PROMPT,
            &[
                ("summary", summary),
                ("output_instruction", &output_instruction),
            ],
        );
        self.run_summary_prompt(&user_prompt)
    }

    fn run_summary_prompt(&self, user_prompt: &str) -> io::Result<String> {
        let response = self.brain.think(
            prompt::REMEMBER_SYSTEM_PROMPT,
            &[ProviderMessage::UserText(user_prompt.to_string())],
            &[],
        )?;
        match response {
            ProviderResponse::Text(reply) => Ok(reply.text.trim().to_string()),
            ProviderResponse::ToolUse(_) => Err(io::Error::other(
                "remember summary unexpectedly returned a tool call",
            )),
        }
    }

    fn think_due_reminder(&self, payload: &str) -> io::Result<String> {
        let payload: Value = serde_json::from_str(payload).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse reminder trigger payload failed: {err}"),
            )
        })?;
        let original_request = payload
            .get("original_request")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scheduled_at = payload
            .get("scheduled_at")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let current_time = payload
            .get("current_time")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let user_prompt = prompt::render(
            prompt::REMINDER_USER_PROMPT,
            &[
                ("original_request", original_request),
                ("scheduled_at", scheduled_at),
                ("current_time", current_time),
            ],
        );
        match self.brain.think(
            prompt::REMINDER_SYSTEM_PROMPT,
            &[ProviderMessage::UserText(user_prompt)],
            &[],
        )? {
            ProviderResponse::Text(reply) => Ok(reply.text),
            ProviderResponse::ToolUse(_) => Err(io::Error::other(
                "reminder trigger unexpectedly returned a tool call",
            )),
        }
    }
}

fn parse_debug_tool_call(input: &str) -> Option<(&str, String)> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix("/test ")?;
    let (name, argument) = rest.split_once(' ')?;
    let argument = argument.trim();
    if argument.is_empty() {
        return None;
    }
    Some((name.trim(), argument.to_string()))
}

fn parse_async_delegation_result(value: &str) -> io::Result<Option<AsyncDelegationResult>> {
    let payload = match value.strip_prefix(ASYNC_DELEGATION_PREFIX) {
        Some(payload) => payload,
        None => return Ok(None),
    };
    let parsed = serde_json::from_str(payload).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse async delegation payload failed: {err}"),
        )
    })?;
    Ok(Some(parsed))
}

fn extract_tool_use_id(assistant_content_json: &str) -> io::Result<String> {
    let value: Value = serde_json::from_str(assistant_content_json).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse assistant tool use content failed: {err}"),
        )
    })?;

    if let Some(id) = value.get("id").and_then(Value::as_str) {
        return Ok(id.to_string());
    }
    if let Some(content) = value.as_array() {
        for item in content {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                return Ok(id.to_string());
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "assistant tool use content missing id",
    ))
}

fn matches_special_command(input: &str, command: &str) -> bool {
    extract_special_command(input) == Some(command)
}

fn parse_special_command_argument<'a>(input: &'a str, command: &str) -> Option<&'a str> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix(command) {
        return Some(rest.trim());
    }

    let (_, rest) = trimmed.split_once(": ")?;
    let rest = rest.trim();
    let rest = rest.strip_prefix(command)?;
    Some(rest.trim())
}

fn extract_special_command(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.starts_with('/') {
        return Some(trimmed);
    }

    let (_, rest) = trimmed.split_once(": ")?;
    let rest = rest.trim();
    if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

#[derive(Clone)]
struct DeepMemoryEntry {
    timestamp: String,
    role: DeepMemoryRole,
    message: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeepMemoryRole {
    User,
    Assistant,
}

fn build_system_prompt_for_role(role_spec: &BottyGuyRoleSpec, rounds: usize) -> io::Result<String> {
    let remember = if role_spec.include_memory_context {
        load_remember_summary()?
    } else {
        String::new()
    };
    let memory = if role_spec.include_memory_context {
        load_recent_deep_memory_transcript(rounds)?
    } else {
        String::new()
    };
    let current_local_time = local_time_string()?;
    let remember_section = if remember.is_empty() {
        String::new()
    } else {
        format!("\n\nLong-term memory summary from memory/summary/remember.md:\n{remember}")
    };
    let memory_section = if memory.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRecent conversation history from memory/deep (latest {rounds} rounds):\n{memory}"
        )
    };
    let role_experience = load_role_experience_summary(role_spec.role)?;
    let role_experience_section = if role_experience.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRole-specific memory from memory/summary/experience/{}-exp.md:\n{}",
            role_spec.role, role_experience
        )
    };
    Ok(prompt::render(
        prompt::TOOL_SYSTEM_PROMPT,
        &[
            ("role_name", role_spec.role),
            ("role_description", role_spec.description),
            ("role_instruction", role_spec.system_instruction_prompt),
            (
                "tool_usage_guidance",
                &render_tool_usage_guidance(
                    &expand_role_skill_names(role_spec)
                        .into_iter()
                        .map(|name| name.to_string())
                        .collect::<Vec<_>>(),
                ),
            ),
            ("remember_section", &remember_section),
            ("memory_section", &memory_section),
            ("role_experience_section", &role_experience_section),
            ("current_local_time", &current_local_time),
        ],
    ))
}

fn build_system_prompt_for_custom_role(
    config: &CustomRoleConfig,
    _rounds: usize,
) -> io::Result<String> {
    let current_local_time = local_time_string()?;
    let instruction = format!(
        "You are a custom sub-agent named `{}`. {}\nHandle the delegated task using your bound skills.",
        config.name, config.description
    );
    Ok(prompt::render(
        prompt::TOOL_SYSTEM_PROMPT,
        &[
            ("role_name", &config.name),
            ("role_description", &config.description),
            ("role_instruction", &instruction),
            (
                "tool_usage_guidance",
                &render_tool_usage_guidance(&expand_custom_role_skill_names(config)),
            ),
            ("remember_section", ""),
            ("memory_section", ""),
            ("role_experience_section", ""),
            ("current_local_time", &current_local_time),
        ],
    ))
}

fn render_tool_usage_guidance(skills: &[String]) -> String {
    if skills.is_empty() {
        return "No tools are available in this role.".to_string();
    }

    let mut lines = vec![format!(
        "Only these tools are available in this role: {}.",
        skills.join(", ")
    )];

    if skills.iter().any(|skill| skill == "terminal") {
        lines.push("Use `terminal` for coding and repository execution through the terminal app. For a new delegated coding task, prefer a single `execute_task` call so the tool can manage the PTY session, polling, transcript parsing, and completion detection internally.".to_string());
        lines.push("Use `continue_session`, `status`, `transcript`, `interrupt`, or `terminate` only when you truly need direct session control. If there is no blocker and no missing requirement, keep driving the coding session yourself. Do not hand work back to leader just because the task is long-running.".to_string());
    }
    if skills.iter().any(|skill| skill == "list") {
        lines.push("Use `list` when the user asks to list directory contents.".to_string());
    }
    if skills.iter().any(|skill| skill == "watch") {
        lines.push(
            "Use `watch` when the user asks to inspect, open, read, or show a file.".to_string(),
        );
    }
    if skills.iter().any(|skill| skill == "write") {
        lines.push("Use `write` when the user asks to save, write, note, record, or persist text into a local file. Preserve the user's filename intent, but remember that write always remaps paths under the configured work dir.".to_string());
    }
    if skills.iter().any(|skill| skill == "crond") {
        lines.push("Use `crond` only for reminder and scheduling requests. For create or edit actions, always provide exact local `schedule_at` in `YYYY-MM-DD HH:MM:SS`.".to_string());
    }
    if skills.iter().any(|skill| skill == "remember") {
        lines.push("Use `remember` only when the current topic may depend on older conversation memory not already covered by the provided summaries.".to_string());
    }
    if skills.iter().any(|skill| skill == "leader") {
        lines.push("Use `leader` to delegate to a specialized role when this role is responsible for routing.".to_string());
        lines.push("If the user asks to stop, cancel, abort, interrupt, terminate, 取消, 中断, 停止, or 终止 a delegated task, you must call `leader` with `action=\"interrupt\"`. Do not claim the task was interrupted unless that tool call has actually succeeded.".to_string());
    }

    lines.join(" ")
}

fn parse_leader_interrupt_shortcut(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !looks_like_interrupt_request(trimmed) {
        return None;
    }

    let lowered = trimmed.to_ascii_lowercase();
    let role = ["coder", "info-searcher", "paperwork", "all-in-one"]
        .into_iter()
        .find(|candidate| lowered.contains(candidate))
        .map(str::to_string);

    let payload = if let Some(role) = role {
        serde_json::json!({
            "action": "interrupt",
            "role": role,
        })
    } else {
        serde_json::json!({
            "action": "interrupt",
        })
    };
    Some(payload.to_string())
}

fn looks_like_interrupt_request(input: &str) -> bool {
    let lowered = input.to_ascii_lowercase();
    let english_hit = ["interrupt", "cancel", "abort", "stop", "terminate", "pause"]
        .into_iter()
        .any(|keyword| lowered.contains(keyword));
    let chinese_hit = ["中断", "取消", "停止", "终止", "停一下", "停下", "打断"]
        .into_iter()
        .any(|keyword| input.contains(keyword));

    english_hit || chinese_hit
}

fn load_recent_deep_memory_transcript(rounds: usize) -> io::Result<String> {
    if rounds == 0 {
        return Ok(String::new());
    }

    let marker = read_trimmed_file(&new_session_marker_path())?;
    let mut entries = load_deep_memory_entries()?;
    if let Some(marker) = marker.as_deref() {
        entries.retain(|entry| entry.timestamp.as_str() > marker);
    }
    entries.retain(|entry| !is_control_memory_entry(entry));
    if entries.is_empty() {
        return Ok(String::new());
    }

    entries.reverse();
    let mut collected = Vec::new();
    let mut user_count = 0usize;

    for entry in entries {
        if entry.role == DeepMemoryRole::User {
            user_count += 1;
        }
        collected.push(entry);
        if user_count >= rounds {
            break;
        }
    }

    collected.reverse();

    Ok(collected
        .into_iter()
        .map(|entry| format_deep_memory_message(&entry))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn load_deep_memory_entries() -> io::Result<Vec<DeepMemoryEntry>> {
    let root = botty_root_dir().join("memory").join("deep");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_deep_memory_files(&root, &mut files)?;
    files.sort_by_key(|path| deep_memory_file_sort_key(path));

    let mut entries = Vec::new();
    for file in files {
        let content = fs::read_to_string(file)?;
        for line in content.lines() {
            if let Some(entry) = parse_deep_memory_entry(line) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

fn collect_deep_memory_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_deep_memory_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn deep_memory_file_sort_key(path: &Path) -> (u32, u32, u32, String) {
    let year = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    let (month_day, index) = path
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(parse_deep_memory_file_stem)
        .unwrap_or((0, 0));

    (year, month_day, index, path.to_string_lossy().into_owned())
}

fn parse_deep_memory_file_stem(stem: &str) -> Option<(u32, u32)> {
    let (month_day, index) = stem.split_once('-')?;
    Some((month_day.parse().ok()?, index.parse().ok()?))
}

fn parse_deep_memory_entry(line: &str) -> Option<DeepMemoryEntry> {
    let (timestamp, rest) = parse_deep_memory_timestamp(line)?;
    if let Some((_, message)) = rest.split_once(" assistant: ") {
        return Some(DeepMemoryEntry {
            timestamp,
            role: DeepMemoryRole::Assistant,
            message: restore_deep_memory_message(message),
        });
    }
    if let Some((_, message)) = rest.split_once(" user: ") {
        return Some(DeepMemoryEntry {
            timestamp,
            role: DeepMemoryRole::User,
            message: restore_deep_memory_message(message),
        });
    }
    None
}

fn restore_deep_memory_message(message: &str) -> String {
    message.replace("\\n", "\n").replace("\\r", "\r")
}

fn format_deep_memory_message(entry: &DeepMemoryEntry) -> String {
    match entry.role {
        DeepMemoryRole::User => format!("user: {}", entry.message),
        DeepMemoryRole::Assistant => format!("assistant: {}", entry.message),
    }
}

fn parse_deep_memory_timestamp(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once("] ")?;
    Some((timestamp.to_string(), rest))
}

fn new_session_marker_path() -> PathBuf {
    botty_root_dir()
        .join("memory")
        .join("summary")
        .join("new.time")
}

fn remember_summary_path() -> PathBuf {
    botty_root_dir()
        .join("memory")
        .join("summary")
        .join("remember.md")
}

fn role_experience_summary_path(role: &str) -> PathBuf {
    botty_root_dir()
        .join("memory")
        .join("summary")
        .join("experience")
        .join(format!("{role}-exp.md"))
}

fn load_remember_summary() -> io::Result<String> {
    Ok(read_trimmed_file(&remember_summary_path())?.unwrap_or_default())
}

fn load_role_experience_summary(role: &str) -> io::Result<String> {
    Ok(read_trimmed_file(&role_experience_summary_path(role))?.unwrap_or_default())
}

fn read_trimmed_file(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn filter_entries_after_rec_time(
    entries: Vec<DeepMemoryEntry>,
    rec_time: Option<&str>,
) -> Vec<DeepMemoryEntry> {
    match rec_time {
        Some(rec_time) => entries
            .into_iter()
            .filter(|entry| entry.timestamp.as_str() > rec_time)
            .collect(),
        None => entries,
    }
}

fn is_control_memory_entry(entry: &DeepMemoryEntry) -> bool {
    matches!(entry.role, DeepMemoryRole::User)
        && matches!(entry.message.trim(), "/new" | "/remember")
}

fn format_memory_transcript(entries: &[DeepMemoryEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "[{}] {}",
                entry.timestamp,
                format_deep_memory_message(entry)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

fn trim_to_max_lines(text: &str, max_lines: usize) -> String {
    text.lines().take(max_lines).collect::<Vec<_>>().join("\n")
}

fn extract_remember_text(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(body) = trimmed.strip_prefix("```md\n") {
        return body
            .strip_suffix("\n```")
            .unwrap_or(body)
            .trim()
            .to_string();
    }
    if let Some(body) = trimmed.strip_prefix("```\n") {
        return body
            .strip_suffix("\n```")
            .unwrap_or(body)
            .trim()
            .to_string();
    }
    trimmed.to_string()
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn local_time_string() -> io::Result<String> {
    let output = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::{looks_like_interrupt_request, parse_leader_interrupt_shortcut};

    #[test]
    fn detects_interrupt_requests_in_chinese() {
        assert!(looks_like_interrupt_request("中断一下这个任务"));
        assert!(looks_like_interrupt_request("帮我取消当前任务"));
    }

    #[test]
    fn detects_interrupt_requests_in_english() {
        assert!(looks_like_interrupt_request("please stop the current task"));
        assert!(looks_like_interrupt_request("interrupt coder"));
    }

    #[test]
    fn ignores_non_interrupt_requests() {
        assert!(!looks_like_interrupt_request("继续这个任务"));
        assert!(parse_leader_interrupt_shortcut("继续这个任务").is_none());
    }

    #[test]
    fn builds_interrupt_payload_with_role_filter_when_present() {
        let payload =
            parse_leader_interrupt_shortcut("interrupt coder now").expect("payload expected");
        assert!(payload.contains(r#""action":"interrupt""#));
        assert!(payload.contains(r#""role":"coder""#));
    }
}
