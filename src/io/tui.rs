use crate::botty_boss;
use crate::io::transport::TransportPlugin;
use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::env;
use std::fs;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const COMMANDS: [&str; 3] = ["/exit", "/quit", "/setup"];
const CHATBOT_PROVIDERS: [&str; 2] = ["telegram", "feishu"];

pub fn run() -> io::Result<()> {
    botty_boss::ensure_chat_ready()?;
    let socket_path = botty_boss::chat_socket_path();
    let mut transport = BossSocketTransport::connect(&socket_path)?;

    let mut app = TuiApp::new();
    app.push_system("TUI chat started. Type / for command suggestions.");

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    loop {
        terminal.draw(|f| app.render(f))?;

        let event = read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if app.is_setup_mode() {
            app.handle_setup_key(key)?;
            continue;
        }

        match app.handle_chat_key(key) {
            ChatAction::None => {}
            ChatAction::Quit => break,
            ChatAction::EnterSetup => app.enter_setup_mode()?,
            ChatAction::Send(message) => {
                app.push_user(&message);
                match request_with_reconnect(&mut transport, &socket_path, &message) {
                    Ok(reply) => app.push_bot(&reply),
                    Err(err) => app.push_system(&format!("request failed: {err}")),
                }
            }
        }
    }

    Ok(())
}

fn request_with_reconnect(
    transport: &mut BossSocketTransport,
    socket_path: &PathBuf,
    message: &str,
) -> io::Result<String> {
    match transport.request(message) {
        Ok(reply) => Ok(reply),
        Err(_) => {
            *transport = BossSocketTransport::connect(socket_path)?;
            transport.request(message)
        }
    }
}

#[derive(Clone, Copy)]
enum Role {
    User,
    Bot,
    System,
}

struct ChatLine {
    role: Role,
    text: String,
}

enum Mode {
    Chat,
    Setup {
        selected_field: usize,
        selected_provider: usize,
        provider_edit: Option<ProviderEdit>,
        config: SetupConfig,
    },
}

struct ProviderEdit {
    selected_provider: usize,
    input: String,
}

enum ChatAction {
    None,
    Quit,
    EnterSetup,
    Send(String),
}

struct TuiApp {
    history: Vec<ChatLine>,
    input: String,
    selected_command: usize,
    mode: Mode,
}

