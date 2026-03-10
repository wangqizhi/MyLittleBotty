use crate::frontend::frontend_app::{
    CreateSkillEditorFocus, FieldEdit, FrontendApp, GuyEnvEditor, GuyEnvEditorFocus, Mode,
    ProviderEdit, Role, SetupEditor, SubAgentEditor, SubAgentEditorFocus, SubmitOutcome,
};
use crate::frontend::frontend_service::{
    mask_secret, FrontendRpc, LocalFrontendRpc, SetupConfig, CHATBOT_PROVIDERS,
};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

pub fn run() -> io::Result<()> {
    let mut rpc = LocalFrontendRpc::connect()?;
    let mut app = FrontendApp::new();
    let mut pending_reply: Option<Receiver<io::Result<String>>> = None;
    let mut pending_setup_save: Option<
        Receiver<io::Result<crate::frontend::frontend_service::SaveSetupResult>>,
    > = None;
    let mut pending_desc_gen: Option<Receiver<io::Result<String>>> = None;

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    loop {
        if let Some(receiver) = pending_reply.as_ref() {
            if let Ok(result) = receiver.try_recv() {
                app.finish_chat_request(result);
                pending_reply = None;
            }
        }
        if let Some(receiver) = pending_setup_save.as_ref() {
            if let Ok(result) = receiver.try_recv() {
                app.finish_setup_save(result);
                pending_setup_save = None;
            }
        }
        if let Some(receiver) = pending_desc_gen.as_ref() {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok(desc) => app.sub_agent_editor_set_description(desc),
                    Err(err) => app.push_system(&format!("generate description failed: {err}")),
                }
                pending_desc_gen = None;
            }
        }

        terminal.draw(|f| render(&app, f))?;

        if !poll(Duration::from_millis(120))? {
            app.tick();
            continue;
        }

        let event = read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if matches!(app.mode(), Mode::SubAgent { .. }) {
            if let Some((name, skills)) =
                handle_sub_agent_key(&mut app, key, pending_desc_gen.is_some())
            {
                pending_desc_gen = Some(spawn_desc_gen_request(name, skills));
            }
            continue;
        }

        if matches!(app.mode(), Mode::CreateSkill { .. }) {
            handle_create_skill_key(&mut app, key);
            continue;
        }

        if app.is_setup_mode() {
            if let Some(receiver) = handle_panel_key(&mut app, &mut rpc, key)? {
                pending_setup_save = Some(receiver);
            }
            continue;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
            && pending_reply.is_some()
        {
            match crate::botty_boss::interrupt_active_request() {
                Ok(()) => app.push_system("Interrupt signal sent."),
                Err(err) => app.push_system(&format!("interrupt failed: {err}")),
            }
            app.finish_chat_request(Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Request interrupted.",
            )));
            pending_reply = None;
            continue;
        }

        match handle_chat_key(&mut app, &mut rpc, key)? {
            SubmitOutcome::Quit => break,
            SubmitOutcome::SendChat(message) => {
                pending_reply = Some(spawn_chat_request(message));
            }
            SubmitOutcome::None => {}
        }
    }

    Ok(())
}

