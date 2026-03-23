use crate::botty_body::{AssistantReply, BottyBody};
use crate::botty_jobs::{self, JobState};
use crate::infra::chatbot_feishu::{
    FeishuClient, FeishuLongConnClient, DEFAULT_API_BASE as FEISHU_API_BASE,
};
use crate::infra::chatbot_telegram::{TelegramClient, DEFAULT_API_BASE as TELEGRAM_API_BASE};
use crate::infra::chatbot_weixin::{
    WeixinClient, DEFAULT_API_BASE as WEIXIN_API_BASE, SESSION_EXPIRED_ERRCODE,
};
use crate::prompt;
use serde_json::{self, json, Value};
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::ffi::CString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub(crate) const BOTTY_GUY_ROLE_ENV: &str = "BOTTY_GUY_ROLE";
const BOTTY_GUY_DEFAULT_ROLE: &str = "leader";
const TELEGRAM_POLL_INTERVAL_SECONDS_DEFAULT: u64 = 1;
const FEISHU_POLL_INTERVAL_SECONDS_DEFAULT: u64 = 1;
const WEIXIN_POLL_INTERVAL_SECONDS_DEFAULT: u64 = 1;
const WEIXIN_LONG_POLL_TIMEOUT_MS_DEFAULT: u64 = 35_000;
const FEISHU_SEEN_CACHE_LIMIT: usize = 200;
const CHAT_MEMORY_MAX_BYTES: u64 = 200 * 1024;
const CHAT_META_PREFIX: &str = "__botty_meta__";
const CONTROL_PREFIX: &str = "__botty_control__";
const ATTACHMENT_PREFIX: &str = "__botty_attachment__";

#[derive(Clone, Copy)]
pub(crate) struct BottyGuyRoleSpec {
    pub role: &'static str,
    pub description: &'static str,
    pub system_instruction_prompt: &'static str,
    pub skill_groups: &'static [&'static str],
    pub skills: &'static [&'static str],
    pub include_memory_context: bool,
    pub experience_memory_rule: Option<&'static str>,
}

const BASE_SKILL_GROUP: &[&str] = &["list", "watch", "write", "search"];
const LEADER_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "leader",
    description: "Dispatch tasks to the right role and manage scheduled tasks.",
    system_instruction_prompt: prompt::ROLE_LEADER_SYSTEM_PROMPT,
    skill_groups: &["base"],
    skills: &["crond", "leader"],
    include_memory_context: true,
    experience_memory_rule: None,
};
const PAPERWORK_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "paperwork",
    description: "Handle document writing, note taking, and paperwork-oriented local tasks.",
    system_instruction_prompt: prompt::ROLE_PAPERWORK_SYSTEM_PROMPT,
    skill_groups: &["base"],
    skills: &[],
    include_memory_context: false,
    experience_memory_rule: None,
};
const ALL_IN_ONE_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "all-in-one",
    description: "Fallback worker with every built-in skill enabled.",
    system_instruction_prompt: prompt::ROLE_ALL_IN_ONE_SYSTEM_PROMPT,
    skill_groups: &["base"],
    skills: &["remember", "crond"],
    include_memory_context: false,
    experience_memory_rule: None,
};
const INFO_SEARCHER_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "info-searcher",
    description: "Handle browser-driven webpage navigation, search, and information extraction.",
    system_instruction_prompt: prompt::ROLE_INFO_SEARCHER_SYSTEM_PROMPT,
    skill_groups: &[],
    skills: &["web-search", "browser"],
    include_memory_context: false,
    experience_memory_rule: Some(
        "Keep stable mappings between app/site names, URL addresses, and how leaders or owners are called. Prefer records like `应用名:xxx url地址:xxx leader称呼:xxx 说明:xxx`.",
    ),
};
const CODER_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "coder",
    description: "Handle coding and repository tasks by controlling a terminal coding agent.",
    system_instruction_prompt: prompt::ROLE_CODER_SYSTEM_PROMPT,
    skill_groups: &[],
    skills: &["terminal"],
    include_memory_context: false,
    experience_memory_rule: Some(
        "Keep the active development project inventory. Prefer records like `项目名:xxx 项目路径:xxx 项目简介:xxx` and update them when project scope or path changes.",
    ),
};

const BUILTIN_ROLE_SPECS: &[&BottyGuyRoleSpec] = &[
    &LEADER_ROLE_SPEC,
    &PAPERWORK_ROLE_SPEC,
    &ALL_IN_ONE_ROLE_SPEC,
    &INFO_SEARCHER_ROLE_SPEC,
    &CODER_ROLE_SPEC,
];