impl TuiApp {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            input: String::new(),
            selected_command: 0,
            mode: Mode::Chat,
        }
    }

    fn is_setup_mode(&self) -> bool {
        matches!(self.mode, Mode::Setup { .. })
    }

    fn push_user(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::User,
            text: text.to_string(),
        });
    }

    fn push_bot(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::Bot,
            text: text.to_string(),
        });
    }

    fn push_system(&mut self, text: &str) {
        self.history.push(ChatLine {
            role: Role::System,
            text: text.to_string(),
        });
    }

    fn command_suggestions(&self) -> Vec<&'static str> {
        if !self.input.starts_with('/') {
            return Vec::new();
        }
        COMMANDS
            .iter()
            .copied()
            .filter(|cmd| cmd.starts_with(self.input.as_str()))
            .collect()
    }

    fn clamp_selected_command(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.selected_command = 0;
        } else {
            self.selected_command = self.selected_command.min(len - 1);
        }
    }

    fn handle_chat_key(&mut self, key: KeyEvent) -> ChatAction {
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    if c == 'c' || c == 'C' {
                        return ChatAction::Quit;
                    }
                    return ChatAction::None;
                }
                self.input.push(c);
                self.selected_command = 0;
                ChatAction::None
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.selected_command = 0;
                ChatAction::None
            }
            KeyCode::Up => {
                let suggestions = self.command_suggestions();
                if !suggestions.is_empty() {
                    if self.selected_command == 0 {
                        self.selected_command = suggestions.len() - 1;
                    } else {
                        self.selected_command -= 1;
                    }
                }
                ChatAction::None
            }
            KeyCode::Down => {
                let suggestions = self.command_suggestions();
                if !suggestions.is_empty() {
                    self.selected_command = (self.selected_command + 1) % suggestions.len();
                }
                ChatAction::None
            }
            KeyCode::Esc => {
                self.input.clear();
                self.selected_command = 0;
                ChatAction::None
            }
            KeyCode::Enter => {
                let mut message = self.input.trim().to_string();
                let suggestions = self.command_suggestions();
                if message.starts_with('/') && !suggestions.is_empty() {
                    self.clamp_selected_command();
                    message = suggestions[self.selected_command].to_string();
                }

                self.input.clear();
                self.selected_command = 0;

                if message.is_empty() {
                    return ChatAction::None;
                }
                if matches!(message.as_str(), "/exit" | "/quit") {
                    return ChatAction::Quit;
                }
                if message == "/setup" {
                    return ChatAction::EnterSetup;
                }
                ChatAction::Send(message)
            }
            _ => ChatAction::None,
        }
    }

    fn enter_setup_mode(&mut self) -> io::Result<()> {
        let config = load_setup_config()?;
        let selected_provider = chatbot_provider_options()
            .iter()
            .position(|p| *p == config.chatbot_provider)
            .unwrap_or(0);
        self.mode = Mode::Setup {
            selected_field: 0,
            selected_provider,
            provider_edit: None,
            config,
        };
        self.input.clear();
        Ok(())
    }

    fn handle_setup_key(&mut self, key: KeyEvent) -> io::Result<()> {
        let Mode::Setup {
            selected_field,
            selected_provider,
            provider_edit,
            config,
        } = &mut self.mode
        else {
            return Ok(());
        };

        if let Some(editor) = provider_edit {
            match key.code {
                KeyCode::Esc => {
                    *provider_edit = None;
                }
                KeyCode::Left => {
                    let len = chatbot_provider_options().len();
                    if len > 0 {
                        if editor.selected_provider == 0 {
                            editor.selected_provider = len - 1;
                        } else {
                            editor.selected_provider -= 1;
                        }
                        editor.input = provider_apikey(config, editor.selected_provider).to_string();
                    }
                }
                KeyCode::Right => {
                    let len = chatbot_provider_options().len();
                    if len > 0 {
                        editor.selected_provider = (editor.selected_provider + 1) % len;
                        editor.input = provider_apikey(config, editor.selected_provider).to_string();
                    }
                }
                KeyCode::Backspace => {
                    editor.input.pop();
                }
                KeyCode::Enter => {
                    let value = editor.input.trim().to_string();
                    if !value.is_empty() {
                        set_provider_apikey(config, editor.selected_provider, &value);
                    }
                    *selected_provider = editor.selected_provider;
                    config.chatbot_provider = chatbot_provider_options()[*selected_provider].to_string();
                    *provider_edit = None;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    editor.input.push(c);
                }
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Chat;
                self.input.clear();
                self.push_system("Setup canceled.");
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 's' || c == 'S') => {
                save_setup_config(config)?;
                self.mode = Mode::Chat;
                self.input.clear();
                self.push_system(&format!("Setup saved to {}", setup_config_file().display()));
                match botty_boss::restart_all() {
                    Ok(()) => self.push_system("Botty input processes restarted."),
                    Err(err) => self.push_system(&format!("Auto restart failed: {err}")),
                }
            }
            KeyCode::Up => {
                if *selected_field == 0 {
                    *selected_field = setup_field_count() - 1;
                } else {
                    *selected_field -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                *selected_field = (*selected_field + 1) % setup_field_count();
            }
            KeyCode::BackTab => {
                if *selected_field == 0 {
                    *selected_field = setup_field_count() - 1;
                } else {
                    *selected_field -= 1;
                }
            }
            KeyCode::Left => {
                if *selected_field == 2 {
                    cycle_provider(selected_provider, config, -1);
                }
            }
            KeyCode::Right => {
                if *selected_field == 2 {
                    cycle_provider(selected_provider, config, 1);
                }
            }
            KeyCode::Enter => {
                if *selected_field == 2 {
                    *provider_edit = Some(ProviderEdit {
                        selected_provider: *selected_provider,
                        input: provider_apikey(config, *selected_provider).to_string(),
                    });
                    self.input.clear();
                    return Ok(());
                }
                if is_toggle_field(*selected_field) {
                    toggle_setup_field(config, *selected_field);
                    self.input.clear();
                    return Ok(());
                }

                let value = self.input.trim();
                if !value.is_empty() {
                    set_setup_field(config, *selected_field, value);
                }
                self.input.clear();
            }
            KeyCode::Char(' ') if is_toggle_field(*selected_field) => {
                toggle_setup_field(config, *selected_field);
            }
            KeyCode::Char(c) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            _ => {}
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        match &self.mode {
            Mode::Chat => self.render_chat_page(frame),
            Mode::Setup {
                selected_field,
                selected_provider,
                provider_edit,
                config,
            } => self.render_setup_page(
                frame,
                *selected_field,
                *selected_provider,
                provider_edit.as_ref(),
                config,
            ),
        }
    }

    fn render_chat_page(&mut self, frame: &mut Frame) {
        self.clamp_selected_command();
        let suggestions = self.command_suggestions();
        let suggestion_height = if suggestions.is_empty() {
            0
        } else {
            suggestions.len().min(4) as u16 + 2
        };

        let layout = if suggestion_height == 0 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(3)])
                .split(frame.area())
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(suggestion_height),
                ])
                .split(frame.area())
        };

        let chat_rect = layout[0];
        let status_rect = layout[1];
        let input_rect = layout[2];
        let suggestion_rect = if suggestion_height > 0 { Some(layout[3]) } else { None };

        let chat_lines: Vec<Line> = self
            .history
            .iter()
            .map(|item| {
                let prefix = match item.role {
                    Role::User => "you",
                    Role::Bot => "guy",
                    Role::System => "system",
                };
                Line::raw(format!("{prefix}: {}", item.text))
            })
            .collect();
        let max_visible = chat_rect.height.saturating_sub(2) as usize;
        let scroll = chat_lines.len().saturating_sub(max_visible) as u16;

        let chat = Paragraph::new(Text::from(chat_lines))
            .block(Block::default().borders(Borders::ALL).title("Chat"))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(chat, chat_rect);

        let status = Paragraph::new(Line::raw("chat mode | Enter send | / for command | Ctrl+C exit"))
            .style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_widget(status, status_rect);

        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Input"));
        frame.render_widget(input, input_rect);

        if let Some(rect) = suggestion_rect {
            let mut items = Vec::new();
            for (idx, cmd) in suggestions.iter().take(4).enumerate() {
                let style = if idx == self.selected_command {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                items.push(ListItem::new(Line::raw((*cmd).to_string())).style(style));
            }
            let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Commands"));
            frame.render_widget(list, rect);
        }

        place_cursor(frame, input_rect, 1 + self.input.chars().count() as u16);
    }

    fn render_setup_page(
        &self,
        frame: &mut Frame,
        selected_field: usize,
        selected_provider: usize,
        provider_edit: Option<&ProviderEdit>,
        config: &SetupConfig,
    ) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3), Constraint::Length(1)])
            .split(frame.area());

        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(layout[0]);

        let fields = setup_fields(config);
        let items: Vec<ListItem> = fields
            .iter()
            .enumerate()
            .map(|(idx, (label, value, masked))| {
                let shown = if *masked { mask_secret(value) } else { value.to_string() };
                let line = format!("{}: {}", label, shown);
                let style = if idx == selected_field {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::raw(line)).style(style)
            })
            .collect();

        let field_list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Setup (Up/Down select, Enter apply/toggle/open editor)"),
        );
        frame.render_widget(field_list, top[0]);

        let options = chatbot_provider_options();
        let selected = options[selected_provider.min(options.len().saturating_sub(1))];
        let side = Paragraph::new(Text::from(vec![
            Line::raw("Actions:"),
            Line::raw("- Ctrl+S: Save and return"),
            Line::raw("- Esc: Cancel"),
            Line::raw("- Tab / Shift+Tab: Next/Prev field"),
            Line::raw(""),
            Line::raw("Chatbot provider:"),
            Line::raw(format!("- {}", options.join(", "))),
            Line::raw(format!("- current: {selected}")),
            Line::raw("- Enter on chatbot provider to edit apikey"),
            Line::raw("- telegram whitelist user_ids supports comma-separated IDs"),
            Line::raw(""),
            Line::raw(format!("Config file: {}", setup_config_file().display())),
        ]))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Help"));
        frame.render_widget(side, top[1]);

        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Edit Value"));
        frame.render_widget(input, layout[1]);

        let footer = Paragraph::new(Line::raw(
            "Toggle fields support Enter/Space. Chatbot provider opens a dedicated editor.",
        ))
        .style(Style::default().fg(Color::Black).bg(Color::Green));
        frame.render_widget(footer, layout[2]);

        if let Some(editor) = provider_edit {
            self.render_provider_editor(frame, editor, config);
        } else {
            place_cursor(frame, layout[1], 1 + self.input.chars().count() as u16);
        }
    }

    fn render_provider_editor(&self, frame: &mut Frame, editor: &ProviderEdit, _config: &SetupConfig) {
        let area = centered_rect(frame.area(), 70, 32);
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Chatbot Provider Editor");
        frame.render_widget(block, area);

        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(inner);

        let options = chatbot_provider_options();
        let selected = options[editor.selected_provider.min(options.len().saturating_sub(1))];
        let hint = Paragraph::new(Text::from(vec![
            Line::raw("Left/Right: switch provider"),
            Line::raw("Type: edit apikey"),
            Line::raw("Enter: save provider+apikey"),
            Line::raw("Esc: cancel"),
            Line::raw(""),
            Line::raw(format!("Provider: {selected}")),
        ]))
        .wrap(Wrap { trim: false });
        frame.render_widget(hint, parts[0]);

        let input = Paragraph::new(editor.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Provider API Key"));
        frame.render_widget(input, parts[1]);
        place_cursor(frame, parts[1], 1 + editor.input.chars().count() as u16);
    }
}