fn handle_chat_key<R: FrontendRpc>(
    app: &mut FrontendApp,
    rpc: &mut R,
    key: KeyEvent,
) -> io::Result<SubmitOutcome> {
    let suggestion_open = !app.command_suggestions().is_empty();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => app.chat_move_home(),
            KeyCode::Char('b') | KeyCode::Char('B') => app.chat_move_left(),
            KeyCode::Char('c') | KeyCode::Char('C') => app.chat_clear(),
            KeyCode::Char('d') | KeyCode::Char('D') => app.chat_delete(),
            KeyCode::Char('e') | KeyCode::Char('E') => app.chat_move_end(),
            KeyCode::Char('f') | KeyCode::Char('F') => app.chat_move_right(),
            KeyCode::Char('h') | KeyCode::Char('H') => app.chat_backspace(),
            KeyCode::Char('n') | KeyCode::Char('N') => app.chat_select_next(),
            KeyCode::Char('p') | KeyCode::Char('P') => app.chat_select_prev(),
            KeyCode::Char('u') | KeyCode::Char('U') => app.chat_clear(),
            _ => {}
        }
        return Ok(SubmitOutcome::None);
    }

    match key.code {
        KeyCode::Char(c) => {
            app.chat_insert(c);
            Ok(SubmitOutcome::None)
        }
        KeyCode::Backspace => {
            app.chat_backspace();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Delete => {
            app.chat_delete();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Left => {
            app.chat_move_left();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Right => {
            app.chat_move_right();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Home => {
            app.chat_move_home();
            Ok(SubmitOutcome::None)
        }
        KeyCode::End => {
            app.chat_move_end();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Up => {
            if suggestion_open {
                app.chat_select_prev_command();
            } else {
                app.chat_select_prev();
            }
            Ok(SubmitOutcome::None)
        }
        KeyCode::Down => {
            if suggestion_open {
                app.chat_select_next_command();
            } else {
                app.chat_select_next();
            }
            Ok(SubmitOutcome::None)
        }
        KeyCode::Esc => {
            app.chat_clear();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Tab => {
            app.chat_select_next_command();
            Ok(SubmitOutcome::None)
        }
        KeyCode::BackTab => {
            app.chat_select_prev_command();
            Ok(SubmitOutcome::None)
        }
        KeyCode::Enter => app.submit_chat(rpc),
        _ => Ok(SubmitOutcome::None),
    }
}

fn handle_panel_key<R: FrontendRpc>(
    app: &mut FrontendApp,
    rpc: &mut R,
    key: KeyEvent,
) -> io::Result<Option<Receiver<io::Result<crate::frontend::frontend_service::SaveSetupResult>>>> {
    if app.is_setup_save_pending() {
        return Ok(None);
    }

    let editor_open = matches!(
        app.mode(),
        Mode::Setup {
            editor: Some(_),
            ..
        }
    ) || matches!(
        app.mode(),
        Mode::GuyEnvEdit {
            editor: Some(_),
            ..
        }
    );

    if editor_open {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => app.editor_move_home(),
                KeyCode::Char('c') | KeyCode::Char('C') => app.editor_clear(),
                KeyCode::Char('s') | KeyCode::Char('S') => app.editor_submit(rpc),
                _ => {}
            }
            return Ok(None);
        }

        match key.code {
            KeyCode::Esc => app.editor_cancel(),
            KeyCode::Left => app.editor_move_left(),
            KeyCode::Right => app.editor_move_right(),
            KeyCode::Home => app.editor_move_home(),
            KeyCode::End => app.editor_move_end(),
            KeyCode::Backspace => app.editor_backspace(),
            KeyCode::Delete => app.editor_delete(),
            KeyCode::Tab => app.toggle_guy_env_editor_focus(),
            KeyCode::BackTab => app.toggle_guy_env_editor_focus(),
            KeyCode::Enter => app.editor_submit(rpc),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.editor_insert(c)
            }
            _ => {}
        }
        return Ok(None);
    }

    match app.mode() {
        Mode::Setup { .. } => match key.code {
            KeyCode::Esc => app.cancel_setup(),
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 's' || c == 'S') =>
            {
                if let Some(config) = app.begin_setup_save() {
                    return Ok(Some(spawn_setup_save_request(config)));
                }
            }
            KeyCode::Up => app.setup_prev_field(),
            KeyCode::Down | KeyCode::Tab => app.setup_next_field(),
            KeyCode::BackTab => app.setup_prev_field(),
            KeyCode::Left => app.setup_cycle_provider(-1),
            KeyCode::Right => app.setup_cycle_provider(1),
            KeyCode::Enter => app.setup_activate(),
            KeyCode::Char(' ') => app.setup_toggle_selected(),
            _ => {}
        },
        Mode::GuyEnvEdit { .. } => match key.code {
            KeyCode::Esc => app.close_panel(),
            KeyCode::Up => app.guy_env_prev_entry(),
            KeyCode::Down | KeyCode::Tab => app.guy_env_next_entry(),
            KeyCode::BackTab => app.guy_env_prev_entry(),
            KeyCode::Enter => app.open_guy_env_editor(),
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'r' || c == 'R') =>
            {
                app.refresh_guy_env(rpc)?;
            }
            _ => {}
        },
        Mode::GuyEnvList { .. } => match key.code {
            KeyCode::Esc => app.close_panel(),
            KeyCode::Up => app.guy_env_prev_entry(),
            KeyCode::Down | KeyCode::Tab => app.guy_env_next_entry(),
            KeyCode::BackTab => app.guy_env_prev_entry(),
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'r' || c == 'R') =>
            {
                app.refresh_guy_env(rpc)?;
            }
            _ => {}
        },
        Mode::Chat | Mode::SubAgent { .. } | Mode::CreateSkill { .. } => {}
    }

    Ok(None)
}

