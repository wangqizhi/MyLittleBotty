use crate::frontend::frontend_service::{
    command_suggestions, FrontendRpc, RestartStatus, SaveSetupResult, SetupConfig, SetupFieldId,
};
use std::io;

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
        config: SetupConfig,
    },
}

pub enum SetupEditor {
    Provider(ProviderEdit),
    Field(FieldEdit),
}

pub struct ProviderEdit {
    pub selected_provider: usize,
    pub input: String,
}

pub struct FieldEdit {
    pub selected_field: SetupFieldId,
    pub input: String,
}

pub enum SubmitOutcome {
    None,
    Quit,
}

pub struct FrontendApp {
    history: Vec<ChatLine>,
    input: String,
    selected_command: usize,
    mode: Mode,
}

impl FrontendApp {
    pub fn new() -> Self {
        let mut app = Self {
            history: Vec::new(),
            input: String::new(),
            selected_command: 0,
            mode: Mode::Chat,
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

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn is_setup_mode(&self) -> bool {
        matches!(self.mode, Mode::Setup { .. })
    }

    pub fn command_suggestions(&self) -> Vec<&'static str> {
        command_suggestions(&self.input)
    }

    pub fn chat_insert(&mut self, c: char) {
        self.input.push(c);
        self.selected_command = 0;
    }

    pub fn chat_backspace(&mut self) {
        self.input.pop();
        self.selected_command = 0;
    }

    pub fn chat_select_prev(&mut self) {
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

    pub fn chat_select_next(&mut self) {
        let suggestions = self.command_suggestions();
        if suggestions.is_empty() {
            return;
        }

        self.selected_command = (self.selected_command + 1) % suggestions.len();
    }

    pub fn chat_clear(&mut self) {
        self.input.clear();
        self.selected_command = 0;
    }

    pub fn submit_chat<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<SubmitOutcome> {
        let mut message = self.input.trim().to_string();
        let suggestions = self.command_suggestions();
        if message.starts_with('/') && !suggestions.is_empty() {
            self.clamp_selected_command();
            message = suggestions[self.selected_command].to_string();
        }

        self.input.clear();
        self.selected_command = 0;

        if message.is_empty() {
            return Ok(SubmitOutcome::None);
        }
        if matches!(message.as_str(), "/exit" | "/quit") {
            return Ok(SubmitOutcome::Quit);
        }
        if message == "/setup" {
            self.enter_setup(rpc)?;
            return Ok(SubmitOutcome::None);
        }

        self.push_user(&message);
        match rpc.send_chat(&message) {
            Ok(reply) => self.push_bot(&reply),
            Err(err) => self.push_system(&format!("request failed: {err}")),
        }
        Ok(SubmitOutcome::None)
    }

    pub fn enter_setup<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let config = rpc.load_setup()?;
        let selected_provider = config.selected_provider_index();
        self.mode = Mode::Setup {
            selected_field: 0,
            selected_provider,
            editor: None,
            config,
        };
        self.input.clear();
        Ok(())
    }

    pub fn cancel_setup(&mut self) {
        self.mode = Mode::Chat;
        self.input.clear();
        self.push_system("Setup canceled.");
    }

    pub fn save_setup<R: FrontendRpc>(&mut self, rpc: &mut R) -> io::Result<()> {
        let config = match &self.mode {
            Mode::Setup { config, .. } => config.clone(),
            Mode::Chat => return Ok(()),
        };

        let result = rpc.save_setup(&config)?;
        self.mode = Mode::Chat;
        self.input.clear();
        self.push_system(&format!("Setup saved to {}", result.config_path.display()));
        self.push_restart_status(result);
        Ok(())
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
            *editor = Some(SetupEditor::Provider(ProviderEdit {
                selected_provider: *selected_provider,
                input: config.provider_apikey(*selected_provider).to_string(),
            }));
            return;
        }

        *editor = Some(SetupEditor::Field(FieldEdit {
            selected_field: field,
            input: config.editable_value(field),
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

    pub fn editor_cancel(&mut self) {
        let Mode::Setup { editor, .. } = &mut self.mode else {
            return;
        };
        *editor = None;
    }

    pub fn editor_backspace(&mut self) {
        let Mode::Setup { editor, .. } = &mut self.mode else {
            return;
        };
        match editor.as_mut() {
            Some(SetupEditor::Provider(provider)) => {
                provider.input.pop();
            }
            Some(SetupEditor::Field(field)) => {
                field.input.pop();
            }
            None => {}
        }
    }

    pub fn editor_insert(&mut self, c: char) {
        let Mode::Setup { editor, .. } = &mut self.mode else {
            return;
        };
        match editor.as_mut() {
            Some(SetupEditor::Provider(provider)) => provider.input.push(c),
            Some(SetupEditor::Field(field)) => field.input.push(c),
            None => {}
        }
    }

    pub fn editor_provider_prev(&mut self) {
        let Mode::Setup { editor, config, .. } = &mut self.mode else {
            return;
        };
        let Some(SetupEditor::Provider(provider)) = editor.as_mut() else {
            return;
        };

        if provider.selected_provider == 0 {
            provider.selected_provider =
                crate::frontend::frontend_service::CHATBOT_PROVIDERS.len() - 1;
        } else {
            provider.selected_provider -= 1;
        }
        provider.input = config.provider_apikey(provider.selected_provider).to_string();
    }

    pub fn editor_provider_next(&mut self) {
        let Mode::Setup { editor, config, .. } = &mut self.mode else {
            return;
        };
        let Some(SetupEditor::Provider(provider)) = editor.as_mut() else {
            return;
        };

        provider.selected_provider =
            (provider.selected_provider + 1)
                % crate::frontend::frontend_service::CHATBOT_PROVIDERS.len();
        provider.input = config.provider_apikey(provider.selected_provider).to_string();
    }

    pub fn editor_submit(&mut self) {
        let Mode::Setup {
            selected_provider,
            editor,
            config,
            ..
        } = &mut self.mode
        else {
            return;
        };

        let mut close_editor = false;
        match editor.as_mut() {
            Some(SetupEditor::Provider(provider)) => {
                let value = provider.input.trim().to_string();
                if !value.is_empty() {
                    config.set_provider_apikey(provider.selected_provider, &value);
                }
                *selected_provider = provider.selected_provider;
                config.chatbot_provider =
                    crate::frontend::frontend_service::CHATBOT_PROVIDERS[*selected_provider]
                        .to_string();
                close_editor = true;
            }
            Some(SetupEditor::Field(field)) => {
                let value = field.input.trim().to_string();
                if !value.is_empty() {
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

    fn clamp_selected_command(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.selected_command = 0;
        } else {
            self.selected_command = self.selected_command.min(len - 1);
        }
    }

    fn push_restart_status(&mut self, result: SaveSetupResult) {
        match result.restart_status {
            RestartStatus::Success(message) => self.push_system(&message),
            RestartStatus::Failed(message) => self.push_system(&message),
        }
    }
}