fn place_cursor(frame: &mut Frame, input_rect: Rect, desired_col: u16) {
    let max_col = input_rect.width.saturating_sub(2);
    let x = input_rect.x + desired_col.min(max_col);
    let y = input_rect.y + 1;
    frame.set_cursor_position((x, y));
}

fn setup_fields(config: &SetupConfig) -> [(&'static str, String, bool); 8] {
    [
        ("AI provider endpoint", config.ai_provider_endpoint.clone(), false),
        ("AI provider apikey", config.ai_provider_apikey.clone(), true),
        ("chatbot provider", config.chatbot_provider.clone(), false),
        (
            "telegram enabled",
            if config.chatbot_telegram_enabled {
                "[x] true".to_string()
            } else {
                "[ ] false".to_string()
            },
            false,
        ),
        (
            "feishu enabled",
            if config.chatbot_feishu_enabled {
                "[x] true".to_string()
            } else {
                "[ ] false".to_string()
            },
            false,
        ),
        (
            "telegram poll seconds",
            config.chatbot_telegram_poll_interval_seconds.to_string(),
            false,
        ),
        (
            "telegram whitelist user_ids",
            config.chatbot_telegram_whitelist_user_ids.clone(),
            false,
        ),
        ("feishu chat id", config.chatbot_feishu_chat_id.clone(), false),
    ]
}

fn setup_field_count() -> usize {
    8
}

fn is_toggle_field(index: usize) -> bool {
    matches!(index, 3 | 4)
}

fn set_setup_field(config: &mut SetupConfig, index: usize, value: &str) {
    match index {
        0 => config.ai_provider_endpoint = value.to_string(),
        1 => config.ai_provider_apikey = value.to_string(),
        2 => config.chatbot_provider = value.to_string(),
        5 => {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                config.chatbot_telegram_poll_interval_seconds = seconds.max(1);
            }
        }
        6 => config.chatbot_telegram_whitelist_user_ids = value.to_string(),
        7 => config.chatbot_feishu_chat_id = value.to_string(),
        _ => {}
    }
}