pub fn run() {
    set_process_name(guy_process_name());
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = stdin.lock().lines();

    while let Some(line_result) = lines.next() {
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                eprintln!("Botty-Guy failed to read input: {err}");
                break;
            }
        };
        let message = match decode_ipc_line(line.trim_end()) {
            Ok(message) => message,
            Err(err) => {
                eprintln!("Botty-Guy failed to decode input: {err}");
                continue;
            }
        };
        let message = message.trim();
        if message.is_empty() {
            continue;
        }

        if let Some((key, value)) = parse_set_env_control(message) {
            // SAFETY: setting process environment is intended here as a local control action
            // handled on the single worker process before the value is read by later requests.
            unsafe {
                std::env::set_var(&key, &value);
            }
            let reply = AssistantReply {
                text: format!("set env ok: {key}"),
                thinking: None,
                control: None,
            };
            let encoded_reply = match encode_ipc_line(&encode_assistant_reply(&reply)) {
                Ok(reply) => reply,
                Err(err) => {
                    eprintln!("Botty-Guy failed to encode control output: {err}");
                    break;
                }
            };
            if let Err(err) = writeln!(stdout, "{encoded_reply}") {
                eprintln!("Botty-Guy failed to write control output: {err}");
                break;
            }
            if let Err(err) = stdout.flush() {
                eprintln!("Botty-Guy failed to flush control output: {err}");
                break;
            }
            continue;
        }

        if let Some((continuation_payload, tool_result)) = parse_resume_control(message) {
            let reply = match BottyBody::from_setup(requested_role().as_str())
                .and_then(|body| body.resume_tool_call(&continuation_payload, &tool_result))
            {
                Ok(reply) => reply,
                Err(err) => {
                    eprintln!("Botty-Guy failed to resume tool call: {err}");
                    AssistantReply {
                        text: err.to_string(),
                        thinking: None,
                        control: None,
                    }
                }
            };
            let encoded_reply = match encode_ipc_line(&encode_assistant_reply(&reply)) {
                Ok(reply) => reply,
                Err(err) => {
                    eprintln!("Botty-Guy failed to encode resume output: {err}");
                    break;
                }
            };
            if let Err(err) = writeln!(stdout, "{encoded_reply}") {
                eprintln!("Botty-Guy failed to write resume output: {err}");
                break;
            }
            if let Err(err) = stdout.flush() {
                eprintln!("Botty-Guy failed to flush resume output: {err}");
                break;
            }
            continue;
        }

        // Rebuild the body for each message so skill/config/env changes take effect
        // without requiring a worker restart.
        let reply = match BottyBody::from_setup(requested_role().as_str())
            .and_then(|body| body.think(message))
        {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("Botty-Guy failed to process input: {err}");
                AssistantReply {
                    text: err.to_string(),
                    thinking: None,
                    control: None,
                }
            }
        };
        let encoded_reply = match encode_ipc_line(&encode_assistant_reply(&reply)) {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("Botty-Guy failed to encode output: {err}");
                break;
            }
        };
        if let Err(err) = writeln!(stdout, "{encoded_reply}") {
            eprintln!("Botty-Guy failed to write output: {err}");
            break;
        }
        if let Err(err) = stdout.flush() {
            eprintln!("Botty-Guy failed to flush output: {err}");
            break;
        }
    }
}

pub(crate) fn requested_role() -> String {
    std::env::var(BOTTY_GUY_ROLE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BOTTY_GUY_DEFAULT_ROLE.to_string())
}

pub(crate) fn resolve_role_spec(role: &str) -> Option<&'static BottyGuyRoleSpec> {
    match role.trim() {
        "" | BOTTY_GUY_DEFAULT_ROLE => Some(&LEADER_ROLE_SPEC),
        "paperwork" => Some(&PAPERWORK_ROLE_SPEC),
        "all-in-one" => Some(&ALL_IN_ONE_ROLE_SPEC),
        "info-searcher" => Some(&INFO_SEARCHER_ROLE_SPEC),
        "coder" => Some(&CODER_ROLE_SPEC),
        _ => None,
    }
}

pub(crate) fn builtin_role_specs() -> &'static [&'static BottyGuyRoleSpec] {
    BUILTIN_ROLE_SPECS
}

pub(crate) fn resolve_role_spec_or_custom(role: &str) -> Option<ResolvedRole> {
    if let Some(spec) = resolve_role_spec(role) {
        return Some(ResolvedRole::Builtin(spec));
    }
    load_custom_role_config(role).map(ResolvedRole::Custom)
}

pub(crate) enum ResolvedRole {
    Builtin(&'static BottyGuyRoleSpec),
    Custom(CustomRoleConfig),
}

#[derive(Clone)]
pub(crate) struct CustomRoleConfig {
    pub name: String,
    pub description: String,
    pub skills: Vec<String>,
}

pub(crate) fn expand_role_skill_names(spec: &BottyGuyRoleSpec) -> Vec<&'static str> {
    let mut names = Vec::new();
    for group in spec.skill_groups {
        for skill in skill_group_members(group) {
            if !names.contains(skill) {
                names.push(*skill);
            }
        }
    }
    for skill in spec.skills {
        if !names.contains(skill) {
            names.push(*skill);
        }
    }
    names
}

pub(crate) fn expand_custom_role_skill_names(config: &CustomRoleConfig) -> Vec<String> {
    let mut names = Vec::new();
    for skill in &config.skills {
        if skill == "base" {
            for base_skill in BASE_SKILL_GROUP {
                let s = base_skill.to_string();
                if !names.contains(&s) {
                    names.push(s);
                }
            }
        } else if !names.contains(skill) {
            names.push(skill.clone());
        }
    }
    names
}