fn render(app: &FrontendApp, frame: &mut Frame) {
    match app.mode() {
        Mode::Chat => render_chat_page(app, frame),
        Mode::Setup {
            selected_field,
            selected_provider,
            editor,
            config,
            ..
        } => render_setup_page(
            frame,
            *selected_field,
            *selected_provider,
            editor.as_ref(),
            config,
            app.pending_setup_save_text(),
        ),
        Mode::GuyEnvEdit { .. } => render_guy_env_edit_page(app, frame),
        Mode::GuyEnvList { .. } => render_guy_env_list_page(app, frame),
        Mode::SubAgent { .. } => render_sub_agent_page(app, frame),
        Mode::CreateSkill { .. } => render_create_skill_page(app, frame),
    }
}

fn render_chat_page(app: &FrontendApp, frame: &mut Frame) {
    let suggestions = app.command_suggestions();
    let selected_command = if suggestions.is_empty() {
        0
    } else {
        app.selected_command().min(suggestions.len() - 1)
    };
    let visible_suggestion_count = 4usize;

    let suggestion_height = if suggestions.is_empty() {
        0
    } else {
        suggestions.len().min(visible_suggestion_count) as u16 + 2
    };

    let layout = if suggestion_height == 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
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
    let suggestion_rect = if suggestion_height > 0 {
        Some(layout[3])
    } else {
        None
    };

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
    let mut chat_lines = chat_lines;
    if let Some(thinking) = app.pending_chat_text() {
        chat_lines.push(Line::raw(format!("system: {thinking}")));
    }
    let max_visible = chat_rect.height.saturating_sub(2) as usize;
    let scroll = chat_lines.len().saturating_sub(max_visible) as u16;

    let chat = Paragraph::new(Text::from(chat_lines))
        .block(Block::default().borders(Borders::ALL).title("Chat"))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(chat, chat_rect);

    let status = Paragraph::new(Line::raw(
        "chat mode | Enter send | Up/Down history | Tab command | Ctrl+A/E home/end | Ctrl+C clear",
    ))
    .style(Style::default().fg(Color::Black).bg(Color::Cyan));
    frame.render_widget(status, status_rect);

    let input =
        Paragraph::new(app.input()).block(Block::default().borders(Borders::ALL).title("Input"));
    frame.render_widget(input, input_rect);

    if let Some(rect) = suggestion_rect {
        let visible_count = suggestions.len().min(visible_suggestion_count);
        let start = selected_command
            .saturating_add(1)
            .saturating_sub(visible_count);
        let mut items = Vec::new();
        for (offset, cmd) in suggestions
            .iter()
            .skip(start)
            .take(visible_count)
            .enumerate()
        {
            let idx = start + offset;
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

    place_cursor(
        frame,
        input_rect,
        text_display_width(&app.input()[..app.input_cursor()]),
    );
}

fn spawn_chat_request(message: String) -> Receiver<io::Result<String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = LocalFrontendRpc::connect().and_then(|mut rpc| rpc.send_chat(&message));
        let _ = sender.send(result);
    });
    receiver
}

fn spawn_setup_save_request(
    config: SetupConfig,
) -> Receiver<io::Result<crate::frontend::frontend_service::SaveSetupResult>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = LocalFrontendRpc::connect().and_then(|mut rpc| rpc.save_setup(&config));
        let _ = sender.send(result);
    });
    receiver
}