fn toggle_setup_field(config: &mut SetupConfig, index: usize) {
    match index {
        3 => config.chatbot_telegram_enabled = !config.chatbot_telegram_enabled,
        4 => config.chatbot_feishu_enabled = !config.chatbot_feishu_enabled,
        _ => {}
    }
}

fn chatbot_provider_options() -> [&'static str; 2] {
    CHATBOT_PROVIDERS
}

fn cycle_provider(selected_provider: &mut usize, config: &mut SetupConfig, delta: i32) {
    let options = chatbot_provider_options();
    let len = options.len() as i32;
    if len == 0 {
        return;
    }
    let next = (*selected_provider as i32 + delta).rem_euclid(len);
    *selected_provider = next as usize;
    config.chatbot_provider = options[*selected_provider].to_string();
}

fn provider_apikey(config: &SetupConfig, selected_provider: usize) -> &str {
    match chatbot_provider_options()
        .get(selected_provider)
        .copied()
        .unwrap_or("telegram")
    {
        "telegram" => config.chatbot_telegram_apikey.as_str(),
        "feishu" => config.chatbot_feishu_apikey.as_str(),
        _ => "",
    }
}

fn set_provider_apikey(config: &mut SetupConfig, selected_provider: usize, apikey: &str) {
    match chatbot_provider_options()
        .get(selected_provider)
        .copied()
        .unwrap_or("telegram")
    {
        "telegram" => config.chatbot_telegram_apikey = apikey.to_string(),
        "feishu" => config.chatbot_feishu_apikey = apikey.to_string(),
        _ => {}
    }
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let visible = value.chars().count().min(4);
    let masked_len = value.chars().count().saturating_sub(visible);
    let suffix: String = value
        .chars()
        .rev()
        .take(visible)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(masked_len), suffix)
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