pub(crate) fn delegated_role_names() -> Vec<String> {
    let mut names: Vec<String> = vec![
        CODER_ROLE_SPEC.role.to_string(),
        INFO_SEARCHER_ROLE_SPEC.role.to_string(),
        PAPERWORK_ROLE_SPEC.role.to_string(),
        ALL_IN_ONE_ROLE_SPEC.role.to_string(),
    ];
    for config in load_all_custom_role_configs() {
        if !names.iter().any(|n| n == &config.name) {
            names.push(config.name);
        }
    }
    names
}

pub(crate) fn delegated_role_exists(role: &str) -> bool {
    delegated_role_names().iter().any(|n| n == role)
}

pub(crate) fn delegated_role_descriptions() -> Vec<(String, String)> {
    let mut result = vec![
        (
            CODER_ROLE_SPEC.role.to_string(),
            CODER_ROLE_SPEC.description.to_string(),
        ),
        (
            INFO_SEARCHER_ROLE_SPEC.role.to_string(),
            INFO_SEARCHER_ROLE_SPEC.description.to_string(),
        ),
        (
            PAPERWORK_ROLE_SPEC.role.to_string(),
            PAPERWORK_ROLE_SPEC.description.to_string(),
        ),
        (
            ALL_IN_ONE_ROLE_SPEC.role.to_string(),
            ALL_IN_ONE_ROLE_SPEC.description.to_string(),
        ),
    ];
    for config in load_all_custom_role_configs() {
        if !result.iter().any(|(n, _)| n == &config.name) {
            result.push((config.name, config.description));
        }
    }
    result
}

pub(crate) fn delegated_task_prompt(role: &str, task: &str, necessary_info: &str) -> String {
    let context_block = if necessary_info.trim().is_empty() {
        "Necessary information:\n(none)".to_string()
    } else {
        format!("Necessary information:\n{}", necessary_info.trim())
    };

    format!(
        "Delegated by leader.\nTarget role: {role}\nTask:\n{}\n\n{}\n\nReturn the final answer directly.",
        task.trim(),
        context_block
    )
}

fn skill_group_members(group: &str) -> &'static [&'static str] {
    match group {
        "base" => BASE_SKILL_GROUP,
        _ => &[],
    }
}

pub fn run_telegram_input() {
    set_process_name(telegram_input_process_name());

    let config = match load_chatbot_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Botty-input-telegram failed to load config: {err}");
            return;
        }
    };

    if !config.enabled {
        return;
    }
    if config.apikey.is_empty() {
        eprintln!("Botty-input-telegram skipped: chatbot.telegram.apikey is empty");
        return;
    }

    let interval = config.poll_interval();
    let mut plugin = TelegramProviderPlugin::new(
        config.apikey,
        config.telegram_api_base,
        interval,
        config.telegram_whitelist_user_ids.clone(),
    );
    run_input_provider_loop(&mut plugin);
}

pub fn run_feishu_input() {
    set_process_name(feishu_input_process_name());

    let config = match load_chatbot_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Botty-input-feishu failed to load config: {err}");
            return;
        }
    };

    if !config.feishu_enabled {
        return;
    }
    if config.feishu_app_id.is_empty() || config.feishu_app_secret.is_empty() {
        eprintln!("Botty-input-feishu skipped: feishu long connection requires app_id/app_secret");
        return;
    }

    let interval = config.feishu_poll_interval();
    let mut plugin = FeishuProviderPlugin::new(
        config.feishu_app_id,
        config.feishu_app_secret,
        config.feishu_access_token,
        config.feishu_api_base,
        interval,
    );
    run_input_provider_loop(&mut plugin);
}

pub fn run_weixin_input() {
    set_process_name(weixin_input_process_name());

    let config = match load_chatbot_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Botty-input-weixin failed to load config: {err}");
            return;
        }
    };

    if !config.weixin_enabled {
        return;
    }
    if config.weixin_apikey.is_empty() {
        eprintln!("Botty-input-weixin skipped: chatbot.weixin.apikey is empty");
        return;
    }

    let interval = config.weixin_poll_interval();
    let sync_path = weixin_sync_file(&config.weixin_account_id);
    let mut plugin = WeixinProviderPlugin::new(
        config.weixin_apikey,
        config.weixin_api_base,
        interval,
        config.weixin_whitelist_user_ids,
        config.weixin_long_poll_timeout_ms,
        sync_path,
    );
    run_input_provider_loop(&mut plugin);
}

struct InboundMessage {
    message_id: String,
    target: String,
    user_id: String,
    text: String,
}

struct PendingLeaderReply {
    target: String,
    user_id: String,
    job_message_id: String,
    notice_sent: bool,
}

trait ChatbotProviderPlugin {
    fn provider_name(&self) -> &'static str;
    fn poll_interval(&self) -> Duration;
    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>>;
    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str;
    fn should_skip_initial_messages(&self) -> bool {
        true
    }
    fn is_user_allowed(&self, _user_id: &str) -> bool {
        true
    }
    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>>;
}