fn render_setup_page(
    frame: &mut Frame,
    selected_field: usize,
    selected_provider: usize,
    editor: Option<&SetupEditor>,
    config: &SetupConfig,
    pending_message: Option<&str>,
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

    let selected =
        CHATBOT_PROVIDERS[selected_provider.min(CHATBOT_PROVIDERS.len().saturating_sub(1))];
    let side = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Ctrl+S: Save and return"),
        Line::raw("- Esc: Cancel"),
        Line::raw("- Tab / Shift+Tab: Next/Prev field"),
        Line::raw(""),
        Line::raw("Work dir:"),
        Line::raw("- default: ~/opt/mylittlebotty-workdir"),
        Line::raw("- changing it migrates current work-dir contents"),
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
        pending_message
            .unwrap_or("Enter edits field in a modal. Toggle fields support Enter/Space."),
    ))
    .style(if pending_message.is_some() {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::Black).bg(Color::Green)
    });
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
        Line::raw("Left/Right: move cursor"),
        Line::raw("Ctrl+A: line start | Ctrl+C: clear"),
        Line::raw("Enter: save provider+apikey"),
        Line::raw("Esc: cancel"),
        Line::raw(""),
        Line::raw("Use setup Left/Right outside editor to switch provider"),
        Line::raw(format!("Provider: {selected}")),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(hint, parts[0]);

    let input = Paragraph::new(editor.input.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Provider API Key | Ctrl+A Home | <- -> Move | Ctrl+C Clear"),
    );
    frame.render_widget(input, parts[1]);
    place_cursor(
        frame,
        parts[1],
        text_display_width_at(editor.input.as_str(), editor.cursor),
    );
}

fn render_field_editor(frame: &mut Frame, editor: &FieldEdit) {
    let area = centered_rect(frame.area(), 70, 24);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit Value | Ctrl+A Home | <- -> Move | Ctrl+C Clear");
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
    place_cursor(
        frame,
        parts[1],
        text_display_width_at(editor.input.as_str(), editor.cursor),
    );
}

fn render_guy_env_edit_page(app: &FrontendApp, frame: &mut Frame) {
    let Some((selected_entry, entries)) = app.guy_env_edit_state() else {
        return;
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(layout[0]);

    let mut items = Vec::new();
    for (idx, (key, value)) in entries.iter().enumerate() {
        let style = if idx == selected_entry {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::raw(format!("{key}={value}"))).style(style));
    }
    let new_idx = entries.len();
    let new_style = if selected_entry == new_idx {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    items.push(ListItem::new(Line::raw("[Add new env]")).style(new_style));

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Guy Env Editor (Up/Down select, Enter edit/add)"),
    );
    frame.render_widget(list, top[0]);

    let selected_text = if selected_entry < entries.len() {
        format!(
            "Current:\n{}={}",
            entries[selected_entry].0, entries[selected_entry].1
        )
    } else {
        "Current:\n[Add new env]".to_string()
    };
    let help = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Enter: edit selected item"),
        Line::raw("- Esc: back to chat"),
        Line::raw("- Ctrl+R: reload env list"),
        Line::raw(""),
        Line::raw(selected_text),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(help, top[1]);

    let footer = Paragraph::new(Line::raw(
        "This page edits guy env locally. Saving applies to the running guy process.",
    ))
    .style(Style::default().fg(Color::Black).bg(Color::Green));
    frame.render_widget(footer, layout[1]);

    if let Some(editor) = app.guy_env_editor() {
        render_guy_env_editor(frame, editor);
    }
}

fn render_guy_env_list_page(app: &FrontendApp, frame: &mut Frame) {
    let Some((selected_entry, entries)) = app.guy_env_list_state() else {
        return;
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(layout[0]);

    let items: Vec<ListItem> = if entries.is_empty() {
        vec![ListItem::new(Line::raw("No env vars configured"))]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(idx, (key, value))| {
                let style = if idx == selected_entry {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::raw(format!("{key}={value}"))).style(style)
            })
            .collect()
    };
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Guy Env"));
    frame.render_widget(list, top[0]);

    let preview = if entries.is_empty() {
        "No env vars configured.".to_string()
    } else {
        let (key, value) = &entries[selected_entry.min(entries.len() - 1)];
        format!("Selected:\n{key}={value}")
    };
    let help = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Esc: back to chat"),
        Line::raw("- Ctrl+R: reload env list"),
        Line::raw(""),
        Line::raw(preview),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Details"));
    frame.render_widget(help, top[1]);

    let footer = Paragraph::new(Line::raw(
        "Read-only guy env page. Use /set-guy-env to edit values.",
    ))
    .style(Style::default().fg(Color::Black).bg(Color::Cyan));
    frame.render_widget(footer, layout[1]);
}

