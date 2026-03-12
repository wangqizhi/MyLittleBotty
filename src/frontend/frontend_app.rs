use crate::botty_guy::{
    delete_custom_role_config, load_all_custom_role_configs, save_custom_role_config,
    CustomRoleConfig,
};
use crate::frontend::frontend_service::{
    command_suggestions, FrontendRpc, GuyEnvSetResult, RestartStatus, SaveSetupResult, SetupConfig,
    SetupFieldId,
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
        editor: Option<SetupEditor>,
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

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SubAgentEditorFocus {
    Name,
    Description,
    Skills,
}

pub struct CreateSkillEditor {
    pub name_input: String,
    pub name_cursor: usize,
    pub description_input: String,
    pub description_cursor: usize,
    pub usage_input: String,
    pub usage_cursor: usize,
    pub action_input: String,
    pub action_cursor: usize,
    pub prompt_template_input: String,
    pub prompt_template_cursor: usize,
    pub focus: CreateSkillEditorFocus,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CreateSkillEditorFocus {
    Name,
    Description,
    Usage,
    Action,
    PromptTemplate,
}

pub enum SetupEditor {
    Field(FieldEdit),
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
            editor: None,
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
            "正在迁移 work dir...".to_string()
        } else {
            "正在保存 setup...".to_string()
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

    pub fn setup_cycle_provider(&mut self, delta: i32) {
        let Mode::Setup {
            selected_field,
            selected_provider,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };

        if SetupFieldId::from_index(*selected_field) == SetupFieldId::ChatbotProvider {
            config.cycle_provider(selected_provider, delta);
        }
    }

    pub fn setup_activate(&mut self) {
        let Mode::Setup {
            selected_field,
            selected_provider,
            editor,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };

        let field = SetupFieldId::from_index(*selected_field);
        if field.is_toggle() {
            config.toggle_field(field);
            return;
        }

        if field == SetupFieldId::ChatbotProvider {
            config.cycle_provider(selected_provider, 1);
            return;
        }

        let input = config.editable_value(field);
        *editor = Some(SetupEditor::Field(FieldEdit {
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
            Mode::Setup { editor, .. } => *editor = None,
            Mode::GuyEnvEdit { editor, .. } => *editor = None,
            _ => {}
        }
    }

    pub fn editor_backspace(&mut self) {
        match &mut self.mode {
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    delete_previous_char(&mut field.input, &mut field.cursor);
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    delete_current_char(&mut field.input, field.cursor);
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    field.input.insert(field.cursor, c);
                    field.cursor += c.len_utf8();
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    field.cursor = previous_char_boundary(&field.input, field.cursor);
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    field.cursor = next_char_boundary(&field.input, field.cursor);
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => field.cursor = 0,
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => field.cursor = field.input.len(),
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
            Mode::Setup { editor, .. } => match editor.as_mut() {
                Some(SetupEditor::Field(field)) => {
                    field.input.clear();
                    field.cursor = 0;
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
            Mode::Setup { editor, config, .. } => {
                let mut close_editor = false;
                match editor.as_mut() {
                    Some(SetupEditor::Field(field)) => {
                        let value = field.input.trim().to_string();
                        if !value.is_empty() || field.selected_field == SetupFieldId::WorkDir {
                            config.set_field(field.selected_field, &value);
                        }
                        close_editor = true;
                    }
                    None => {}
                }

                if close_editor {
                    *editor = None;
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
        };
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn sub_agent_state(&self) -> Option<(usize, &[SubAgentEntry], Option<&SubAgentEditor>)> {
        match &self.mode {
            Mode::SubAgent {
                agents,
                selected_entry,
                editor,
            } => Some((*selected_entry, agents, editor.as_ref())),
            _ => None,
        }
    }

    pub fn sub_agent_prev_entry(&mut self) {
        let Mode::SubAgent {
            selected_entry,
            agents,
            ..
        } = &mut self.mode
        else {
            return;
        };
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
            ..
        } = &mut self.mode
        else {
            return;
        };
        let len = agents.len() + 1;
        *selected_entry = (*selected_entry + 1) % len;
    }

    pub fn open_sub_agent_editor(&mut self) {
        let Mode::SubAgent {
            selected_entry,
            agents,
            editor,
        } = &mut self.mode
        else {
            return;
        };

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
                description_input: String::new(),
                description_cursor: 0,
                usage_input: String::new(),
                usage_cursor: 0,
                action_input: "prompt".to_string(),
                action_cursor: 6,
                prompt_template_input: String::new(),
                prompt_template_cursor: 0,
                focus: CreateSkillEditorFocus::Name,
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

    pub fn create_skill_editor_cycle_focus(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        editor.focus = match editor.focus {
            CreateSkillEditorFocus::Name => CreateSkillEditorFocus::Description,
            CreateSkillEditorFocus::Description => CreateSkillEditorFocus::Usage,
            CreateSkillEditorFocus::Usage => CreateSkillEditorFocus::Action,
            CreateSkillEditorFocus::Action => CreateSkillEditorFocus::PromptTemplate,
            CreateSkillEditorFocus::PromptTemplate => CreateSkillEditorFocus::Name,
        };
    }

    pub fn create_skill_editor_insert(&mut self, c: char) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        input.insert(*cursor, c);
        *cursor += c.len_utf8();
    }

    pub fn create_skill_editor_backspace(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        delete_previous_char(input, cursor);
    }

    pub fn create_skill_editor_delete(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        delete_current_char(input, *cursor);
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

    pub fn create_skill_editor_clear(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let (input, cursor) = create_skill_active_field(editor);
        input.clear();
        *cursor = 0;
    }

    pub fn create_skill_editor_save(&mut self) {
        let Mode::CreateSkill { editor } = &mut self.mode else {
            return;
        };
        let name = editor.name_input.trim().to_string();
        if name.is_empty() {
            self.push_system("Skill name cannot be empty.");
            return;
        }
        let description = editor.description_input.trim().to_string();
        let usage = editor.usage_input.trim().to_string();
        let action = editor.action_input.trim().to_string();
        let prompt_template = editor.prompt_template_input.trim().to_string();

        match custom_skill::save_custom_skill(
            &name,
            &description,
            &usage,
            &action,
            &prompt_template,
        ) {
            Ok(path) => {
                self.mode = Mode::Chat;
                self.push_system(&format!("Custom skill saved to {}", path.display()));
            }
            Err(err) => self.push_system(&format!("save custom skill failed: {err}")),
        }
    }
}

fn create_skill_active_field(editor: &mut CreateSkillEditor) -> (&mut String, &mut usize) {
    match editor.focus {
        CreateSkillEditorFocus::Name => (&mut editor.name_input, &mut editor.name_cursor),
        CreateSkillEditorFocus::Description => (
            &mut editor.description_input,
            &mut editor.description_cursor,
        ),
        CreateSkillEditorFocus::Usage => (&mut editor.usage_input, &mut editor.usage_cursor),
        CreateSkillEditorFocus::Action => (&mut editor.action_input, &mut editor.action_cursor),
        CreateSkillEditorFocus::PromptTemplate => (
            &mut editor.prompt_template_input,
            &mut editor.prompt_template_cursor,
        ),
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