struct OutgoingAttachment {
    kind: String,
    path: String,
    caption: String,
}

fn run_input_provider_loop(plugin: &mut impl ChatbotProviderPlugin) {
    let mut seen = HashSet::new();
    let mut seen_order = VecDeque::new();
    let mut pending_replies = Vec::new();
    let mut initialized = false;

    loop {
        flush_pending_leader_replies(plugin, &mut pending_replies, &mut seen, &mut seen_order);

        let messages = match plugin.fetch_messages() {
            Ok(messages) => messages,
            Err(err) => {
                eprintln!("{} fetch messages failed: {err}", plugin.provider_name());
                thread::sleep(plugin.poll_interval());
                continue;
            }
        };

        if !initialized && plugin.should_skip_initial_messages() {
            for message in messages {
                let _ = remember_message_id(&mut seen, &mut seen_order, &message.message_id);
            }
            initialized = true;
            thread::sleep(plugin.poll_interval());
            continue;
        }
        initialized = true;

        for message in messages {
            if !remember_message_id(&mut seen, &mut seen_order, &message.message_id) {
                continue;
            }

            let user_id = plugin.user_id(&message);
            if !plugin.is_user_allowed(user_id) {
                let _ =
                    persist_chat_message("user", plugin.provider_name(), user_id, &message.text);
                let _ = persist_chat_message(
                    "assistant",
                    plugin.provider_name(),
                    user_id,
                    "Sorry, I'm just a little Botty.",
                );
                match plugin.send_reply(&message.target, "Sorry, I'm just a little Botty.") {
                    Ok(Some(sent_id)) => {
                        let _ = remember_message_id(&mut seen, &mut seen_order, &sent_id);
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("{} send message failed: {err}", plugin.provider_name()),
                }
                continue;
            }

            let normalized = normalize_line_message(&message.text);
            if normalized.is_empty() {
                continue;
            }
            let prefixed = format!("{}: {normalized}", plugin.provider_name());

            let job_message_id = match enqueue_leader_guy(
                plugin.provider_name(),
                user_id,
                &message.target,
                &prefixed,
            ) {
                Ok(job_message_id) => job_message_id,
                Err(err) => {
                    eprintln!("{} enqueue leader failed: {err}", plugin.provider_name());
                    let reply = err.to_string();
                    let _ =
                        persist_chat_message("assistant", plugin.provider_name(), user_id, &reply);
                    if !reply.trim().is_empty() {
                        match plugin.send_reply(&message.target, &reply) {
                            Ok(Some(sent_id)) => {
                                let _ = remember_message_id(&mut seen, &mut seen_order, &sent_id);
                            }
                            Ok(None) => {}
                            Err(err) => {
                                eprintln!("{} send message failed: {err}", plugin.provider_name())
                            }
                        }
                    }
                    continue;
                }
            };
            pending_replies.push(PendingLeaderReply {
                target: message.target.clone(),
                user_id: user_id.to_string(),
                job_message_id,
                notice_sent: false,
            });
        }

        flush_pending_leader_replies(plugin, &mut pending_replies, &mut seen, &mut seen_order);
        thread::sleep(plugin.poll_interval());
    }
}

fn flush_pending_leader_replies(
    plugin: &mut impl ChatbotProviderPlugin,
    pending_replies: &mut Vec<PendingLeaderReply>,
    seen: &mut HashSet<String>,
    seen_order: &mut VecDeque<String>,
) {
    let mut index = 0usize;
    while index < pending_replies.len() {
        if !pending_replies[index].notice_sent {
            match try_load_leader_job_notice(&pending_replies[index].job_message_id) {
                Ok(Some(notice)) => {
                    if !notice.trim().is_empty() {
                        match plugin.send_reply(&pending_replies[index].target, &notice) {
                            Ok(Some(sent_id)) => {
                                let _ = remember_message_id(seen, seen_order, &sent_id);
                            }
                            Ok(None) => {}
                            Err(err) => {
                                eprintln!("{} send message failed: {err}", plugin.provider_name());
                                index += 1;
                                continue;
                            }
                        }
                    }
                    pending_replies[index].notice_sent = true;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!(
                        "{} load leader notice {} failed: {err}",
                        plugin.provider_name(),
                        pending_replies[index].job_message_id
                    );
                    index += 1;
                    continue;
                }
            }
        }

        let outcome = match try_load_leader_job_outcome(&pending_replies[index].job_message_id) {
            Ok(outcome) => outcome,
            Err(err) => {
                eprintln!(
                    "{} load leader job {} failed: {err}",
                    plugin.provider_name(),
                    pending_replies[index].job_message_id
                );
                index += 1;
                continue;
            }
        };

        let Some(reply) = outcome else {
            index += 1;
            continue;
        };

        let pending = pending_replies.remove(index);
        let _ = persist_chat_message(
            "assistant",
            plugin.provider_name(),
            &pending.user_id,
            &reply,
        );
        if reply.trim().is_empty() {
            continue;
        }
        match plugin.send_reply(&pending.target, &reply) {
            Ok(Some(sent_id)) => {
                let _ = remember_message_id(seen, seen_order, &sent_id);
            }
            Ok(None) => {}
            Err(err) => eprintln!("{} send message failed: {err}", plugin.provider_name()),
        }
    }
}

fn try_load_leader_job_notice(message_id: &str) -> io::Result<Option<String>> {
    let root = botty_root_dir();
    match botty_jobs::load_job(&root, BOTTY_GUY_DEFAULT_ROLE, JobState::Waiting, message_id) {
        Ok(job) => Ok(job
            .pending_user_notice
            .filter(|text| !text.trim().is_empty())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn try_load_leader_job_outcome(message_id: &str) -> io::Result<Option<String>> {
    let root = botty_root_dir();
    match botty_jobs::load_job(&root, BOTTY_GUY_DEFAULT_ROLE, JobState::Done, message_id) {
        Ok(job) => return Ok(Some(job.result_text.unwrap_or_default())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    match botty_jobs::load_job(&root, BOTTY_GUY_DEFAULT_ROLE, JobState::Failed, message_id) {
        Ok(job) => Ok(Some(
            job.last_error
                .unwrap_or_else(|| "leader job failed".to_string()),
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn enqueue_leader_guy(
    source: &str,
    user_id: &str,
    target: &str,
    message: &str,
) -> io::Result<String> {
    let stream = UnixStream::connect(crate::botty_boss::chat_socket_path())?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);
    let payload = json!({
        "source": source,
        "user_id": user_id,
        "target": target,
        "message": message,
    })
    .to_string();
    let control = format!("{CONTROL_PREFIX}enqueue-external|{payload}");
    writeln!(
        writer,
        "{}",
        encode_ipc_line(&encode_meta_message(source, user_id, &control))?
    )?;
    writer.flush()?;

    let mut reply = String::new();
    let bytes = reader.read_line(&mut reply)?;
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Botty-Boss closed socket",
        ));
    }
    let decoded = decode_ipc_line(reply.trim_end())?;
    Ok(decoded)
}

struct TelegramProviderPlugin {
    client: TelegramClient,
    poll_interval: Duration,
    offset: i64,
    whitelist_user_ids: HashSet<String>,
}

impl TelegramProviderPlugin {
    fn new(
        apikey: String,
        api_base: String,
        poll_interval: Duration,
        whitelist_user_ids: HashSet<String>,
    ) -> Self {
        Self {
            client: TelegramClient::new(api_base, apikey),
            poll_interval,
            offset: 0,
            whitelist_user_ids,
        }
    }
}

impl ChatbotProviderPlugin for TelegramProviderPlugin {
    fn provider_name(&self) -> &'static str {
        "telegram"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>> {
        let updates = self.client.fetch_updates(self.offset)?;
        let mut messages = Vec::new();
        for update in updates {
            if update.update_id >= self.offset {
                self.offset = update.update_id + 1;
            }
            messages.push(InboundMessage {
                message_id: update.update_id.to_string(),
                target: update.chat_id.to_string(),
                user_id: update.user_id.to_string(),
                text: update.text,
            });
        }
        Ok(messages)
    }

    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str {
        message.user_id.as_str()
    }

    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.whitelist_user_ids.contains(user_id)
    }

    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>> {
        let chat_id = target
            .parse::<i64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid telegram chat_id"))?;
        let (clean_text, attachments) = parse_outgoing_attachments(text);
        for attachment in attachments {
            if attachment.kind == "photo" {
                self.client
                    .send_photo(chat_id, &attachment.path, &attachment.caption)?;
            }
        }
        if !clean_text.trim().is_empty() {
            self.client.send_message(chat_id, &clean_text)?;
        }
        Ok(None)
    }
}

struct FeishuProviderPlugin {
    client: FeishuClient,
    events: FeishuLongConnClient,
    poll_interval: Duration,
}

impl FeishuProviderPlugin {
    fn new(
        app_id: String,
        app_secret: String,
        access_token: String,
        api_base: String,
        poll_interval: Duration,
    ) -> Self {
        Self {
            events: FeishuLongConnClient::new(api_base.clone(), app_id.clone(), app_secret.clone()),
            client: FeishuClient::new(api_base, app_id, app_secret, access_token),
            poll_interval,
        }
    }
}

impl ChatbotProviderPlugin for FeishuProviderPlugin {
    fn provider_name(&self) -> &'static str {
        "feishu"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>> {
        let Some(message) = self.events.poll_message()? else {
            return Ok(Vec::new());
        };
        Ok(vec![InboundMessage {
            message_id: message.message_id,
            target: message.chat_id,
            user_id: message.user_id,
            text: message.text,
        }])
    }

    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str {
        message.user_id.as_str()
    }

    fn should_skip_initial_messages(&self) -> bool {
        false
    }

    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>> {
        let (clean_text, _) = parse_outgoing_attachments(text);
        self.client.send_message(target, &clean_text)
    }
}

struct WeixinProviderPlugin {
    client: WeixinClient,
    poll_interval: Duration,
    whitelist_user_ids: HashSet<String>,
    context_tokens: HashMap<String, String>,
    get_updates_buf: String,
    sync_file: PathBuf,
    long_poll_timeout_ms: u64,
}

impl WeixinProviderPlugin {
    fn new(
        apikey: String,
        api_base: String,
        poll_interval: Duration,
        whitelist_user_ids: HashSet<String>,
        long_poll_timeout_ms: u64,
        sync_file: PathBuf,
    ) -> Self {
        let get_updates_buf = load_weixin_sync_buf(&sync_file).unwrap_or_default();
        Self {
            client: WeixinClient::new(api_base, apikey),
            poll_interval,
            whitelist_user_ids,
            context_tokens: HashMap::new(),
            get_updates_buf,
            sync_file,
            long_poll_timeout_ms,
        }
    }
}

impl ChatbotProviderPlugin for WeixinProviderPlugin {
    fn provider_name(&self) -> &'static str {
        "weixin"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>> {
        match self
            .client
            .fetch_updates(&self.get_updates_buf, self.long_poll_timeout_ms)
        {
            Ok(result) => {
                if !result.get_updates_buf.is_empty() {
                    self.get_updates_buf = result.get_updates_buf;
                    let _ = save_weixin_sync_buf(&self.sync_file, &self.get_updates_buf);
                }
                if let Some(timeout_ms) = result.longpolling_timeout_ms {
                    self.long_poll_timeout_ms = timeout_ms.max(1_000);
                }

                let mut messages = Vec::new();
                for message in result.messages {
                    self.context_tokens
                        .insert(message.user_id.clone(), message.context_token);
                    messages.push(InboundMessage {
                        message_id: message.message_id,
                        target: message.user_id.clone(),
                        user_id: message.user_id,
                        text: message.text,
                    });
                }
                Ok(messages)
            }
            Err(err) => {
                if err
                    .to_string()
                    .contains(&format!("errcode={SESSION_EXPIRED_ERRCODE}"))
                {
                    return Err(io::Error::other(format!(
                        "weixin session expired, rerun `mylittlebotty weixin-login`: {err}"
                    )));
                }
                Err(err)
            }
        }
    }

    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str {
        message.user_id.as_str()
    }

    fn should_skip_initial_messages(&self) -> bool {
        false
    }

    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.whitelist_user_ids.is_empty() || self.whitelist_user_ids.contains(user_id)
    }

    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>> {
        let (clean_text, _) = parse_outgoing_attachments(text);
        if clean_text.trim().is_empty() {
            return Ok(None);
        }
        let context_token = self.context_tokens.get(target).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("weixin context_token missing for target {target}"),
            )
        })?;
        self.client.send_message(target, &clean_text, &context_token)?;
        Ok(None)
    }
}

fn guy_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-Guy-dev"
    } else {
        "Botty-Guy"
    }
}