fn render_guy_env_editor(frame: &mut Frame, editor: &GuyEnvEditor) {
    let area = centered_rect(frame.area(), 76, 34);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit Guy Env | Tab switch field | Enter save");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);

    let mode = if editor.original_key.is_some() {
        "Editing existing env"
    } else {
        "Creating new env"
    };
    let hint = Paragraph::new(Text::from(vec![
        Line::raw(mode),
        Line::raw("Tab / Shift+Tab: switch between key and value"),
        Line::raw("Ctrl+A: line start | Ctrl+C: clear current field"),
        Line::raw("Esc: cancel | Enter / Ctrl+S: save"),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(hint, parts[0]);

    let key_title = if editor.focus == GuyEnvEditorFocus::Key {
        "Key (active)"
    } else {
        "Key"
    };
    let value_title = if editor.focus == GuyEnvEditorFocus::Value {
        "Value (active)"
    } else {
        "Value"
    };
    let key_input = Paragraph::new(editor.key_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(key_title));
    let value_input = Paragraph::new(editor.value_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(value_title));
    frame.render_widget(key_input, parts[1]);
    frame.render_widget(value_input, parts[2]);

    let note = Paragraph::new(Text::from(vec![
        Line::raw("Env key rules:"),
        Line::raw("- start with letter or _"),
        Line::raw("- only letters, digits, _"),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Validation"));
    frame.render_widget(note, parts[3]);

    match editor.focus {
        GuyEnvEditorFocus::Key => place_cursor(
            frame,
            parts[1],
            text_display_width_at(editor.key_input.as_str(), editor.key_cursor),
        ),
        GuyEnvEditorFocus::Value => place_cursor(
            frame,
            parts[2],
            text_display_width_at(editor.value_input.as_str(), editor.value_cursor),
        ),
    }
}

fn text_display_width(text: &str) -> u16 {
    text.width().min(u16::MAX as usize) as u16 + 1
}

fn text_display_width_at(text: &str, cursor: usize) -> u16 {
    text[..cursor.min(text.len())]
        .width()
        .min(u16::MAX as usize) as u16
        + 1
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

// --- Sub-agent key handler ---

/// Returns Some((name, skills)) if the user requested description generation.
fn handle_sub_agent_key(
    app: &mut FrontendApp,
    key: KeyEvent,
    desc_gen_pending: bool,
) -> Option<(String, Vec<String>)> {
    let has_editor = matches!(
        app.mode(),
        Mode::SubAgent {
            editor: Some(_),
            ..
        }
    );

    if has_editor {
        // Inside the sub-agent editor
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => app.sub_agent_editor_move_home(),
                KeyCode::Char('c') | KeyCode::Char('C') => app.sub_agent_editor_clear(),
                KeyCode::Char('s') | KeyCode::Char('S') => app.sub_agent_editor_save(),
                KeyCode::Char('g') | KeyCode::Char('G') => {
                    if !desc_gen_pending {
                        return app.sub_agent_editor_generate_description();
                    }
                }
                _ => {}
            }
            return None;
        }

        match key.code {
            KeyCode::Esc => app.sub_agent_editor_cancel(),
            KeyCode::Tab => app.sub_agent_editor_cycle_focus(),
            KeyCode::BackTab => app.sub_agent_editor_cycle_focus(),
            KeyCode::Left => app.sub_agent_editor_move_left(),
            KeyCode::Right => app.sub_agent_editor_move_right(),
            KeyCode::Up => app.sub_agent_editor_skill_prev(),
            KeyCode::Down => app.sub_agent_editor_skill_next(),
            KeyCode::Backspace => app.sub_agent_editor_backspace(),
            KeyCode::Delete => app.sub_agent_editor_delete(),
            KeyCode::Char(' ') => app.sub_agent_editor_toggle_skill(),
            KeyCode::Enter => app.sub_agent_editor_save(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.sub_agent_editor_insert(c);
            }
            _ => {}
        }
        return None;
    }

    // List view
    match key.code {
        KeyCode::Esc => app.close_panel(),
        KeyCode::Up => app.sub_agent_prev_entry(),
        KeyCode::Down | KeyCode::Tab => app.sub_agent_next_entry(),
        KeyCode::BackTab => app.sub_agent_prev_entry(),
        KeyCode::Enter => app.open_sub_agent_editor(),
        _ => {}
    }
    None
}

// --- Create skill key handler ---

fn handle_create_skill_key(app: &mut FrontendApp, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => app.create_skill_editor_move_home(),
            KeyCode::Char('c') | KeyCode::Char('C') => app.create_skill_editor_clear(),
            KeyCode::Char('s') | KeyCode::Char('S') => app.create_skill_editor_save(),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => app.close_panel(),
        KeyCode::Tab => app.create_skill_editor_cycle_focus(),
        KeyCode::BackTab => app.create_skill_editor_cycle_focus(),
        KeyCode::Left => app.create_skill_editor_move_left(),
        KeyCode::Right => app.create_skill_editor_move_right(),
        KeyCode::Backspace => app.create_skill_editor_backspace(),
        KeyCode::Delete => app.create_skill_editor_delete(),
        KeyCode::Enter => app.create_skill_editor_save(),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.create_skill_editor_insert(c);
        }
        _ => {}
    }
}

// --- Sub-agent rendering ---

fn render_sub_agent_page(app: &FrontendApp, frame: &mut Frame) {
    let Some((selected_entry, agents, editor)) = app.sub_agent_state() else {
        return;
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(layout[0]);

    let mut items = Vec::new();
    for (idx, agent) in agents.iter().enumerate() {
        let style = if idx == selected_entry {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let skill_list = agent.skills.join(", ");
        items.push(
            ListItem::new(Line::raw(format!(
                "{} | skills: [{}] | {}",
                agent.name, skill_list, agent.description
            )))
            .style(style),
        );
    }
    let new_idx = agents.len();
    let new_style = if selected_entry == new_idx {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    items.push(ListItem::new(Line::raw("[Create new sub-agent]")).style(new_style));

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Sub-agents (Up/Down select, Enter edit/create)"),
    );
    frame.render_widget(list, top[0]);

    let selected_text = if selected_entry < agents.len() {
        let a = &agents[selected_entry];
        format!(
            "Name: {}\nDescription: {}\nSkills: {}",
            a.name,
            a.description,
            a.skills.join(", ")
        )
    } else {
        "[Create new sub-agent]".to_string()
    };
    let help = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Enter: edit/create sub-agent"),
        Line::raw("- Esc: back to chat"),
        Line::raw(""),
        Line::raw(selected_text),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Details"));
    frame.render_widget(help, top[1]);

    let footer = Paragraph::new(Line::raw(
        "Sub-agents are custom roles that leader can delegate tasks to.",
    ))
    .style(Style::default().fg(Color::Black).bg(Color::Green));
    frame.render_widget(footer, layout[1]);

    if let Some(editor) = editor {
        render_sub_agent_editor(frame, editor);
    }
}

fn render_sub_agent_editor(frame: &mut Frame, editor: &SubAgentEditor) {
    let area = centered_rect(frame.area(), 80, 80);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Sub-agent Editor | Tab switch field | Ctrl+S save | Ctrl+G gen desc");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // name
            Constraint::Min(4),    // skills list
            Constraint::Length(3), // description
            Constraint::Length(3), // hints
        ])
        .split(inner);

    let name_title = if editor.focus == SubAgentEditorFocus::Name {
        "Name (active)"
    } else {
        "Name"
    };
    let name_input = Paragraph::new(editor.name_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(name_title));
    frame.render_widget(name_input, parts[0]);

    // Skills list
    let skills_title = if editor.focus == SubAgentEditorFocus::Skills {
        "Skills (Space toggle, Up/Down navigate)"
    } else {
        "Skills"
    };
    let max_visible = parts[1].height.saturating_sub(2) as usize;
    let start = editor
        .skill_scroll
        .saturating_sub(max_visible.saturating_sub(1));
    let mut skill_items = Vec::new();
    for (idx, skill) in editor
        .available_skills
        .iter()
        .enumerate()
        .skip(start)
        .take(max_visible)
    {
        let checked = if editor.selected_skills.get(idx).copied().unwrap_or(false) {
            "[x]"
        } else {
            "[ ]"
        };
        let style = if idx == editor.skill_scroll && editor.focus == SubAgentEditorFocus::Skills {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        skill_items.push(ListItem::new(Line::raw(format!("{checked} {skill}"))).style(style));
    }
    let skills_list =
        List::new(skill_items).block(Block::default().borders(Borders::ALL).title(skills_title));
    frame.render_widget(skills_list, parts[1]);

    let desc_title = if editor.focus == SubAgentEditorFocus::Description {
        "Description (active) | Ctrl+G auto-generate"
    } else {
        "Description"
    };
    let desc_text = if editor.generating_description {
        "Generating..."
    } else {
        editor.description_input.as_str()
    };
    let desc_input =
        Paragraph::new(desc_text).block(Block::default().borders(Borders::ALL).title(desc_title));
    frame.render_widget(desc_input, parts[2]);

    let hints = Paragraph::new(Text::from(vec![
        Line::raw("Tab: cycle focus | Space: toggle skill | Ctrl+G: auto gen description"),
        Line::raw("Enter/Ctrl+S: save | Esc: cancel"),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(hints, parts[3]);

    match editor.focus {
        SubAgentEditorFocus::Name => place_cursor(
            frame,
            parts[0],
            text_display_width_at(editor.name_input.as_str(), editor.name_cursor),
        ),
        SubAgentEditorFocus::Description => place_cursor(
            frame,
            parts[2],
            text_display_width_at(editor.description_input.as_str(), editor.description_cursor),
        ),
        SubAgentEditorFocus::Skills => {} // no text cursor in skills list
    }
}

// --- Create skill rendering ---

fn render_create_skill_page(app: &FrontendApp, frame: &mut Frame) {
    let Some(editor) = app.create_skill_editor() else {
        return;
    };

    let area = centered_rect(frame.area(), 80, 80);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Create Custom Skill | Tab switch field | Ctrl+S save | Esc cancel");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // name
            Constraint::Length(3), // description
            Constraint::Length(3), // usage
            Constraint::Length(3), // action
            Constraint::Length(3), // prompt_template
            Constraint::Min(1),    // hints
        ])
        .split(inner);

    let fields: [(&str, &str, usize, CreateSkillEditorFocus); 5] = [
        (
            "Name",
            &editor.name_input,
            editor.name_cursor,
            CreateSkillEditorFocus::Name,
        ),
        (
            "Description",
            &editor.description_input,
            editor.description_cursor,
            CreateSkillEditorFocus::Description,
        ),
        (
            "Usage",
            &editor.usage_input,
            editor.usage_cursor,
            CreateSkillEditorFocus::Usage,
        ),
        (
            "Action (prompt/script)",
            &editor.action_input,
            editor.action_cursor,
            CreateSkillEditorFocus::Action,
        ),
        (
            "Prompt Template (use {{input}})",
            &editor.prompt_template_input,
            editor.prompt_template_cursor,
            CreateSkillEditorFocus::PromptTemplate,
        ),
    ];

    for (i, (label, value, _cursor, focus_id)) in fields.iter().enumerate() {
        let title = if editor.focus == *focus_id {
            format!("{label} (active)")
        } else {
            label.to_string()
        };
        let input =
            Paragraph::new(*value).block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(input, parts[i]);
    }

    let hints = Paragraph::new(Text::from(vec![
        Line::raw("Tab: cycle fields | Enter/Ctrl+S: save | Esc: cancel"),
        Line::raw("Skill files are saved to ~/.mylittlebotty/skill/<name>.json"),
        Line::raw("Action 'prompt': uses prompt_template with {{input}} placeholder"),
        Line::raw("Action 'script': runs ~/.mylittlebotty/skill/scripts/<name>.sh"),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(hints, parts[5]);

    // Place cursor on active field
    for (i, (_label, value, cursor, focus_id)) in fields.iter().enumerate() {
        if editor.focus == *focus_id {
            place_cursor(frame, parts[i], text_display_width_at(value, *cursor));
            break;
        }
    }
}

fn spawn_desc_gen_request(name: String, skills: Vec<String>) -> Receiver<io::Result<String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = LocalFrontendRpc::connect().and_then(|mut rpc| {
            let prompt = format!(
                "Generate a short one-sentence description (for the leader to know when to delegate tasks) for a sub-agent named '{}' that has these skills bound: [{}]. Reply with ONLY the description text, nothing else.",
                name, skills.join(", ")
            );
            rpc.send_chat(&prompt)
        });
        let _ = sender.send(result);
    });
    receiver
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