struct SetupConfig {
    ai_provider_endpoint: String,
    ai_provider_apikey: String,
    chatbot_provider: String,
    chatbot_telegram_api_base: String,
    chatbot_telegram_apikey: String,
    chatbot_feishu_api_base: String,
    chatbot_feishu_apikey: String,
    chatbot_telegram_enabled: bool,
    chatbot_feishu_enabled: bool,
    chatbot_telegram_whitelist_user_ids: String,
    chatbot_telegram_poll_interval_seconds: u64,
    chatbot_feishu_poll_interval_seconds: u64,
    chatbot_feishu_chat_id: String,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            ai_provider_endpoint: String::new(),
            ai_provider_apikey: String::new(),
            chatbot_provider: "telegram".to_string(),
            chatbot_telegram_api_base: "https://api.telegram.org".to_string(),
            chatbot_telegram_apikey: String::new(),
            chatbot_feishu_api_base: "https://open.feishu.cn/open-apis".to_string(),
            chatbot_feishu_apikey: String::new(),
            chatbot_telegram_enabled: true,
            chatbot_feishu_enabled: false,
            chatbot_telegram_whitelist_user_ids: String::new(),
            chatbot_telegram_poll_interval_seconds: 1,
            chatbot_feishu_poll_interval_seconds: 1,
            chatbot_feishu_chat_id: String::new(),
        }
    }
}