fn telegram_input_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-input-telegram-dev"
    } else {
        "Botty-input-telegram"
    }
}

fn feishu_input_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-input-feishu-dev"
    } else {
        "Botty-input-feishu"
    }
}

fn weixin_input_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-input-weixin-dev"
    } else {
        "Botty-input-weixin"
    }
}

struct ChatbotConfig {
    enabled: bool,
    apikey: String,
    telegram_api_base: String,
    poll_interval_seconds: u64,
    feishu_enabled: bool,
    feishu_app_id: String,
    feishu_app_secret: String,
    feishu_access_token: String,
    feishu_api_base: String,
    feishu_chat_id: String,
    feishu_poll_interval_seconds: u64,
    weixin_enabled: bool,
    weixin_apikey: String,
    weixin_api_base: String,
    weixin_account_id: String,
    weixin_user_id: String,
    weixin_poll_interval_seconds: u64,
    weixin_long_poll_timeout_ms: u64,
    weixin_whitelist_user_ids: HashSet<String>,
    telegram_whitelist_user_ids: HashSet<String>,
}

impl Default for ChatbotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            apikey: String::new(),
            telegram_api_base: TELEGRAM_API_BASE.to_string(),
            poll_interval_seconds: TELEGRAM_POLL_INTERVAL_SECONDS_DEFAULT,
            feishu_enabled: false,
            feishu_app_id: String::new(),
            feishu_app_secret: String::new(),
            feishu_access_token: String::new(),
            feishu_api_base: FEISHU_API_BASE.to_string(),
            feishu_chat_id: String::new(),
            feishu_poll_interval_seconds: FEISHU_POLL_INTERVAL_SECONDS_DEFAULT,
            weixin_enabled: false,
            weixin_apikey: String::new(),
            weixin_api_base: WEIXIN_API_BASE.to_string(),
            weixin_account_id: String::new(),
            weixin_user_id: String::new(),
            weixin_poll_interval_seconds: WEIXIN_POLL_INTERVAL_SECONDS_DEFAULT,
            weixin_long_poll_timeout_ms: WEIXIN_LONG_POLL_TIMEOUT_MS_DEFAULT,
            weixin_whitelist_user_ids: HashSet::new(),
            telegram_whitelist_user_ids: HashSet::new(),
        }
    }
}

