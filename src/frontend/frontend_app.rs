use crate::botty_guy::{
    delete_custom_role_config, load_all_custom_role_configs, save_custom_role_config,
    CustomRoleConfig,
};
use crate::frontend::frontend_service::{
    command_suggestions, AiProviderProfile, FrontendRpc, GuyEnvSetResult, RestartStatus,
    SaveSetupResult, SetupConfig, SetupFieldId,
};
use crate::io as botty_io;
use crate::skill::all_available_skill_names;
use crate::skill::custom_skill;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Copy)]
pub enum Role {
    User,
    Bot,
    System,
}

pub struct ChatLine {
    pub role: Role,
    pub text: String,
}

pub enum Mode {
    Chat,
    Setup {
        selected_field: usize,
        selected_provider: usize,
        overlay: Option<SetupOverlay>,
        original_work_dir: String,
        config: SetupConfig,
    },
    GuyEnvEdit {
        selected_entry: usize,
        editor: Option<GuyEnvEditor>,
        entries: Vec<(String, String)>,
    },
    GuyEnvList {
        selected_entry: usize,
        entries: Vec<(String, String)>,
    },
    SubAgent {
        agents: Vec<SubAgentEntry>,
        selected_entry: usize,
        editor: Option<SubAgentEditor>,
        confirm_delete: Option<SubAgentDeleteConfirm>,
    },
    CreateSkill {
        editor: CreateSkillEditor,
    },
}

pub struct SubAgentEntry {
    pub name: String,
    pub description: String,
    pub skills: Vec<String>,
}

pub struct SubAgentEditor {
    pub original_name: Option<String>,
    pub name_input: String,
    pub name_cursor: usize,
    pub description_input: String,
    pub description_cursor: usize,
    pub available_skills: Vec<String>,
    pub selected_skills: Vec<bool>,
    pub skill_scroll: usize,
    pub focus: SubAgentEditorFocus,
    pub generating_description: bool,
}

pub struct SubAgentDeleteConfirm {
    pub name: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SubAgentEditorFocus {
    Name,
    Description,
    Skills,
}

pub struct CreateSkillEditor {
    pub name_input: String,
    pub name_cursor: usize,
    pub purpose_input: String,
    pub purpose_cursor: usize,
    pub focus: CreateSkillEditorFocus,
    pub generated_description: String,
    pub generating_description: bool,
    pub pending_generated_name: String,
    pub pending_generated_purpose: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CreateSkillEditorFocus {
    Name,
    Purpose,
}

pub enum SetupOverlay {
    Field(FieldEdit),
    AiProfiles(SetupProfilePanel),
    ChatbotProviders(ChatbotProviderPanel),
}

pub struct GuyEnvEditor {
    pub original_key: Option<String>,
    pub key_input: String,
    pub key_cursor: usize,
    pub value_input: String,
    pub value_cursor: usize,
    pub focus: GuyEnvEditorFocus,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GuyEnvEditorFocus {
    Key,
    Value,
}

pub struct FieldEdit {
    pub selected_field: SetupFieldId,
    pub input: String,
    pub cursor: usize,
}

pub struct SetupProfilePanel {
    pub selected_profile: usize,
    pub editor: Option<SetupProfileEditor>,
    pub message: Option<String>,
}

pub struct SetupProfileEditor {
    pub original_name: Option<String>,
    pub draft: AiProviderProfile,
    pub selected_field: usize,
    pub field_editor: Option<ProfileFieldEdit>,
}

pub struct ProfileFieldEdit {
    pub selected_field: AiProfileFieldId,
    pub input: String,
    pub cursor: usize,
}

pub struct ChatbotProviderPanel {
    pub message: Option<String>,
    pub editor: Option<ChatbotProviderEditor>,
}

pub struct ChatbotProviderEditor {
    pub provider: String,
    pub selected_field: usize,
    pub field_editor: Option<ChatbotFieldEdit>,
}

pub struct ChatbotFieldEdit {
    pub selected_field: ChatbotProviderFieldId,
    pub input: String,
    pub cursor: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum AiProfileFieldId {
    Name,
    Endpoint,
    Apikey,
    Model,
    Debug,
    Vision,
}

impl AiProfileFieldId {
    pub const ALL: [AiProfileFieldId; 6] = [
        AiProfileFieldId::Name,
        AiProfileFieldId::Endpoint,
        AiProfileFieldId::Apikey,
        AiProfileFieldId::Model,
        AiProfileFieldId::Debug,
        AiProfileFieldId::Vision,
    ];

    pub fn from_index(index: usize) -> Self {
        Self::ALL
            .get(index)
            .copied()
            .unwrap_or(AiProfileFieldId::Name)
    }

    pub fn label(self) -> &'static str {
        match self {
            AiProfileFieldId::Name => "profile name",
            AiProfileFieldId::Endpoint => "endpoint",
            AiProfileFieldId::Apikey => "apikey",
            AiProfileFieldId::Model => "model",
            AiProfileFieldId::Debug => "debug",
            AiProfileFieldId::Vision => "image support",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(self, AiProfileFieldId::Debug | AiProfileFieldId::Vision)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ChatbotProviderFieldId {
    Enabled,
    ApiBase,
    Token,
    PollSeconds,
    WhitelistUserIds,
    AppId,
    AppSecret,
    ChatId,
    CdnBase,
    AccountId,
    UserId,
    LongPollTimeoutMs,
}

const TELEGRAM_CHATBOT_FIELDS: [ChatbotProviderFieldId; 5] = [
    ChatbotProviderFieldId::Enabled,
    ChatbotProviderFieldId::ApiBase,
    ChatbotProviderFieldId::Token,
    ChatbotProviderFieldId::PollSeconds,
    ChatbotProviderFieldId::WhitelistUserIds,
];

const FEISHU_CHATBOT_FIELDS: [ChatbotProviderFieldId; 6] = [
    ChatbotProviderFieldId::Enabled,
    ChatbotProviderFieldId::ApiBase,
    ChatbotProviderFieldId::AppId,
    ChatbotProviderFieldId::AppSecret,
    ChatbotProviderFieldId::Token,
    ChatbotProviderFieldId::PollSeconds,
];

const FEISHU_CHATBOT_FIELDS_WITH_CHAT_ID: [ChatbotProviderFieldId; 7] = [
    ChatbotProviderFieldId::Enabled,
    ChatbotProviderFieldId::ApiBase,
    ChatbotProviderFieldId::AppId,
    ChatbotProviderFieldId::AppSecret,
    ChatbotProviderFieldId::Token,
    ChatbotProviderFieldId::PollSeconds,
    ChatbotProviderFieldId::ChatId,
];

const WEIXIN_CHATBOT_FIELDS: [ChatbotProviderFieldId; 9] = [
    ChatbotProviderFieldId::Enabled,
    ChatbotProviderFieldId::ApiBase,
    ChatbotProviderFieldId::CdnBase,
    ChatbotProviderFieldId::Token,
    ChatbotProviderFieldId::AccountId,
    ChatbotProviderFieldId::UserId,
    ChatbotProviderFieldId::WhitelistUserIds,
    ChatbotProviderFieldId::PollSeconds,
    ChatbotProviderFieldId::LongPollTimeoutMs,
];

impl ChatbotProviderFieldId {
    pub fn fields_for(provider: &str) -> &'static [ChatbotProviderFieldId] {
        match provider {
            "telegram" => &TELEGRAM_CHATBOT_FIELDS,
            "feishu" => &FEISHU_CHATBOT_FIELDS_WITH_CHAT_ID,
            "weixin" => &WEIXIN_CHATBOT_FIELDS,
            _ => &FEISHU_CHATBOT_FIELDS,
        }
    }

    pub fn from_provider_index(provider: &str, index: usize) -> Self {
        Self::fields_for(provider)
            .get(index)
            .copied()
            .unwrap_or(ChatbotProviderFieldId::Enabled)
    }

    pub fn label(self, provider: &str) -> &'static str {
        match (provider, self) {
            (_, ChatbotProviderFieldId::Enabled) => "enabled",
            (_, ChatbotProviderFieldId::ApiBase) => "api base",
            ("telegram", ChatbotProviderFieldId::Token) => "bot token",
            ("feishu", ChatbotProviderFieldId::Token) => "access token",
            ("weixin", ChatbotProviderFieldId::Token) => "apikey",
            (_, ChatbotProviderFieldId::PollSeconds) => "poll seconds",
            (_, ChatbotProviderFieldId::WhitelistUserIds) => "whitelist user_ids",
            ("feishu", ChatbotProviderFieldId::AppId) => "app id",
            ("feishu", ChatbotProviderFieldId::AppSecret) => "app secret",
            ("feishu", ChatbotProviderFieldId::ChatId) => "chat id",
            ("weixin", ChatbotProviderFieldId::CdnBase) => "cdn base",
            ("weixin", ChatbotProviderFieldId::AccountId) => "account id",
            ("weixin", ChatbotProviderFieldId::UserId) => "user id",
            ("weixin", ChatbotProviderFieldId::LongPollTimeoutMs) => "long poll timeout ms",
            _ => "value",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(self, ChatbotProviderFieldId::Enabled)
    }

    pub fn is_masked(self) -> bool {
        matches!(
            self,
            ChatbotProviderFieldId::Token | ChatbotProviderFieldId::AppSecret
        )
    }
}

pub enum SubmitOutcome {
    None,
    Quit,
    SendChat(String),
}

pub struct FrontendApp {
    history: Vec<ChatLine>,
    input: String,
    input_cursor: usize,
    selected_command: usize,
    command_history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    mode: Mode,
    pending_chat: bool,
    pending_setup_save: Option<String>,
    thinking_frame: usize,
}

impl FrontendApp {
    pub fn new() -> Self {
        let mut app = Self {
            history: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            selected_command: 0,
            command_history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            mode: Mode::Chat,
            pending_chat: false,
            pending_setup_save: None,
            thinking_frame: 0,
        };
        app.push_system("TUI chat started. Type / for command suggestions.");
        app
    }

