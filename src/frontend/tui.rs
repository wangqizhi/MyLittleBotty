use crate::frontend::frontend_app::{
    FieldEdit, FrontendApp, Mode, ProviderEdit, Role, SetupEditor, SubmitOutcome,
};
use crate::frontend::frontend_service::{
    mask_secret, FrontendRpc, LocalFrontendRpc, SetupConfig, CHATBOT_PROVIDERS,
};
use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io;
use unicode_width::UnicodeWidthStr;

pub fn run() -> io::Result<()> {
    let mut rpc = LocalFrontendRpc::connect()?;
    let mut app = FrontendApp::new();

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    loop {
        terminal.draw(|f| render(&app, f))?;

        let event = read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if app.is_setup_mode() {
            handle_setup_key(&mut app, &mut rpc, key)?;
            continue;
        }

        if matches!(handle_chat_key(&mut app, &mut rpc, key)?, SubmitOutcome::Quit) {
            break;
        }
    }

    Ok(())
}

fn handle_chat_key<R: FrontendRpc>(
    app: &mut FrontendApp,
    rpc: &mut R,
    key: KeyEvent,
) -> io::Result<SubmitOutcome> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if c == 'c' || c == 'C' {
                    return Ok(SubmitOutcome::Quit);
                }
                return Ok(SubmitOutcome::None);
            }
            app.chat_insert(c);
            Ok(SubmitOutcome::None)
        }
        KeyCode::Backspace => {
            app.chat_backspace();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Up => {
            app.chat_select_prev();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Down => {
            app.chat_select_next();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Esc => {
            app.chat_clear();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Enter => app.submit_chat(rpc),
        _ => Ok(SubmitOutcome::None),
    }
}

fn handle_setup_key<R: FrontendRpc>(
    app: &mut FrontendApp,
    rpc: &mut R,
    key: KeyEvent,
) -> io::Result<()> {
    let editor_open = matches!(
        app.mode(),
        Mode::Setup {
            editor: Some(_),
            ..
        }
    );

    if editor_open {
        match key.code {
            KeyCode::Esc => app.editor_cancel(),
            KeyCode::Left => app.editor_provider_prev(),
            KeyCode::Right => app.editor_provider_next(),
            KeyCode::Backspace => app.editor_backspace(),
            KeyCode::Enter => app.editor_submit(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => app.editor_insert(c),
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => app.cancel_setup(),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 's' || c == 'S') => {
            app.save_setup(rpc)?;
        }
        KeyCode::Up => app.setup_prev_field(),
        KeyCode::Down | KeyCode::Tab => app.setup_next_field(),
        KeyCode::BackTab => app.setup_prev_field(),
        KeyCode::Left => app.setup_cycle_provider(-1),
        KeyCode::Right => app.setup_cycle_provider(1),
        KeyCode::Enter => app.setup_activate(),
        KeyCode::Char(' ') => app.setup_toggle_selected(),
        _ => {}
    }

    Ok(())
}

fn render(app: &FrontendApp, frame: &mut Frame) {
    match app.mode() {
        Mode::Chat => render_chat_page(app, frame),
        Mode::Setup {
            selected_field,
            selected_provider,
            editor,
            config,
        } => render_setup_page(frame, *selected_field, *selected_provider, editor.as_ref(), config),
    }
}

fn render_chat_page(app: &FrontendApp, frame: &mut Frame) {
    let suggestions = app.command_suggestions();
    let selected_command = if suggestions.is_empty() {
        0
    } else {
        app.selected_command().min(suggestions.len() - 1)
    };

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

    let chat_lines: Vec<Line> = app
        .history()
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

    let input = Paragraph::new(app.input())
        .block(Block::default().borders(Borders::ALL).title("Input"));
    frame.render_widget(input, input_rect);

    if let Some(rect) = suggestion_rect {
        let mut items = Vec::new();
        for (idx, cmd) in suggestions.iter().take(4).enumerate() {
            let style = if idx == selected_command {
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

    place_cursor(frame, input_rect, text_display_width(app.input()));
}

fn render_setup_page(
    frame: &mut Frame,
    selected_field: usize,
    selected_provider: usize,
    editor: Option<&SetupEditor>,
    config: &SetupConfig,
) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(layout[0]);

    let items: Vec<ListItem> = config
        .fields()
        .into_iter()
        .enumerate()
        .map(|(idx, field)| {
            let shown = if field.masked {
                mask_secret(&field.value)
            } else {
                field.value
            };
            let line = format!("{}: {}", field.label, shown);
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

    let selected = CHATBOT_PROVIDERS[selected_provider.min(CHATBOT_PROVIDERS.len().saturating_sub(1))];
    let side = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Ctrl+S: Save and return"),
        Line::raw("- Esc: Cancel"),
        Line::raw("- Tab / Shift+Tab: Next/Prev field"),
        Line::raw(""),
        Line::raw("Chatbot provider:"),
        Line::raw(format!("- {}", CHATBOT_PROVIDERS.join(", "))),
        Line::raw(format!("- current: {selected}")),
        Line::raw("- Enter opens editor for current field"),
        Line::raw("- Left/Right switches chatbot provider"),
        Line::raw("- telegram whitelist user_ids supports comma-separated IDs"),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(side, top[1]);

    let footer = Paragraph::new(Line::raw(
        "Enter edits field in a modal. Toggle fields support Enter/Space.",
    ))
    .style(Style::default().fg(Color::Black).bg(Color::Green));
    frame.render_widget(footer, layout[1]);

    if let Some(editor) = editor {
        match editor {
            SetupEditor::Provider(editor) => render_provider_editor(frame, editor),
            SetupEditor::Field(editor) => render_field_editor(frame, editor),
        }
    }
}

fn render_provider_editor(frame: &mut Frame, editor: &ProviderEdit) {
    let area = centered_rect(frame.area(), 70, 32);
    frame.render_widget(Clear, area);
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

    let selected = CHATBOT_PROVIDERS[editor
        .selected_provider
        .min(CHATBOT_PROVIDERS.len().saturating_sub(1))];
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
    place_cursor(frame, parts[1], text_display_width(editor.input.as_str()));
}

fn render_field_editor(frame: &mut Frame, editor: &FieldEdit) {
    let area = centered_rect(frame.area(), 70, 24);
    frame.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title("Edit Value");
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

    let label = editor.selected_field.label();
    let hint = Paragraph::new(Text::from(vec![
        Line::raw(format!("Field: {label}")),
        Line::raw("Type: edit value"),
        Line::raw("Enter: save"),
        Line::raw("Esc: cancel"),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(hint, parts[0]);

    let input = Paragraph::new(editor.input.as_str())
        .block(Block::default().borders(Borders::ALL).title(label));
    frame.render_widget(input, parts[1]);
    place_cursor(frame, parts[1], text_display_width(editor.input.as_str()));
}

fn text_display_width(text: &str) -> u16 {
    text.width().min(u16::MAX as usize) as u16 + 1
}

fn place_cursor(frame: &mut Frame, input_rect: Rect, desired_col: u16) {
    let max_col = input_rect.width.saturating_sub(2);
    let x = input_rect.x + desired_col.min(max_col);
    let y = input_rect.y + 1;
    frame.set_cursor_position((x, y));
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