impl ChatbotConfig {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_seconds.max(1))
    }

    fn feishu_poll_interval(&self) -> Duration {
        Duration::from_secs(self.feishu_poll_interval_seconds.max(1))
    }

    fn weixin_poll_interval(&self) -> Duration {
        Duration::from_secs(self.weixin_poll_interval_seconds.max(1))
    }
}

fn load_chatbot_config() -> io::Result<ChatbotConfig> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ChatbotConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = ChatbotConfig::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "chatbot.provider" => {
                config.enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "telegram");
                config.feishu_enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "feishu");
                config.weixin_enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "weixin");
            }
            "chatbot.telegram.enabled" => config.enabled = parse_bool(value),
            "chatbot.telegram.apikey" => config.apikey = value.to_string(),
            "chatbot.telegram.api_base" => config.telegram_api_base = value.to_string(),
            "chatbot.telegram.whitelist_user_ids" => {
                config.telegram_whitelist_user_ids = parse_user_id_whitelist(value);
            }
            "chatbot.telegram.whitelise_user_ids" => {
                config.telegram_whitelist_user_ids = parse_user_id_whitelist(value);
            }
            "chatbot.feishu.enabled" => config.feishu_enabled = parse_bool(value),
            "chatbot.feishu.app_id" => config.feishu_app_id = value.to_string(),
            "chatbot.feishu.app_secret" => config.feishu_app_secret = value.to_string(),
            "chatbot.feishu.apikey" => config.feishu_access_token = value.to_string(),
            "chatbot.feishu.api_base" => config.feishu_api_base = value.to_string(),
            "chatbot.feishu.chat_id" => config.feishu_chat_id = value.to_string(),
            "chatbot.weixin.enabled" => config.weixin_enabled = parse_bool(value),
            "chatbot.weixin.apikey" => config.weixin_apikey = value.to_string(),
            "chatbot.weixin.api_base" => config.weixin_api_base = value.to_string(),
            "chatbot.weixin.account_id" => config.weixin_account_id = value.to_string(),
            "chatbot.weixin.user_id" => config.weixin_user_id = value.to_string(),
            "chatbot.weixin.whitelist_user_ids" => {
                config.weixin_whitelist_user_ids = parse_user_id_whitelist(value);
            }
            "chatbot.apikey" => {
                if config.apikey.is_empty() {
                    config.apikey = value.to_string();
                }
                if config.feishu_access_token.is_empty() {
                    config.feishu_access_token = value.to_string();
                }
                if config.weixin_apikey.is_empty() {
                    config.weixin_apikey = value.to_string();
                }
            }
            "chatbot.telegram.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.feishu.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.feishu_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.weixin.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.weixin_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.weixin.long_poll_timeout_ms" => {
                if let Ok(timeout_ms) = value.parse::<u64>() {
                    config.weixin_long_poll_timeout_ms = timeout_ms.max(1_000);
                }
            }
            _ => {}
        }
    }

    Ok(config)
}