    pub fn history(&self) -> &[ChatLine] {
        &self.history
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn selected_command(&self) -> usize {
        self.selected_command
    }

    pub fn input_cursor(&self) -> usize {
        self.input_cursor
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn is_setup_mode(&self) -> bool {
        matches!(
            self.mode,
            Mode::Setup { .. }
                | Mode::GuyEnvEdit { .. }
                | Mode::GuyEnvList { .. }
                | Mode::SubAgent { .. }
                | Mode::CreateSkill { .. }
        )
    }

    pub fn pending_chat_text(&self) -> Option<String> {
        if !self.pending_chat {
            return None;
        }

        let dots = match self.thinking_frame % 4 {
            0 => ".",
            1 => "..",
            2 => "...",
            _ => "",
        };
        Some(format!("thinking{dots}"))
    }

    pub fn pending_setup_save_text(&self) -> Option<&str> {
        self.pending_setup_save.as_deref()
    }

    pub fn is_setup_save_pending(&self) -> bool {
        self.pending_setup_save.is_some()
    }

    pub fn command_suggestions(&self) -> Vec<&'static str> {
        command_suggestions(&self.input)
    }

    pub fn chat_insert(&mut self, c: char) {
        self.exit_history_navigation();
        self.input.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
        self.selected_command = 0;
    }

    pub fn chat_backspace(&mut self) {
        self.exit_history_navigation();
        delete_previous_char(&mut self.input, &mut self.input_cursor);
        self.selected_command = 0;
    }

    pub fn chat_delete(&mut self) {
        self.exit_history_navigation();
        delete_current_char(&mut self.input, self.input_cursor);
        self.selected_command = 0;
    }

    pub fn chat_move_left(&mut self) {
        self.input_cursor = previous_char_boundary(&self.input, self.input_cursor);
    }

    pub fn chat_move_right(&mut self) {
        self.input_cursor = next_char_boundary(&self.input, self.input_cursor);
    }

    pub fn chat_move_home(&mut self) {
        self.input_cursor = 0;
    }

    pub fn chat_move_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    pub fn chat_select_prev(&mut self) {
        if self.command_history.is_empty() {
            return;
        }

        let next_index = match self.history_cursor {
            Some(index) if index > 0 => index - 1,
            Some(_) => 0,
            None => {
                self.history_draft = self.input.clone();
                self.command_history.len() - 1
            }
        };

        self.history_cursor = Some(next_index);
        if let Some(entry) = self.command_history.get(next_index) {
            self.input = entry.clone();
            self.input_cursor = self.input.len();
        }
        self.selected_command = 0;
    }

    pub fn chat_select_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };

        if index + 1 < self.command_history.len() {
            let next_index = index + 1;
            self.history_cursor = Some(next_index);
            if let Some(entry) = self.command_history.get(next_index) {
                self.input = entry.clone();
                self.input_cursor = self.input.len();
            }
        } else {
            self.history_cursor = None;
            self.input = self.history_draft.clone();
            self.input_cursor = self.input.len();
        }
        self.selected_command = 0;
    }

    pub fn chat_select_prev_command(&mut self) {
        let suggestions = self.command_suggestions();
        if suggestions.is_empty() {
            return;
        }

        if self.selected_command == 0 {
            self.selected_command = suggestions.len() - 1;
        } else {
            self.selected_command -= 1;
        }
    }

    pub fn chat_select_next_command(&mut self) {
        let suggestions = self.command_suggestions();
        if suggestions.is_empty() {
            return;
        }

        self.selected_command = (self.selected_command + 1) % suggestions.len();
    }

    pub fn chat_clear(&mut self) {
        self.exit_history_navigation();
        self.input.clear();
        self.input_cursor = 0;
        self.selected_command = 0;
    }

    pub fn submit_chat<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<SubmitOutcome> {
        if self.pending_chat {
            return Ok(SubmitOutcome::None);
        }

        let mut message = self.input.trim().to_string();
        let suggestions = self.command_suggestions();
        if message.starts_with('/') && !suggestions.is_empty() {
            self.clamp_selected_command();
            message = suggestions[self.selected_command].to_string();
        }

        self.input.clear();
        self.input_cursor = 0;
        self.selected_command = 0;
        self.history_cursor = None;
        self.history_draft.clear();

        if message.is_empty() {
            return Ok(SubmitOutcome::None);
        }
        self.push_command_history(&message);
        if matches!(message.as_str(), "/exit" | "/quit") {
            return Ok(SubmitOutcome::Quit);
        }
        if message == "/setup" {
            self.enter_setup(rpc)?;
            return Ok(SubmitOutcome::None);
        }
        if message == "/set-guy-env" {
            self.enter_guy_env_edit(rpc)?;
            return Ok(SubmitOutcome::None);
        }
        if message == "/list-guy-env" {
            self.enter_guy_env_list(rpc)?;
            return Ok(SubmitOutcome::None);
        }
        if message == "/restart-server" {
            self.push_user(&message);
            match rpc.restart_server() {
                Ok(status) => self.push_restart_status_lines(status),
                Err(err) => self.push_system(&format!("restart failed: {err}")),
            }
            return Ok(SubmitOutcome::None);
        }
        if message == "/new" {
            self.start_new_chat_session()?;
            return Ok(SubmitOutcome::None);
        }
        if message == "/sub-agent" {
            self.enter_sub_agent_mode();
            return Ok(SubmitOutcome::None);
        }
        if message == "/create-skill" {
            self.enter_create_skill_mode();
            return Ok(SubmitOutcome::None);
        }
        if let Some(argument) = message.strip_prefix("/set-guy-env ") {
            self.push_user(&message);
            self.handle_set_guy_env(rpc, argument)?;
            return Ok(SubmitOutcome::None);
        }

        self.push_user(&message);
        self.pending_chat = true;
        self.thinking_frame = 0;
        Ok(SubmitOutcome::SendChat(message))
    }

    pub fn enter_setup<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let config = rpc.load_setup()?;
        let selected_provider = config.selected_provider_index();
        self.mode = Mode::Setup {
            selected_field: 0,
            selected_provider,
            overlay: None,
            original_work_dir: config.work_dir.clone(),
            config,
        };
        self.input.clear();
        self.input_cursor = 0;
        Ok(())
    }

    pub fn cancel_setup(&mut self) {
        self.mode = Mode::Chat;
        self.input.clear();
        self.input_cursor = 0;
        self.push_system("Setup canceled.");
    }

    pub fn enter_guy_env_edit<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let entries = rpc.list_guy_env()?;
        self.mode = Mode::GuyEnvEdit {
            selected_entry: 0,
            editor: None,
            entries,
        };
        self.input.clear();
        self.input_cursor = 0;
        Ok(())
    }

    pub fn enter_guy_env_list<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let entries = rpc.list_guy_env()?;
        self.mode = Mode::GuyEnvList {
            selected_entry: 0,
            entries,
        };
        self.input.clear();
        self.input_cursor = 0;
        Ok(())
    }