fn load_setup_config() -> io::Result<SetupConfig> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(SetupConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = SetupConfig::default();
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
            "ai.provider.endpoint" => config.ai_provider_endpoint = value.to_string(),
            "ai.provider.apikey" => config.ai_provider_apikey = value.to_string(),
            "provider.endpoint" => config.ai_provider_endpoint = value.to_string(),
            "provider.apikey" => config.ai_provider_apikey = value.to_string(),
            "chatbot.provider" => apply_chatbot_provider_list(&mut config, value),
            "chatbot.telegram.api_base" => config.chatbot_telegram_api_base = value.to_string(),
            "chatbot.telegram.apikey" => config.chatbot_telegram_apikey = value.to_string(),
            "chatbot.feishu.api_base" => config.chatbot_feishu_api_base = value.to_string(),
            "chatbot.feishu.apikey" => config.chatbot_feishu_apikey = value.to_string(),
            "chatbot.apikey" => {
                if config.chatbot_provider == "feishu" {
                    config.chatbot_feishu_apikey = value.to_string();
                } else {
                    config.chatbot_telegram_apikey = value.to_string();
                }
            }
            "chatbot.telegram.enabled" => config.chatbot_telegram_enabled = parse_bool(value),
            "chatbot.feishu.enabled" => config.chatbot_feishu_enabled = parse_bool(value),
            "chatbot.telegram.whitelist_user_ids" => {
                config.chatbot_telegram_whitelist_user_ids = value.to_string()
            }
            "chatbot.feishu.chat_id" => config.chatbot_feishu_chat_id = value.to_string(),
            "chatbot.telegram.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.chatbot_telegram_poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.feishu.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.chatbot_feishu_poll_interval_seconds = seconds.max(1);
                }
            }
            _ => {}
        }
    }

    Ok(config)
}

fn save_setup_config(config: &SetupConfig) -> io::Result<()> {
    let path = setup_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = format!(
        "ai.provider.endpoint={}\nai.provider.apikey={}\nchatbot.provider={}\nchatbot.telegram.api_base={}\nchatbot.telegram.apikey={}\nchatbot.feishu.api_base={}\nchatbot.feishu.apikey={}\nchatbot.telegram.enabled={}\nchatbot.feishu.enabled={}\nchatbot.telegram.whitelist_user_ids={}\nchatbot.telegram.poll_interval_seconds={}\nchatbot.feishu.poll_interval_seconds={}\nchatbot.feishu.chat_id={}\n",
        config.ai_provider_endpoint,
        config.ai_provider_apikey,
        enabled_provider_list(config),
        config.chatbot_telegram_api_base,
        config.chatbot_telegram_apikey,
        config.chatbot_feishu_api_base,
        config.chatbot_feishu_apikey,
        config.chatbot_telegram_enabled,
        config.chatbot_feishu_enabled,
        config.chatbot_telegram_whitelist_user_ids,
        config.chatbot_telegram_poll_interval_seconds,
        config.chatbot_feishu_poll_interval_seconds,
        config.chatbot_feishu_chat_id
    );

    fs::write(path, content)
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn apply_chatbot_provider_list(config: &mut SetupConfig, value: &str) {
    config.chatbot_telegram_enabled = false;
    config.chatbot_feishu_enabled = false;

    let mut first_enabled = None;
    for item in value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        match item {
            "telegram" => {
                config.chatbot_telegram_enabled = true;
                if first_enabled.is_none() {
                    first_enabled = Some("telegram");
                }
            }
            "feishu" => {
                config.chatbot_feishu_enabled = true;
                if first_enabled.is_none() {
                    first_enabled = Some("feishu");
                }
            }
            _ => {}
        }
    }

    config.chatbot_provider = first_enabled.unwrap_or("telegram").to_string();
}

fn enabled_provider_list(config: &SetupConfig) -> String {
    let mut list = Vec::new();
    if config.chatbot_telegram_enabled {
        list.push("telegram");
    }
    if config.chatbot_feishu_enabled {
        list.push("feishu");
    }
    list.join(",")
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
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

struct BossSocketTransport {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

const CHAT_META_PREFIX: &str = "__botty_meta__";

impl BossSocketTransport {
    fn connect(path: &PathBuf) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);
        Ok(Self { reader, writer })
    }
}

impl TransportPlugin for BossSocketTransport {
    fn request(&mut self, message: &str) -> io::Result<String> {
        let payload = encode_meta_message("tui", "tui", message);
        writeln!(self.writer, "{payload}")?;
        self.writer.flush()?;

        let mut reply = String::new();
        let bytes = self.reader.read_line(&mut reply)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Botty-Boss closed connection",
            ));
        }
        Ok(reply.trim_end().to_string())
    }
}

fn encode_meta_message(source: &str, user_id: &str, message: &str) -> String {
    format!("{CHAT_META_PREFIX}|source={source}|user_id={user_id}|{message}")
}