fn parse_user_id_whitelist(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

fn persist_chat_message(role: &str, source: &str, user_id: &str, message: &str) -> io::Result<()> {
    let year = local_time_format("%Y")?;
    let month_day = local_time_format("%m%d")?;
    let timestamp = local_time_format("%Y-%m-%d %H:%M:%S")?;
    let sanitized = message.replace('\n', "\\n").replace('\r', "\\r");
    let line = format!("[{timestamp}] source={source} user_id={user_id} {role}: {sanitized}\n");

    let dir = botty_root_dir().join("memory").join("deep").join(year);
    fs::create_dir_all(&dir)?;

    let target = select_chat_memory_file(&dir, &month_day, line.len() as u64)?;
    let mut file = OpenOptions::new().create(true).append(true).open(target)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn select_chat_memory_file(
    dir: &PathBuf,
    month_day: &str,
    incoming_bytes: u64,
) -> io::Result<PathBuf> {
    for index in 1..=9_999u32 {
        let candidate = dir.join(format!("{month_day}-{index}.log"));
        let size = match fs::metadata(&candidate) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err),
        };

        if size.saturating_add(incoming_bytes) <= CHAT_MEMORY_MAX_BYTES {
            return Ok(candidate);
        }
    }

    Err(io::Error::other(
        "too many chat memory files for current day",
    ))
}

fn local_time_format(format: &str) -> io::Result<String> {
    let output = Command::new("date").arg(format!("+{format}")).output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn encode_assistant_reply(reply: &AssistantReply) -> String {
    json!({
        "text": reply.text,
        "thinking": reply.thinking,
        "control": reply.control,
    })
    .to_string()
}

fn encode_meta_message(source: &str, user_id: &str, message: &str) -> String {
    format!("{CHAT_META_PREFIX}|source={source}|user_id={user_id}|{message}")
}

fn parse_set_env_control(message: &str) -> Option<(String, String)> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("set-env|")?;
    let mut parts = payload.splitn(2, '|');
    let key = parts.next()?.trim();
    let value = parts.next()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value))
}