    pub fn refresh_guy_env<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let entries = rpc.list_guy_env()?;
        match &mut self.mode {
            Mode::GuyEnvEdit {
                selected_entry,
                entries: saved_entries,
                ..
            } => {
                *saved_entries = entries;
                let max_index = saved_entries.len();
                *selected_entry = (*selected_entry).min(max_index);
            }
            Mode::GuyEnvList {
                selected_entry,
                entries: saved_entries,
            } => {
                *saved_entries = entries;
                let max_index = saved_entries.len().saturating_sub(1);
                *selected_entry = (*selected_entry).min(max_index);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn close_panel(&mut self) {
        self.mode = Mode::Chat;
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn begin_setup_save(&mut self) -> Option<SetupConfig> {
        if self.pending_setup_save.is_some() {
            return None;
        }

        let (config, original_work_dir) = match &self.mode {
            Mode::Setup {
                config,
                original_work_dir,
                ..
            } => (config.clone(), original_work_dir.clone()),
            Mode::Chat
            | Mode::GuyEnvEdit { .. }
            | Mode::GuyEnvList { .. }
            | Mode::SubAgent { .. }
            | Mode::CreateSkill { .. } => return None,
        };

        let previous = botty_io::resolve_work_dir_input(&original_work_dir);
        let next = botty_io::resolve_work_dir_input(&config.work_dir);
        self.pending_setup_save = Some(if previous != next {
            "Migrating work dir...".to_string()
        } else {
            "Saving setup...".to_string()
        });
        Some(config)
    }

    pub fn finish_setup_save(&mut self, result: io::Result<SaveSetupResult>) {
        self.pending_setup_save = None;
        match result {
            Ok(result) => {
                self.mode = Mode::Chat;
                self.input.clear();
                self.input_cursor = 0;
                self.push_system(&format!("Setup saved to {}", result.config_path.display()));
                self.push_system(&format!(
                    "Shared work dir config saved to {}",
                    result.work_dir_config_path.display()
                ));
                if let Some((from, to)) = result.migrated_work_dir.as_ref() {
                    self.push_system(&format!(
                        "Work dir migrated: {} -> {}",
                        from.display(),
                        to.display()
                    ));
                }
                self.push_restart_status(result);
            }
            Err(err) => self.push_system(&format!("save setup failed: {err}")),
        }
    }

    pub fn setup_prev_field(&mut self) {
        let Mode::Setup { selected_field, .. } = &mut self.mode else {
            return;
        };
        if *selected_field == 0 {
            *selected_field = SetupFieldId::ALL.len() - 1;
        } else {
            *selected_field -= 1;
        }
    }

    pub fn setup_next_field(&mut self) {
        let Mode::Setup { selected_field, .. } = &mut self.mode else {
            return;
        };
        *selected_field = (*selected_field + 1) % SetupFieldId::ALL.len();
    }

    pub fn setup_activate(&mut self) {
        let Mode::Setup {
            selected_field,
            overlay,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };

        let field = SetupFieldId::from_index(*selected_field);
        if field == SetupFieldId::AiProfiles {
            *overlay = Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                selected_profile: config.active_ai_profile_index(),
                editor: None,
                message: None,
            }));
            return;
        }

        if field == SetupFieldId::ChatbotProvider {
            *overlay = Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                message: None,
                editor: None,
            }));
            return;
        }

        if field.is_toggle() {
            config.toggle_field(field);
            return;
        }

        let input = config.editable_value(field);
        *overlay = Some(SetupOverlay::Field(FieldEdit {
            selected_field: field,
            cursor: input.len(),
            input,
        }));
    }

    pub fn setup_toggle_selected(&mut self) {
        let Mode::Setup {
            selected_field,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };

        let field = SetupFieldId::from_index(*selected_field);
        if field.is_toggle() {
            config.toggle_field(field);
        }
    }

    pub fn setup_open_ai_profiles(&mut self) {
        let Mode::Setup {
            config, overlay, ..
        } = &mut self.mode
        else {
            return;
        };
        *overlay = Some(SetupOverlay::AiProfiles(SetupProfilePanel {
            selected_profile: config.active_ai_profile_index(),
            editor: None,
            message: None,
        }));
    }

    pub fn setup_close_overlay(&mut self) {
        let Mode::Setup { overlay, .. } = &mut self.mode else {
            return;
        };

        if let Some(SetupOverlay::AiProfiles(panel)) = overlay.as_mut() {
            if panel.message.is_some() {
                panel.message = None;
                return;
            }
            if panel.editor.is_some() {
                panel.editor = None;
                return;
            }
        }
        if let Some(SetupOverlay::ChatbotProviders(panel)) = overlay.as_mut() {
            if panel.message.is_some() {
                panel.message = None;
                return;
            }
            if let Some(editor) = panel.editor.as_mut() {
                if editor.field_editor.is_some() {
                    editor.field_editor = None;
                    return;
                }
            }
            if panel.editor.is_some() {
                panel.editor = None;
                return;
            }
        }
        *overlay = None;
    }

    pub fn setup_ai_profiles_prev(&mut self) {
        let profile_count = match &self.mode {
            Mode::Setup { config, .. } => config.ai_provider_profiles.len(),
            _ => 0,
        };
        let Some(panel) = self.setup_ai_profiles_panel_mut() else {
            return;
        };
        let len = profile_count + 1;
        if len == 0 {
            return;
        }
        if panel.selected_profile == 0 {
            panel.selected_profile = len - 1;
        } else {
            panel.selected_profile -= 1;
        }
    }

    pub fn setup_ai_profiles_next(&mut self) {
        let profile_count = match &self.mode {
            Mode::Setup { config, .. } => config.ai_provider_profiles.len(),
            _ => 0,
        };
        let Some(panel) = self.setup_ai_profiles_panel_mut() else {
            return;
        };
        let len = profile_count + 1;
        if len == 0 {
            return;
        }
        panel.selected_profile = (panel.selected_profile + 1) % len;
    }

    pub fn setup_ai_profiles_activate(&mut self) {
        let Mode::Setup {
            config, overlay, ..
        } = &mut self.mode
        else {
            return;
        };
        let Some(SetupOverlay::AiProfiles(panel)) = overlay.as_mut() else {
            return;
        };
        if panel.selected_profile < config.ai_provider_profiles.len() {
            config.activate_ai_profile_by_index(panel.selected_profile);
        }
    }

    pub fn setup_ai_profiles_delete_selected(&mut self) {
        let Mode::Setup {
            config, overlay, ..
        } = &mut self.mode
        else {
            return;
        };
        let Some(SetupOverlay::AiProfiles(panel)) = overlay.as_mut() else {
            return;
        };
        panel.message = None;
        if panel.selected_profile >= config.ai_provider_profiles.len() {
            return;
        }
        if let Err(err) = config.delete_ai_profile(panel.selected_profile) {
            let profile_name = config
                .ai_provider_profiles
                .get(panel.selected_profile)
                .map(|profile| profile.name.clone())
                .unwrap_or_default();
            panel.message = Some(match err.kind() {
                io::ErrorKind::InvalidInput if profile_name == config.ai_provider_active => {
                    "The current profile is active. Switch to another profile before deleting it."
                        .to_string()
                }
                io::ErrorKind::InvalidInput => err.to_string(),
                _ => format!("delete AI profile failed: {err}"),
            });
            return;
        }
        let max_index = config.ai_provider_profiles.len();
        panel.selected_profile = panel.selected_profile.min(max_index);
    }

    pub fn setup_ai_profiles_edit_selected(&mut self) {
        let Mode::Setup {
            config, overlay, ..
        } = &mut self.mode
        else {
            return;
        };
        let Some(SetupOverlay::AiProfiles(panel)) = overlay.as_mut() else {
            return;
        };
        panel.message = None;
        if panel.selected_profile < config.ai_provider_profiles.len() {
            let profile = config.ai_provider_profiles[panel.selected_profile].clone();
            panel.editor = Some(SetupProfileEditor {
                original_name: Some(profile.name.clone()),
                draft: profile,
                selected_field: 0,
                field_editor: None,
            });
        } else {
            panel.editor = Some(SetupProfileEditor {
                original_name: None,
                draft: AiProviderProfile {
                    name: String::new(),
                    endpoint: String::new(),
                    apikey: String::new(),
                    model: String::new(),
                    debug: false,
                    vision: false,
                },
                selected_field: 0,
                field_editor: None,
            });
        }
    }

    pub fn setup_ai_profiles_new(&mut self) {
        let next_index = match &self.mode {
            Mode::Setup { config, .. } => config.ai_provider_profiles.len(),
            _ => return,
        };
        if let Some(panel) = self.setup_ai_profiles_panel_mut() {
            panel.selected_profile = next_index;
            panel.message = None;
        } else {
            self.setup_open_ai_profiles();
            if let Some(panel) = self.setup_ai_profiles_panel_mut() {
                panel.selected_profile = next_index;
                panel.message = None;
            }
        }
        self.setup_ai_profiles_edit_selected();
    }

    pub fn setup_ai_profile_editor_prev_field(&mut self) {
        let Some(editor) = self.setup_ai_profile_editor_mut() else {
            return;
        };
        if editor.selected_field == 0 {
            editor.selected_field = AiProfileFieldId::ALL.len() - 1;
        } else {
            editor.selected_field -= 1;
        }
    }

    pub fn setup_ai_profile_editor_next_field(&mut self) {
        let Some(editor) = self.setup_ai_profile_editor_mut() else {
            return;
        };
        editor.selected_field = (editor.selected_field + 1) % AiProfileFieldId::ALL.len();
    }

    pub fn setup_ai_profile_editor_activate(&mut self) {
        let Some(editor) = self.setup_ai_profile_editor_mut() else {
            return;
        };
        let field = AiProfileFieldId::from_index(editor.selected_field);
        if field.is_toggle() {
            match field {
                AiProfileFieldId::Debug => editor.draft.debug = !editor.draft.debug,
                AiProfileFieldId::Vision => editor.draft.vision = !editor.draft.vision,
                _ => {}
            }
            return;
        }

        let input = match field {
            AiProfileFieldId::Name => editor.draft.name.clone(),
            AiProfileFieldId::Endpoint => editor.draft.endpoint.clone(),
            AiProfileFieldId::Apikey => editor.draft.apikey.clone(),
            AiProfileFieldId::Model => editor.draft.model.clone(),
            AiProfileFieldId::Debug => String::new(),
            AiProfileFieldId::Vision => String::new(),
        };
        editor.field_editor = Some(ProfileFieldEdit {
            selected_field: field,
            cursor: input.len(),
            input,
        });
    }

    pub fn setup_ai_profile_editor_toggle_selected(&mut self) {
        let Some(editor) = self.setup_ai_profile_editor_mut() else {
            return;
        };
        match AiProfileFieldId::from_index(editor.selected_field) {
            AiProfileFieldId::Debug => editor.draft.debug = !editor.draft.debug,
            AiProfileFieldId::Vision => editor.draft.vision = !editor.draft.vision,
            _ => {}
        }
    }

    fn setup_ai_profiles_panel_mut(&mut self) -> Option<&mut SetupProfilePanel> {
        let Mode::Setup { overlay, .. } = &mut self.mode else {
            return None;
        };
        match overlay.as_mut() {
            Some(SetupOverlay::AiProfiles(panel)) => Some(panel),
            _ => None,
        }
    }

    fn setup_ai_profile_editor_mut(&mut self) -> Option<&mut SetupProfileEditor> {
        self.setup_ai_profiles_panel_mut()?.editor.as_mut()
    }

    pub fn setup_chatbot_providers_prev(&mut self) {
        let Mode::Setup {
            selected_provider,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };
        config.cycle_provider(selected_provider, -1);
    }

    pub fn setup_chatbot_providers_next(&mut self) {
        let Mode::Setup {
            selected_provider,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };
        config.cycle_provider(selected_provider, 1);
    }

    pub fn setup_chatbot_providers_edit_selected(&mut self) {
        let provider = match &self.mode {
            Mode::Setup { config, .. } => config.chatbot_provider.clone(),
            _ => return,
        };
        let Some(panel) = self.setup_chatbot_panel_mut() else {
            return;
        };
        panel.message = None;
        panel.editor = Some(ChatbotProviderEditor {
            provider,
            selected_field: 0,
            field_editor: None,
        });
    }

    pub fn setup_chatbot_provider_editor_prev_field(&mut self) {
        let Some(editor) = self.setup_chatbot_provider_editor_mut() else {
            return;
        };
        let len = ChatbotProviderFieldId::fields_for(editor.provider.as_str()).len();
        if editor.selected_field == 0 {
            editor.selected_field = len.saturating_sub(1);
        } else {
            editor.selected_field -= 1;
        }
    }

    pub fn setup_chatbot_provider_editor_next_field(&mut self) {
        let Some(editor) = self.setup_chatbot_provider_editor_mut() else {
            return;
        };
        let len = ChatbotProviderFieldId::fields_for(editor.provider.as_str()).len();
        if len == 0 {
            return;
        }
        editor.selected_field = (editor.selected_field + 1) % len;
    }

    pub fn setup_chatbot_provider_editor_activate(&mut self) {
        let (provider, selected_field) = match self.setup_chatbot_provider_editor_mut() {
            Some(editor) => (editor.provider.clone(), editor.selected_field),
            None => return,
        };
        let field = ChatbotProviderFieldId::from_provider_index(provider.as_str(), selected_field);
        if field.is_toggle() {
            self.setup_chatbot_provider_editor_toggle_selected();
            return;
        }

        let input = match &self.mode {
            Mode::Setup { config, .. } => {
                chatbot_provider_field_value(config, provider.as_str(), field)
            }
            _ => String::new(),
        };

        let Some(editor) = self.setup_chatbot_provider_editor_mut() else {
            return;
        };
        editor.field_editor = Some(ChatbotFieldEdit {
            selected_field: field,
            cursor: input.len(),
            input,
        });
    }

    pub fn setup_chatbot_provider_editor_toggle_selected(&mut self) {
        let (provider, selected_field) = match self.setup_chatbot_provider_editor_mut() {
            Some(editor) => (editor.provider.clone(), editor.selected_field),
            None => return,
        };
        let field = ChatbotProviderFieldId::from_provider_index(provider.as_str(), selected_field);
        if !field.is_toggle() {
            return;
        }
        if let Mode::Setup { config, .. } = &mut self.mode {
            toggle_chatbot_provider_field(config, provider.as_str(), field);
        }
    }

    fn setup_chatbot_panel_mut(&mut self) -> Option<&mut ChatbotProviderPanel> {
        let Mode::Setup { overlay, .. } = &mut self.mode else {
            return None;
        };
        match overlay.as_mut() {
            Some(SetupOverlay::ChatbotProviders(panel)) => Some(panel),
            _ => None,
        }
    }

    fn setup_chatbot_provider_editor_mut(&mut self) -> Option<&mut ChatbotProviderEditor> {
        self.setup_chatbot_panel_mut()?.editor.as_mut()
    }

    pub fn guy_env_prev_entry(&mut self) {
        match &mut self.mode {
            Mode::GuyEnvEdit {
                selected_entry,
                entries,
                ..
            } => {
                let len = entries.len() + 1;
                if len == 0 {
                    return;
                }
                if *selected_entry == 0 {
                    *selected_entry = len - 1;
                } else {
                    *selected_entry -= 1;
                }
            }
            Mode::GuyEnvList {
                selected_entry,
                entries,
            } => {
                if entries.is_empty() {
                    return;
                }
                if *selected_entry == 0 {
                    *selected_entry = entries.len() - 1;
                } else {
                    *selected_entry -= 1;
                }
            }
            _ => {}
        }
    }

    pub fn guy_env_next_entry(&mut self) {
        match &mut self.mode {
            Mode::GuyEnvEdit {
                selected_entry,
                entries,
                ..
            } => {
                let len = entries.len() + 1;
                if len == 0 {
                    return;
                }
                *selected_entry = (*selected_entry + 1) % len;
            }
            Mode::GuyEnvList {
                selected_entry,
                entries,
            } => {
                if entries.is_empty() {
                    return;
                }
                *selected_entry = (*selected_entry + 1) % entries.len();
            }
            _ => {}
        }
    }

    pub fn open_guy_env_editor(&mut self) {
        let Mode::GuyEnvEdit {
            selected_entry,
            editor,
            entries,
        } = &mut self.mode
        else {
            return;
        };

        let new_editor = if *selected_entry < entries.len() {
            let (key, value) = entries[*selected_entry].clone();
            GuyEnvEditor {
                original_key: Some(key.clone()),
                key_input: key,
                key_cursor: entries[*selected_entry].0.len(),
                value_input: value,
                value_cursor: entries[*selected_entry].1.len(),
                focus: GuyEnvEditorFocus::Value,
            }
        } else {
            GuyEnvEditor {
                original_key: None,
                key_input: String::new(),
                key_cursor: 0,
                value_input: String::new(),
                value_cursor: 0,
                focus: GuyEnvEditorFocus::Key,
            }
        };
        *editor = Some(new_editor);
    }

    pub fn editor_cancel(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => {
                if let Some(SetupOverlay::AiProfiles(panel)) = overlay.as_mut() {
                    if let Some(editor) = panel.editor.as_mut() {
                        if editor.field_editor.is_some() {
                            editor.field_editor = None;
                            return;
                        }
                    }
                    if panel.editor.is_some() {
                        panel.editor = None;
                        return;
                    }
                }
                if let Some(SetupOverlay::ChatbotProviders(panel)) = overlay.as_mut() {
                    if let Some(editor) = panel.editor.as_mut() {
                        if editor.field_editor.is_some() {
                            editor.field_editor = None;
                            return;
                        }
                    }
                    if panel.editor.is_some() {
                        panel.editor = None;
                        return;
                    }
                }
                *overlay = None;
            }
            Mode::GuyEnvEdit { editor, .. } => *editor = None,
            _ => {}
        }
    }

    pub fn editor_backspace(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    delete_previous_char(&mut field.input, &mut field.cursor);
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            delete_previous_char(&mut field.input, &mut field.cursor);
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            delete_previous_char(&mut field.input, &mut field.cursor);
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            delete_previous_char(&mut editor.key_input, &mut editor.key_cursor)
                        }
                        GuyEnvEditorFocus::Value => {
                            delete_previous_char(&mut editor.value_input, &mut editor.value_cursor)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_delete(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    delete_current_char(&mut field.input, field.cursor);
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            delete_current_char(&mut field.input, field.cursor);
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            delete_current_char(&mut field.input, field.cursor);
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            delete_current_char(&mut editor.key_input, editor.key_cursor)
                        }
                        GuyEnvEditorFocus::Value => {
                            delete_current_char(&mut editor.value_input, editor.value_cursor)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_insert(&mut self, c: char) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    field.input.insert(field.cursor, c);
                    field.cursor += c.len_utf8();
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.input.insert(field.cursor, c);
                            field.cursor += c.len_utf8();
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.input.insert(field.cursor, c);
                            field.cursor += c.len_utf8();
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            editor.key_input.insert(editor.key_cursor, c);
                            editor.key_cursor += c.len_utf8();
                        }
                        GuyEnvEditorFocus::Value => {
                            editor.value_input.insert(editor.value_cursor, c);
                            editor.value_cursor += c.len_utf8();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_move_left(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    field.cursor = previous_char_boundary(&field.input, field.cursor);
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = previous_char_boundary(&field.input, field.cursor);
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = previous_char_boundary(&field.input, field.cursor);
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            editor.key_cursor =
                                previous_char_boundary(&editor.key_input, editor.key_cursor);
                        }
                        GuyEnvEditorFocus::Value => {
                            editor.value_cursor =
                                previous_char_boundary(&editor.value_input, editor.value_cursor);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_move_right(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    field.cursor = next_char_boundary(&field.input, field.cursor);
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = next_char_boundary(&field.input, field.cursor);
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = next_char_boundary(&field.input, field.cursor);
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            editor.key_cursor =
                                next_char_boundary(&editor.key_input, editor.key_cursor);
                        }
                        GuyEnvEditorFocus::Value => {
                            editor.value_cursor =
                                next_char_boundary(&editor.value_input, editor.value_cursor);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_move_home(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => field.cursor = 0,
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = 0;
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = 0;
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => editor.key_cursor = 0,
                        GuyEnvEditorFocus::Value => editor.value_cursor = 0,
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_move_end(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => field.cursor = field.input.len(),
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = field.input.len();
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.cursor = field.input.len();
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => editor.key_cursor = editor.key_input.len(),
                        GuyEnvEditorFocus::Value => editor.value_cursor = editor.value_input.len(),
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_clear(&mut self) {
        match &mut self.mode {
            Mode::Setup { overlay, .. } => match overlay.as_mut() {
                Some(SetupOverlay::Field(field)) => {
                    field.input.clear();
                    field.cursor = 0;
                }
                Some(SetupOverlay::AiProfiles(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.input.clear();
                            field.cursor = 0;
                        }
                    }
                }
                Some(SetupOverlay::ChatbotProviders(panel)) => {
                    if let Some(editor) = panel.editor.as_mut() {
                        if let Some(field) = editor.field_editor.as_mut() {
                            field.input.clear();
                            field.cursor = 0;
                        }
                    }
                }
                None => {}
            },
            Mode::GuyEnvEdit { editor, .. } => {
                if let Some(editor) = editor.as_mut() {
                    match editor.focus {
                        GuyEnvEditorFocus::Key => {
                            editor.key_input.clear();
                            editor.key_cursor = 0;
                        }
                        GuyEnvEditorFocus::Value => {
                            editor.value_input.clear();
                            editor.value_cursor = 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editor_submit<R: FrontendRpc>(&mut self, rpc: &mut R) {
        match &mut self.mode {
            Mode::Setup {
                overlay, config, ..
            } => {
                let mut close_overlay = false;
                match overlay.as_mut() {
                    Some(SetupOverlay::Field(field)) => {
                        let value = field.input.trim().to_string();
                        if !value.is_empty() || field.selected_field == SetupFieldId::WorkDir {
                            config.set_field(field.selected_field, &value);
                        }
                        close_overlay = true;
                    }
                    Some(SetupOverlay::AiProfiles(panel)) => {
                        if let Some(editor) = panel.editor.as_mut() {
                            if let Some(field) = editor.field_editor.take() {
                                match field.selected_field {
                                    AiProfileFieldId::Name => editor.draft.name = field.input,
                                    AiProfileFieldId::Endpoint => {
                                        editor.draft.endpoint = field.input;
                                    }
                                    AiProfileFieldId::Apikey => editor.draft.apikey = field.input,
                                    AiProfileFieldId::Model => editor.draft.model = field.input,
                                    AiProfileFieldId::Debug => {}
                                    AiProfileFieldId::Vision => {}
                                }
                                return;
                            }

                            match config.upsert_ai_profile(
                                editor.original_name.as_deref(),
                                editor.draft.clone(),
                            ) {
                                Ok(()) => {
                                    let selected_name = editor.draft.name.clone();
                                    panel.editor = None;
                                    panel.selected_profile = config
                                        .ai_provider_profiles
                                        .iter()
                                        .position(|profile| profile.name == selected_name)
                                        .unwrap_or_else(|| config.active_ai_profile_index());
                                    panel.message = None;
                                }
                                Err(err) => panel.message = Some(format!("{err}")),
                            }
                        }
                    }
                    Some(SetupOverlay::ChatbotProviders(panel)) => {
                        if let Some(editor) = panel.editor.as_mut() {
                            if let Some(field) = editor.field_editor.take() {
                                set_chatbot_provider_field(
                                    config,
                                    editor.provider.as_str(),
                                    field.selected_field,
                                    field.input.as_str(),
                                );
                                return;
                            }

                            panel.editor = None;
                            panel.message = None;
                        }
                    }
                    None => {}
                }

                if close_overlay {
                    *overlay = None;
                }
            }
            Mode::GuyEnvEdit {
                selected_entry,
                editor,
                entries,
            } => {
                let Some(editor_state) = editor.take() else {
                    return;
                };

                let key = editor_state.key_input.trim().to_string();
                let value = editor_state.value_input.trim().to_string();
                if key.is_empty() {
                    self.push_system("Guy env key cannot be empty.");
                    return;
                }

                match rpc.set_guy_env(&key, &value) {
                    Ok(result) => {
                        if let Some(original_key) = editor_state.original_key {
                            entries.retain(|(saved_key, _)| saved_key != &original_key);
                        }
                        if let Some((_, saved_value)) =
                            entries.iter_mut().find(|(saved_key, _)| saved_key == &key)
                        {
                            *saved_value = value.clone();
                        } else {
                            entries.push((key.clone(), value.clone()));
                        }
                        entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                        *selected_entry = entries
                            .iter()
                            .position(|(saved_key, _)| saved_key == &key)
                            .unwrap_or(entries.len());
                        self.push_guy_env_set_status(result);
                    }
                    Err(err) => self.push_system(&format!("set guy env failed: {err}")),
                }
            }
            _ => {}
        }
    }

    pub fn push_user(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::User,
            text: text.to_string(),
        });
    }

    pub fn push_bot(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::Bot,
            text: text.to_string(),
        });
    }

    pub fn push_system(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::System,
            text: text.to_string(),
        });
    }

    pub fn tick(&mut self) {
        if self.pending_chat {
            self.thinking_frame = self.thinking_frame.wrapping_add(1);
        }
    }

    pub fn finish_chat_request(&mut self, result: io::Result<String>) {
        self.pending_chat = false;
        self.thinking_frame = 0;
        match result {
            Ok(reply) => self.push_bot(&reply),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                self.push_system("request interrupted.")
            }
            Err(err) => self.push_system(&format!("request failed: {err}")),
        }
    }

    fn clamp_selected_command(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.selected_command = 0;
        } else {
            self.selected_command = self.selected_command.min(len - 1);
        }
    }

    fn start_new_chat_session(&mut self) -> io::Result<()> {
        write_new_session_marker()?;
        self.history.clear();
        self.input.clear();
        self.input_cursor = 0;
        self.selected_command = 0;
        self.history_cursor = None;
        self.history_draft.clear();
        self.pending_chat = false;
        self.thinking_frame = 0;
        self.push_system("Started a new chat session. Older history will not be sent.");
        Ok(())
    }

    fn exit_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft.clear();
    }

    fn push_command_history(&mut self, message: &str) {
        if self
            .command_history
            .last()
            .is_some_and(|last| last == message)
        {
            return;
        }

        if self.command_history.len() == 100 {
            self.command_history.remove(0);
        }
        self.command_history.push(message.to_string());
    }

    fn push_restart_status(&mut self, result: SaveSetupResult) {
        self.push_restart_status_lines(result.restart_status);
    }

    fn push_guy_env_set_status(&mut self, result: GuyEnvSetResult) {
        self.push_system(&format!(
            "Guy env saved to {}",
            result.config_path.display()
        ));
        if result.applied_live {
            self.push_system("Applied to the running guy process.");
        } else {
            self.push_system("Saved, but live apply failed. It will be used next time guy starts.");
        }
    }

    fn push_restart_status_lines(&mut self, status: RestartStatus) {
        match status {
            RestartStatus::Success(message) | RestartStatus::Failed(message) => {
                for line in message.lines().filter(|line| !line.trim().is_empty()) {
                    self.push_system(line);
                }
            }
        }
    }

    fn handle_set_guy_env<R: FrontendRpc>(
        &mut self,
        rpc: &mut R,
        argument: &str,
    ) -> io::Result<()> {
        let Some((key, value)) = argument.split_once('=') else {
            self.push_system("Usage: /set-guy-env KEY=VALUE");
            return Ok(());
        };

        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            self.push_system("Usage: /set-guy-env KEY=VALUE");
            return Ok(());
        }

        match rpc.set_guy_env(key, value) {
            Ok(result) => {
                self.push_system(&format!("Set guy env {key}={value}"));
                self.push_guy_env_set_status(result);
            }
            Err(err) => self.push_system(&format!("set guy env failed: {err}")),
        }
        Ok(())
    }

    pub fn guy_env_editor(&self) -> Option<&GuyEnvEditor> {
        match &self.mode {
            Mode::GuyEnvEdit { editor, .. } => editor.as_ref(),
            _ => None,
        }
    }

    pub fn guy_env_edit_state(&self) -> Option<(usize, &[(String, String)])> {
        match &self.mode {
            Mode::GuyEnvEdit {
                selected_entry,
                entries,
                ..
            } => Some((*selected_entry, entries)),
            _ => None,
        }
    }

    pub fn guy_env_list_state(&self) -> Option<(usize, &[(String, String)])> {
        match &self.mode {
            Mode::GuyEnvList {
                selected_entry,
                entries,
            } => Some((*selected_entry, entries)),
            _ => None,
        }
    }

    pub fn toggle_guy_env_editor_focus(&mut self) {
        let Mode::GuyEnvEdit { editor, .. } = &mut self.mode else {
            return;
        };
        if let Some(editor) = editor.as_mut() {
            editor.focus = match editor.focus {
                GuyEnvEditorFocus::Key => GuyEnvEditorFocus::Value,
                GuyEnvEditorFocus::Value => GuyEnvEditorFocus::Key,
            };
        }
    }

    // --- Sub-agent mode ---

    pub fn enter_sub_agent_mode(&mut self) {
        let configs = load_all_custom_role_configs();
        let agents: Vec<SubAgentEntry> = configs
            .into_iter()
            .map(|c| SubAgentEntry {
                name: c.name,
                description: c.description,
                skills: c.skills,
            })
            .collect();
        self.mode = Mode::SubAgent {
            agents,
            selected_entry: 0,
            editor: None,
            confirm_delete: None,
        };
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn sub_agent_state(
        &self,
    ) -> Option<(
        usize,
        &[SubAgentEntry],
        Option<&SubAgentEditor>,
        Option<&SubAgentDeleteConfirm>,
    )> {
        match &self.mode {
            Mode::SubAgent {
                agents,
                selected_entry,
                editor,
                confirm_delete,
            } => Some((
                *selected_entry,
                agents,
                editor.as_ref(),
                confirm_delete.as_ref(),
            )),
            _ => None,
        }
    }

    pub fn sub_agent_prev_entry(&mut self) {
        let Mode::SubAgent {
            selected_entry,
            agents,
            confirm_delete,
            ..
        } = &mut self.mode
        else {
            return;
        };
        if confirm_delete.is_some() {
            return;
        }
        let len = agents.len() + 1; // +1 for [Create new]
        if len == 0 {
            return;
        }
        if *selected_entry == 0 {
            *selected_entry = len - 1;
        } else {
            *selected_entry -= 1;
        }
    }

    pub fn sub_agent_next_entry(&mut self) {
        let Mode::SubAgent {
            selected_entry,
            agents,
            confirm_delete,
            ..
        } = &mut self.mode
        else {
            return;
        };
        if confirm_delete.is_some() {
            return;
        }
        let len = agents.len() + 1;
        *selected_entry = (*selected_entry + 1) % len;
    }

    pub fn open_sub_agent_editor(&mut self) {
        let Mode::SubAgent {
            selected_entry,
            agents,
            editor,
            confirm_delete,
        } = &mut self.mode
        else {
            return;
        };
        if confirm_delete.is_some() {
            return;
        }

        let available_skills = all_available_skill_names();
        let new_editor = if *selected_entry < agents.len() {
            let agent = &agents[*selected_entry];
            let selected_skills: Vec<bool> = available_skills
                .iter()
                .map(|s| agent.skills.contains(s))
                .collect();
            SubAgentEditor {
                original_name: Some(agent.name.clone()),
                name_input: agent.name.clone(),
                name_cursor: agent.name.len(),
                description_input: agent.description.clone(),
                description_cursor: agent.description.len(),
                available_skills,
                selected_skills,
                skill_scroll: 0,
                focus: SubAgentEditorFocus::Name,
                generating_description: false,
            }
        } else {
            let selected_skills = vec![false; available_skills.len()];
            SubAgentEditor {
                original_name: None,
                name_input: String::new(),
                name_cursor: 0,
                description_input: String::new(),
                description_cursor: 0,
                available_skills,
                selected_skills,
                skill_scroll: 0,
                focus: SubAgentEditorFocus::Name,
                generating_description: false,
            }
        };
        *editor = Some(new_editor);
    }

    pub fn sub_agent_request_delete(&mut self) {
        let Mode::SubAgent {
            agents,
            selected_entry,
            editor,
            confirm_delete,
        } = &mut self.mode
        else {
            return;
        };
        if editor.is_some() {
            return;
        }
        if *selected_entry >= agents.len() {
            self.push_system("Select an existing sub-agent to delete.");
            return;
        }
        *confirm_delete = Some(SubAgentDeleteConfirm {
            name: agents[*selected_entry].name.clone(),
        });
    }

    pub fn sub_agent_cancel_delete(&mut self) {
        let Mode::SubAgent { confirm_delete, .. } = &mut self.mode else {
            return;
        };
        *confirm_delete = None;
    }

    pub fn sub_agent_confirm_delete(&mut self) {
        let Mode::SubAgent {
            agents,
            selected_entry,
            confirm_delete,
            ..
        } = &mut self.mode
        else {
            return;
        };
        let Some(confirm) = confirm_delete.take() else {
            return;
        };

        match delete_custom_role_config(&confirm.name) {
            Ok(()) => {
                agents.retain(|agent| agent.name != confirm.name);
                let len = agents.len() + 1;
                if *selected_entry >= len {
                    *selected_entry = len.saturating_sub(1);
                }
                self.push_system(&format!("Sub-agent '{}' deleted.", confirm.name));
            }
            Err(err) => {
                self.push_system(&format!("delete sub-agent failed: {err}"));
            }
        }
    }

    pub fn sub_agent_editor_toggle_skill(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        if editor.focus != SubAgentEditorFocus::Skills {
            return;
        }
        if editor.skill_scroll < editor.selected_skills.len() {
            editor.selected_skills[editor.skill_scroll] =
                !editor.selected_skills[editor.skill_scroll];
        }
    }

    pub fn sub_agent_editor_skill_prev(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        if editor.focus == SubAgentEditorFocus::Skills && editor.skill_scroll > 0 {
            editor.skill_scroll -= 1;
        }
    }

    pub fn sub_agent_editor_skill_next(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        if editor.focus == SubAgentEditorFocus::Skills
            && editor.skill_scroll + 1 < editor.available_skills.len()
        {
            editor.skill_scroll += 1;
        }
    }

    pub fn sub_agent_editor_cycle_focus(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        editor.focus = match editor.focus {
            SubAgentEditorFocus::Name => SubAgentEditorFocus::Skills,
            SubAgentEditorFocus::Skills => SubAgentEditorFocus::Description,
            SubAgentEditorFocus::Description => SubAgentEditorFocus::Name,
        };
    }

    pub fn sub_agent_editor_insert(&mut self, c: char) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                editor.name_input.insert(editor.name_cursor, c);
                editor.name_cursor += c.len_utf8();
            }
            SubAgentEditorFocus::Description => {
                editor
                    .description_input
                    .insert(editor.description_cursor, c);
                editor.description_cursor += c.len_utf8();
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_backspace(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                delete_previous_char(&mut editor.name_input, &mut editor.name_cursor);
            }
            SubAgentEditorFocus::Description => {
                delete_previous_char(
                    &mut editor.description_input,
                    &mut editor.description_cursor,
                );
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_delete(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                delete_current_char(&mut editor.name_input, editor.name_cursor);
            }
            SubAgentEditorFocus::Description => {
                delete_current_char(&mut editor.description_input, editor.description_cursor);
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_move_left(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                editor.name_cursor = previous_char_boundary(&editor.name_input, editor.name_cursor);
            }
            SubAgentEditorFocus::Description => {
                editor.description_cursor =
                    previous_char_boundary(&editor.description_input, editor.description_cursor);
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_move_right(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                editor.name_cursor = next_char_boundary(&editor.name_input, editor.name_cursor);
            }
            SubAgentEditorFocus::Description => {
                editor.description_cursor =
                    next_char_boundary(&editor.description_input, editor.description_cursor);
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_move_home(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => editor.name_cursor = 0,
            SubAgentEditorFocus::Description => editor.description_cursor = 0,
            SubAgentEditorFocus::Skills => editor.skill_scroll = 0,
        }
    }

    pub fn sub_agent_editor_clear(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        match editor.focus {
            SubAgentEditorFocus::Name => {
                editor.name_input.clear();
                editor.name_cursor = 0;
            }
            SubAgentEditorFocus::Description => {
                editor.description_input.clear();
                editor.description_cursor = 0;
            }
            SubAgentEditorFocus::Skills => {}
        }
    }

    pub fn sub_agent_editor_save(&mut self) {
        let Mode::SubAgent { agents, editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor_state) = editor.take() else {
            return;
        };

        let original_name = editor_state.original_name.clone();
        let name = editor_state.name_input.trim().to_string();
        if name.is_empty() {
            self.push_system("Sub-agent name cannot be empty.");
            return;
        }

        let selected_skills: Vec<String> = editor_state
            .available_skills
            .iter()
            .zip(editor_state.selected_skills.iter())
            .filter(|(_, selected)| **selected)
            .map(|(skill, _)| skill.clone())
            .collect();

        let config = CustomRoleConfig {
            name: name.clone(),
            description: editor_state.description_input.trim().to_string(),
            skills: selected_skills.clone(),
        };

        match save_custom_role_config(&config) {
            Ok(path) => {
                let mut cleanup_error = None;
                if let Some(original_name) = original_name
                    .as_ref()
                    .filter(|original_name| **original_name != name)
                {
                    if let Err(err) = delete_custom_role_config(original_name) {
                        cleanup_error = Some(format!("delete old sub-agent config failed: {err}"));
                    }
                }

                // Update local list
                if let Some(existing) = agents.iter_mut().find(|a| {
                    a.name == name
                        || original_name
                            .as_ref()
                            .is_some_and(|original_name| a.name == *original_name)
                }) {
                    existing.name = name.clone();
                    existing.description = config.description;
                    existing.skills = selected_skills;
                } else {
                    agents.push(SubAgentEntry {
                        name,
                        description: config.description,
                        skills: selected_skills,
                    });
                }
                if let Some(message) = cleanup_error {
                    self.push_system(&message);
                }
                self.push_system(&format!("Sub-agent saved to {}", path.display()));
            }
            Err(err) => self.push_system(&format!("save sub-agent failed: {err}")),
        }
    }

    pub fn sub_agent_editor_cancel(&mut self) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        *editor = None;
    }

    pub fn sub_agent_editor_generate_description(&mut self) -> Option<(String, Vec<String>)> {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return None;
        };
        let Some(editor) = editor.as_mut() else {
            return None;
        };
        let name = editor.name_input.trim().to_string();
        let selected_skills: Vec<String> = editor
            .available_skills
            .iter()
            .zip(editor.selected_skills.iter())
            .filter(|(_, selected)| **selected)
            .map(|(skill, _)| skill.clone())
            .collect();
        if name.is_empty() || selected_skills.is_empty() {
            return None;
        }
        editor.generating_description = true;
        Some((name, selected_skills))
    }

    pub fn sub_agent_editor_set_description(&mut self, description: String) {
        let Mode::SubAgent { editor, .. } = &mut self.mode else {
            return;
        };
        let Some(editor) = editor.as_mut() else {
            return;
        };
        editor.description_input = description;
        editor.description_cursor = editor.description_input.len();
        editor.generating_description = false;
    }

    // --- Create skill mode ---

    pub fn enter_create_skill_mode(&mut self) {
        self.mode = Mode::CreateSkill {
            editor: CreateSkillEditor {
                name_input: String::new(),
                name_cursor: 0,
                purpose_input: String::new(),
                purpose_cursor: 0,
                focus: CreateSkillEditorFocus::Name,
                generated_description: String::new(),
                generating_description: false,
                pending_generated_name: String::new(),
                pending_generated_purpose: String::new(),
            },
        };
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn create_skill_editor(&self) -> Option<&CreateSkillEditor> {
        match &self.mode {
            Mode::CreateSkill { editor } => Some(editor),
            _ => None,
        }
    }

    pub fn create_skill_preview(&self) -> Option<(String, String, String, bool)> {
        let Mode::CreateSkill { editor } = &self.mode else {
            return None;
        };
        let normalized_name = normalize_skill_name(editor.name_input.trim());
        let target_path = if normalized_name.is_empty() {
            "~/.mylittlebotty/skill/<name>.json".to_string()
        } else {
            format!("~/.mylittlebotty/skill/{normalized_name}.json")
        };
        Some((
            normalized_name,
            editor.generated_description.clone(),
            target_path,
            editor.generating_description,
        ))
    }

    pub fn create_skill_editor_insert(&mut self, c: char) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        {
            let (input, cursor) = create_skill_active_field(editor);
            input.insert(*cursor, c);
            *cursor += c.len_utf8();
        }
        editor.generated_description.clear();
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_backspace(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        {
            let (input, cursor) = create_skill_active_field(editor);
            delete_previous_char(input, cursor);
        }
        editor.generated_description.clear();
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_delete(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        {
            let (input, cursor) = create_skill_active_field(editor);
            delete_current_char(input, *cursor);
        }
        editor.generated_description.clear();
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_move_left(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        *cursor = previous_char_boundary(input, *cursor);
    }

    pub fn create_skill_editor_move_right(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        *cursor = next_char_boundary(input, *cursor);
    }

    pub fn create_skill_editor_move_home(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (_, cursor) = create_skill_active_field(editor);
        *cursor = 0;
    }

    pub fn create_skill_editor_cycle_focus(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        editor.focus = match editor.focus {
            CreateSkillEditorFocus::Name => CreateSkillEditorFocus::Purpose,
            CreateSkillEditorFocus::Purpose => CreateSkillEditorFocus::Name,
        };
    }

    pub fn create_skill_editor_clear(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        {
            let (input, cursor) = create_skill_active_field(editor);
            input.clear();
            *cursor = 0;
        }
        editor.generated_description.clear();
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_begin_generate(&mut self) -> Option<(String, String)> {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return None;
        };
        let name = normalize_skill_name(&editor.name_input);
        let purpose = editor.purpose_input.trim().to_string();
        if name.is_empty() || purpose.is_empty() || editor.generating_description {
            return None;
        }
        editor.generating_description = true;
        editor.pending_generated_name = name.clone();
        editor.pending_generated_purpose = purpose.clone();
        Some((name, purpose))
    }

    pub fn create_skill_editor_set_generated_description(
        &mut self,
        name: String,
        purpose: String,
        description: String,
    ) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        if editor.pending_generated_name != name || editor.pending_generated_purpose != purpose {
            return;
        }
        editor.generated_description = description;
        editor.generating_description = false;
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_generation_failed(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        editor.generating_description = false;
        editor.pending_generated_name.clear();
        editor.pending_generated_purpose.clear();
    }

    pub fn create_skill_editor_save(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let name = normalize_skill_name(&editor.name_input);
        let purpose = editor.purpose_input.trim().to_string();
        if name.is_empty() {
            self.push_system("Skill name cannot be empty.");
            return;
        }
        if purpose.is_empty() {
            self.push_system("Skill purpose cannot be empty.");
            return;
        }

        match custom_skill::save_generated_custom_skill(
            &name,
            &purpose,
            if editor.generated_description.trim().is_empty() {
                None
            } else {
                Some(editor.generated_description.trim())
            },
        ) {
            Ok(path) => {
                self.mode = Mode::Chat;
                self.push_system(&format!("Custom skill saved to {}", path.display()));
            }
            Err(err) => self.push_system(&format!("save custom skill failed: {err}")),
        }
    }
}

fn chatbot_provider_field_value(
    config: &SetupConfig,
    provider: &str,
    field: ChatbotProviderFieldId,
) -> String {
    match (provider, field) {
        ("telegram", ChatbotProviderFieldId::Enabled) => {
            checkbox_value(config.chatbot_telegram_enabled)
        }
        ("telegram", ChatbotProviderFieldId::ApiBase) => config.chatbot_telegram_api_base.clone(),
        ("telegram", ChatbotProviderFieldId::Token) => config.chatbot_telegram_apikey.clone(),
        ("telegram", ChatbotProviderFieldId::PollSeconds) => {
            config.chatbot_telegram_poll_interval_seconds.to_string()
        }
        ("telegram", ChatbotProviderFieldId::WhitelistUserIds) => {
            config.chatbot_telegram_whitelist_user_ids.clone()
        }
        ("feishu", ChatbotProviderFieldId::Enabled) => {
            checkbox_value(config.chatbot_feishu_enabled)
        }
        ("feishu", ChatbotProviderFieldId::ApiBase) => config.chatbot_feishu_api_base.clone(),
        ("feishu", ChatbotProviderFieldId::AppId) => config.chatbot_feishu_app_id.clone(),
        ("feishu", ChatbotProviderFieldId::AppSecret) => config.chatbot_feishu_app_secret.clone(),
        ("feishu", ChatbotProviderFieldId::Token) => config.chatbot_feishu_access_token.clone(),
        ("feishu", ChatbotProviderFieldId::PollSeconds) => {
            config.chatbot_feishu_poll_interval_seconds.to_string()
        }
        ("feishu", ChatbotProviderFieldId::ChatId) => config.chatbot_feishu_chat_id.clone(),
        ("weixin", ChatbotProviderFieldId::Enabled) => {
            checkbox_value(config.chatbot_weixin_enabled)
        }
        ("weixin", ChatbotProviderFieldId::ApiBase) => config.chatbot_weixin_api_base.clone(),
        ("weixin", ChatbotProviderFieldId::CdnBase) => config.chatbot_weixin_cdn_base.clone(),
        ("weixin", ChatbotProviderFieldId::Token) => config.chatbot_weixin_apikey.clone(),
        ("weixin", ChatbotProviderFieldId::AccountId) => config.chatbot_weixin_account_id.clone(),
        ("weixin", ChatbotProviderFieldId::UserId) => config.chatbot_weixin_user_id.clone(),
        ("weixin", ChatbotProviderFieldId::WhitelistUserIds) => {
            config.chatbot_weixin_whitelist_user_ids.clone()
        }
        ("weixin", ChatbotProviderFieldId::PollSeconds) => {
            config.chatbot_weixin_poll_interval_seconds.to_string()
        }
        ("weixin", ChatbotProviderFieldId::LongPollTimeoutMs) => {
            config.chatbot_weixin_long_poll_timeout_ms.to_string()
        }
        _ => String::new(),
    }
}

fn set_chatbot_provider_field(
    config: &mut SetupConfig,
    provider: &str,
    field: ChatbotProviderFieldId,
    value: &str,
) {
    match (provider, field) {
        ("telegram", ChatbotProviderFieldId::ApiBase) => {
            config.chatbot_telegram_api_base = value.to_string()
        }
        ("telegram", ChatbotProviderFieldId::Token) => {
            config.chatbot_telegram_apikey = value.to_string()
        }
        ("telegram", ChatbotProviderFieldId::PollSeconds) => {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                config.chatbot_telegram_poll_interval_seconds = seconds.max(1);
            }
        }
        ("telegram", ChatbotProviderFieldId::WhitelistUserIds) => {
            config.chatbot_telegram_whitelist_user_ids = value.to_string()
        }
        ("feishu", ChatbotProviderFieldId::ApiBase) => {
            config.chatbot_feishu_api_base = value.to_string()
        }
        ("feishu", ChatbotProviderFieldId::AppId) => {
            config.chatbot_feishu_app_id = value.to_string()
        }
        ("feishu", ChatbotProviderFieldId::AppSecret) => {
            config.chatbot_feishu_app_secret = value.to_string()
        }
        ("feishu", ChatbotProviderFieldId::Token) => {
            config.chatbot_feishu_access_token = value.to_string()
        }
        ("feishu", ChatbotProviderFieldId::PollSeconds) => {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                config.chatbot_feishu_poll_interval_seconds = seconds.max(1);
            }
        }
        ("feishu", ChatbotProviderFieldId::ChatId) => {
            config.chatbot_feishu_chat_id = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::ApiBase) => {
            config.chatbot_weixin_api_base = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::CdnBase) => {
            config.chatbot_weixin_cdn_base = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::Token) => {
            config.chatbot_weixin_apikey = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::AccountId) => {
            config.chatbot_weixin_account_id = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::UserId) => {
            config.chatbot_weixin_user_id = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::WhitelistUserIds) => {
            config.chatbot_weixin_whitelist_user_ids = value.to_string()
        }
        ("weixin", ChatbotProviderFieldId::PollSeconds) => {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                config.chatbot_weixin_poll_interval_seconds = seconds.max(1);
            }
        }
        ("weixin", ChatbotProviderFieldId::LongPollTimeoutMs) => {
            if let Ok(timeout) = value.trim().parse::<u64>() {
                config.chatbot_weixin_long_poll_timeout_ms = timeout.max(1);
            }
        }
        _ => {}
    }
}

fn toggle_chatbot_provider_field(
    config: &mut SetupConfig,
    provider: &str,
    field: ChatbotProviderFieldId,
) {
    if field != ChatbotProviderFieldId::Enabled {
        return;
    }
    match provider {
        "telegram" => config.chatbot_telegram_enabled = !config.chatbot_telegram_enabled,
        "feishu" => config.chatbot_feishu_enabled = !config.chatbot_feishu_enabled,
        "weixin" => config.chatbot_weixin_enabled = !config.chatbot_weixin_enabled,
        _ => {}
    }
}

fn checkbox_value(value: bool) -> String {
    if value {
        "[x] true".to_string()
    } else {
        "[ ] false".to_string()
    }
}

fn create_skill_active_field(editor: &mut CreateSkillEditor) -> (&mut String, &mut usize) {
    match editor.focus {
        CreateSkillEditorFocus::Name => (&mut editor.name_input, &mut editor.name_cursor),
        CreateSkillEditorFocus::Purpose => (&mut editor.purpose_input, &mut editor.purpose_cursor),
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor.saturating_sub(1);
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn normalize_skill_name(input: &str) -> String {
    let mut name = String::new();
    let mut last_was_dash = false;
    for ch in input.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_' | ' ' | '/') {
            Some('-')
        } else {
            None
        };
        let Some(mapped) = mapped else {
            continue;
        };
        if mapped == '-' {
            if name.is_empty() || last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        name.push(mapped);
    }
    name.trim_matches('-').to_string()
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut index = cursor + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index.min(text.len())
}

fn delete_previous_char(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let start = previous_char_boundary(text, *cursor);
    text.drain(start..*cursor);
    *cursor = start;
}

fn delete_current_char(text: &mut String, cursor: usize) {
    if cursor >= text.len() {
        return;
    }
    let end = next_char_boundary(text, cursor);
    text.drain(cursor..end);
}

fn write_new_session_marker() -> io::Result<()> {
    let path = botty_root_dir()
        .join("memory")
        .join("summary")
        .join("new.time");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, local_time_format("%Y-%m-%d %H:%M:%S")?)?;
    Ok(())
}

fn botty_root_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn local_time_format(format: &str) -> io::Result<String> {
    let output = Command::new("date").arg(format!("+{format}")).output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