fn parse_resume_control(message: &str) -> Option<(String, String)> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("resume|")?;
    let value: Value = serde_json::from_str(payload).ok()?;
    Some((
        value.get("continuation_payload")?.as_str()?.to_string(),
        value.get("tool_result")?.as_str()?.to_string(),
    ))
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

fn normalize_line_message(message: &str) -> String {
    message
        .replace('\n', " ")
        .replace('\r', " ")
        .trim()
        .to_string()
}

fn remember_message_id(
    seen: &mut HashSet<String>,
    seen_order: &mut VecDeque<String>,
    message_id: &str,
) -> bool {
    if seen.contains(message_id) {
        return false;
    }
    let owned = message_id.to_string();
    seen.insert(owned.clone());
    seen_order.push_back(owned);

    while seen_order.len() > FEISHU_SEEN_CACHE_LIMIT {
        if let Some(oldest) = seen_order.pop_front() {
            seen.remove(&oldest);
        }
    }
    true
}

fn parse_outgoing_attachments(text: &str) -> (String, Vec<OutgoingAttachment>) {
    let mut clean_lines = Vec::new();
    let mut attachments = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        let marker_line = trimmed.strip_prefix("attachment=").unwrap_or(trimmed);
        if let Some(payload) = marker_line.strip_prefix(ATTACHMENT_PREFIX) {
            let payload = payload.strip_prefix('|').unwrap_or(payload);
            let mut parts = payload.splitn(3, '|');
            let kind = parts.next().unwrap_or_default().trim();
            let path = parts.next().unwrap_or_default().trim();
            let caption = parts.next().unwrap_or_default().trim();
            if !kind.is_empty() && !path.is_empty() {
                attachments.push(OutgoingAttachment {
                    kind: kind.to_string(),
                    path: path.to_string(),
                    caption: caption.to_string(),
                });
                continue;
            }
        }
        clean_lines.push(line);
    }

    (clean_lines.join("\n").trim().to_string(), attachments)
}

fn weixin_sync_file(account_id: &str) -> PathBuf {
    let name = if account_id.trim().is_empty() {
        "default".to_string()
    } else {
        sanitize_filename_component(account_id)
    };
    botty_root_dir()
        .join("config")
        .join(format!("weixin-{name}{}.sync.json", runtime_suffix()))
}

fn load_weixin_sync_buf(path: &PathBuf) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    value
        .get("get_updates_buf")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn save_weixin_sync_buf(path: &PathBuf, get_updates_buf: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json!({ "get_updates_buf": get_updates_buf }).to_string())
}

fn sanitize_filename_component(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch);
        } else {
            result.push('-');
        }
    }
    let trimmed = result.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn botty_root_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}

fn set_process_name(name: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, c_name.as_ptr() as usize, 0, 0, 0);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::pthread_setname_np(c_name.as_ptr());
            }
        }
    }
}

// --- Custom sub-agent / role config ---

fn sub_agents_config_file() -> PathBuf {
    botty_root_dir().join("config").join("sub-agents.json")
}

pub(crate) fn load_all_custom_role_configs() -> Vec<CustomRoleConfig> {
    let path = sub_agents_config_file();
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    let value: Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let agents = match value.get("agents").and_then(Value::as_array) {
        Some(agents) => agents,
        None => return Vec::new(),
    };
    agents
        .iter()
        .filter_map(|agent| {
            let name = agent.get("name")?.as_str()?.to_string();
            let description = agent
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let skills = agent
                .get("skills")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            Some(CustomRoleConfig {
                name,
                description,
                skills,
            })
        })
        .collect()
}

pub(crate) fn load_custom_role_config(role: &str) -> Option<CustomRoleConfig> {
    load_all_custom_role_configs()
        .into_iter()
        .find(|config| config.name == role)
}

pub(crate) fn save_custom_role_config(config: &CustomRoleConfig) -> io::Result<PathBuf> {
    let path = sub_agents_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut all = load_all_custom_role_configs();
    if let Some(existing) = all.iter_mut().find(|c| c.name == config.name) {
        existing.description = config.description.clone();
        existing.skills = config.skills.clone();
    } else {
        all.push(config.clone());
    }
    let agents_json: Vec<Value> = all
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "description": c.description,
                "skills": c.skills,
            })
        })
        .collect();
    let content = serde_json::to_string_pretty(&json!({ "agents": agents_json }))
        .map_err(|err| io::Error::other(format!("serialize sub-agents config failed: {err}")))?;
    fs::write(&path, content)?;
    Ok(path)
}

pub(crate) fn delete_custom_role_config(name: &str) -> io::Result<()> {
    let path = sub_agents_config_file();
    if !path.exists() {
        return Ok(());
    }
    let mut all = load_all_custom_role_configs();
    all.retain(|c| c.name != name);
    let agents_json: Vec<Value> = all
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "description": c.description,
                "skills": c.skills,
            })
        })
        .collect();
    let content = serde_json::to_string_pretty(&json!({ "agents": agents_json }))
        .map_err(|err| io::Error::other(format!("serialize sub-agents config failed: {err}")))?;
    fs::write(&path, content)?;
    Ok(())
}
