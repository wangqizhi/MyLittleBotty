use crate::frontend::frontend_app::{
    ChatbotFieldEdit, ChatbotProviderEditor, ChatbotProviderFieldId, ChatbotProviderPanel,
    CreateSkillEditorFocus, FieldEdit, FrontendApp, GuyEnvEditor, GuyEnvEditorFocus, Mode,
    ProfileFieldEdit, Role, SetupOverlay, SetupProfileEditor, SetupProfilePanel,
    SubAgentDeleteConfirm, SubAgentEditor, SubAgentEditorFocus, SubmitOutcome,
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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn run() -> io::Result<()> {
    let mut rpc = LocalFrontendRpc::connect()?;
    let mut app = FrontendApp::new();
    let mut pending_reply: Option<Receiver<io::Result<String>>> = None;
    let mut pending_setup_save: Option<
        Receiver<io::Result<crate::frontend::frontend_service::SaveSetupResult>>,
    > = None;
    let mut pending_desc_gen: Option<Receiver<io::Result<String>>> = None;
    let mut pending_skill_gen: Option<Receiver<io::Result<(String, String, String)>>> = None;

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
        if let Some(receiver) = pending_skill_gen.as_ref() {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok((name, purpose, desc)) => {
                        app.create_skill_editor_set_generated_description(name, purpose, desc)
                    }
                    Err(err) => {
                        app.create_skill_editor_generation_failed();
                        app.push_system(&format!("generate skill description failed: {err}"));
                    }
                }
                pending_skill_gen = None;
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
            if let Some((name, purpose)) =
                handle_create_skill_key(&mut app, key, pending_skill_gen.is_some())
            {
                pending_skill_gen = Some(spawn_skill_desc_gen_request(name, purpose));
            }
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

    let profile_editor_open = matches!(
        app.mode(),
        Mode::Setup {
            overlay: Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                editor: Some(_),
                ..
            })),
            ..
        }
    );

    let chatbot_editor_open = matches!(
        app.mode(),
        Mode::Setup {
            overlay: Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                editor: Some(_),
                ..
            })),
            ..
        }
    );

    let editor_open = matches!(
        app.mode(),
        Mode::Setup {
            overlay: Some(SetupOverlay::Field(_)),
            ..
        }
    ) || matches!(
        app.mode(),
        Mode::Setup {
            overlay: Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                editor: Some(SetupProfileEditor {
                    field_editor: Some(_),
                    ..
                }),
                ..
            })),
            ..
        }
    ) || matches!(
        app.mode(),
        Mode::Setup {
            overlay: Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                editor: Some(ChatbotProviderEditor {
                    field_editor: Some(_),
                    ..
                }),
                ..
            })),
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
            KeyCode::Tab => {
                if matches!(app.mode(), Mode::GuyEnvEdit { .. }) {
                    app.toggle_guy_env_editor_focus();
                }
            }
            KeyCode::BackTab => {
                if matches!(app.mode(), Mode::GuyEnvEdit { .. }) {
                    app.toggle_guy_env_editor_focus();
                }
            }
            KeyCode::Enter => app.editor_submit(rpc),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.editor_insert(c)
            }
            _ => {}
        }
        return Ok(None);
    }

    if profile_editor_open {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => app.editor_submit(rpc),
                _ => {}
            }
            return Ok(None);
        }
    }

    if chatbot_editor_open {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => app.editor_submit(rpc),
                _ => {}
            }
            return Ok(None);
        }
    }

    match app.mode() {
        Mode::Setup { overlay, .. } => match key.code {
            KeyCode::Esc => {
                if overlay.is_some() {
                    app.setup_close_overlay();
                } else {
                    app.cancel_setup();
                }
            }
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 's' || c == 'S') =>
            {
                if let Some(config) = app.begin_setup_save() {
                    return Ok(Some(spawn_setup_save_request(config)));
                }
            }
            KeyCode::Up => {
                if matches!(
                    overlay,
                    Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_ai_profile_editor_prev_field();
                } else if matches!(
                    overlay,
                    Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_chatbot_provider_editor_prev_field();
                } else if matches!(overlay, Some(SetupOverlay::ChatbotProviders(_))) {
                    app.setup_chatbot_providers_prev();
                } else if overlay.is_some() {
                    app.setup_ai_profiles_prev();
                } else {
                    app.setup_prev_field();
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if matches!(
                    overlay,
                    Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_ai_profile_editor_next_field();
                } else if matches!(
                    overlay,
                    Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_chatbot_provider_editor_next_field();
                } else if matches!(overlay, Some(SetupOverlay::ChatbotProviders(_))) {
                    app.setup_chatbot_providers_next();
                } else if overlay.is_some() {
                    app.setup_ai_profiles_next();
                } else {
                    app.setup_next_field();
                }
            }
            KeyCode::BackTab => {
                if matches!(
                    overlay,
                    Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_ai_profile_editor_prev_field();
                } else if matches!(
                    overlay,
                    Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_chatbot_provider_editor_prev_field();
                } else if matches!(overlay, Some(SetupOverlay::ChatbotProviders(_))) {
                    app.setup_chatbot_providers_prev();
                } else if overlay.is_some() {
                    app.setup_ai_profiles_prev();
                } else {
                    app.setup_prev_field();
                }
            }
            KeyCode::Left => {}
            KeyCode::Right => {}
            KeyCode::Enter => {
                if matches!(
                    overlay,
                    Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_ai_profile_editor_activate();
                } else if matches!(
                    overlay,
                    Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_chatbot_provider_editor_activate();
                } else if matches!(overlay, Some(SetupOverlay::ChatbotProviders(_))) {
                    app.setup_chatbot_providers_edit_selected();
                } else if overlay.is_some() {
                    app.setup_ai_profiles_edit_selected();
                } else {
                    app.setup_activate();
                }
            }
            KeyCode::Char(' ') => {
                if matches!(
                    overlay,
                    Some(SetupOverlay::AiProfiles(SetupProfilePanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_ai_profile_editor_toggle_selected();
                } else if matches!(
                    overlay,
                    Some(SetupOverlay::ChatbotProviders(ChatbotProviderPanel {
                        editor: Some(_),
                        ..
                    }))
                ) {
                    app.setup_chatbot_provider_editor_toggle_selected();
                } else {
                    app.setup_toggle_selected();
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if matches!(overlay, Some(SetupOverlay::AiProfiles(_))) {
                    app.setup_ai_profiles_activate();
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if matches!(overlay, Some(SetupOverlay::AiProfiles(_))) {
                    app.setup_ai_profiles_delete_selected();
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if matches!(overlay, Some(SetupOverlay::AiProfiles(_))) {
                    app.setup_ai_profiles_new();
                } else {
                    app.setup_open_ai_profiles();
                    app.setup_ai_profiles_new();
                }
            }
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
            selected_provider: _,
            overlay,
            config,
            ..
        } => render_setup_page(
            frame,
            *selected_field,
            overlay.as_ref(),
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
    overlay: Option<&SetupOverlay>,
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

    let side = Paragraph::new(Text::from(vec![
        Line::raw("Actions:"),
        Line::raw("- Ctrl+S: Save and return"),
        Line::raw("- Esc: Cancel"),
        Line::raw("- Tab / Shift+Tab: Next/Prev field"),
        Line::raw("- Enter on AI profiles opens profile manager"),
        Line::raw("- remember-hourly enabled toggles the built-in /remember system task"),
        Line::raw("- Enter on chatbot providers opens provider manager"),
        Line::raw(""),
        Line::raw("Work dir:"),
        Line::raw("- default: ~/opt/mylittlebotty-workdir"),
        Line::raw("- changing it migrates current work-dir contents"),
        Line::raw(""),
        Line::raw("Chatbot provider:"),
        Line::raw(format!("- {}", CHATBOT_PROVIDERS.join(", "))),
        Line::raw(format!("- current: {}", config.chatbot_provider)),
        Line::raw("- enabled providers are shown in the field value"),
        Line::raw("- configure provider details inside the panel"),
        Line::raw("- telegram whitelist user_ids supports comma-separated IDs"),
        Line::raw("- feishu input uses long connection and needs app id + app secret"),
        Line::raw("- feishu chat id is only needed for proactive push like reminders"),
        Line::raw("- weixin panel supports enable/apikey/account_id/user_id edits"),
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

    if let Some(overlay) = overlay {
        match overlay {
            SetupOverlay::Field(editor) => render_field_editor(frame, editor),
            SetupOverlay::AiProfiles(panel) => render_ai_profiles_panel(frame, panel, config),
            SetupOverlay::ChatbotProviders(panel) => {
                render_chatbot_providers_panel(frame, panel, config)
            }
        }
    }
}

fn render_chatbot_providers_panel(
    frame: &mut Frame,
    panel: &ChatbotProviderPanel,
    config: &SetupConfig,
) {
    let area = centered_rect(frame.area(), 80, 74);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Chatbot Providers | Enter Edit | Up/Down Switch Provider | Esc Close");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(inner);

    let provider_items: Vec<ListItem> = CHATBOT_PROVIDERS
        .iter()
        .map(|provider| {
            let enabled = match *provider {
                "telegram" => config.chatbot_telegram_enabled,
                "feishu" => config.chatbot_feishu_enabled,
                "weixin" => config.chatbot_weixin_enabled,
                _ => false,
            };
            let suffix = if enabled { " [enabled]" } else { "" };
            let style = if *provider == config.chatbot_provider {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::raw(format!("{provider}{suffix}"))).style(style)
        })
        .collect();

    frame.render_widget(
        List::new(provider_items).block(Block::default().borders(Borders::ALL).title("Providers")),
        parts[0],
    );

    let details: Vec<Line> =
        chatbot_provider_detail_lines(config, config.chatbot_provider.as_str());
    frame.render_widget(
        Paragraph::new(Text::from(details))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Details")),
        parts[1],
    );

    if let Some(editor) = panel.editor.as_ref() {
        render_chatbot_provider_editor(frame, editor, config);
    }
    if let Some(message) = panel.message.as_deref() {
        render_message_modal(frame, "Chatbot Provider Message", message);
    }
}

fn render_ai_profiles_panel(frame: &mut Frame, panel: &SetupProfilePanel, config: &SetupConfig) {
    let area = centered_rect(frame.area(), 78, 70);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("AI Profiles | Enter Edit | a Activate | n New | d Delete | Esc Close");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(inner);

    let mut items = Vec::new();
    let image_profile_name = config.image_ai_profile_name();
    for (idx, profile) in config.ai_provider_profiles.iter().enumerate() {
        let mut suffix = String::new();
        if profile.name == config.ai_provider_active {
            suffix.push_str(" [active]");
            if image_profile_name == Some(profile.name.as_str()) {
                suffix.push_str("[img]");
            }
        } else if image_profile_name == Some(profile.name.as_str()) {
            suffix.push_str(" [img]");
        }
        let style = if idx == panel.selected_profile {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::raw(format!("{}{}", profile.name, suffix))).style(style));
    }

    let new_index = config.ai_provider_profiles.len();
    items.push(ListItem::new(Line::raw("+ New profile")).style(
        if panel.selected_profile == new_index {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        },
    ));

    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Profiles")),
        parts[0],
    );

    let detail_lines = if panel.selected_profile < config.ai_provider_profiles.len() {
        let profile = &config.ai_provider_profiles[panel.selected_profile];
        vec![
            Line::raw(format!("name: {}", profile.name)),
            Line::raw(format!("endpoint: {}", profile.endpoint)),
            Line::raw(format!("apikey: {}", mask_secret(&profile.apikey))),
            Line::raw(format!("model: {}", profile.model)),
            Line::raw(format!(
                "debug: {}",
                if profile.debug {
                    "[x] true"
                } else {
                    "[ ] false"
                }
            )),
            Line::raw(format!(
                "image support: {}",
                if profile.vision {
                    "[x] true"
                } else {
                    "[ ] false"
                }
            )),
            Line::raw(""),
            Line::raw("Deleting the active profile is blocked."),
            Line::raw("Switch to another profile before deleting."),
        ]
    } else {
        vec![
            Line::raw("Create a new AI profile."),
            Line::raw(""),
            Line::raw("Suggested fields:"),
            Line::raw("- name"),
            Line::raw("- endpoint"),
            Line::raw("- apikey"),
            Line::raw("- model"),
            Line::raw("- debug"),
            Line::raw("- image support"),
        ]
    };
    frame.render_widget(
        Paragraph::new(Text::from(detail_lines))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Details")),
        parts[1],
    );

    if let Some(editor) = panel.editor.as_ref() {
        render_ai_profile_editor(frame, editor);
    }
    if let Some(message) = panel.message.as_deref() {
        render_message_modal(frame, "AI Profile Message", message);
    }
}

fn render_ai_profile_editor(frame: &mut Frame, editor: &SetupProfileEditor) {
    let area = centered_rect(frame.area(), 72, 60);
    frame.render_widget(Clear, area);
    let title = if editor.original_name.is_some() {
        "Edit AI Profile | Enter Edit/Toggle | Ctrl+S Save | Esc Cancel"
    } else {
        "New AI Profile | Enter Edit/Toggle | Ctrl+S Save | Esc Cancel"
    };
    frame.render_widget(Block::default().borders(Borders::ALL).title(title), area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let items = vec![
        profile_editor_item(
            "profile name",
            editor.draft.name.as_str(),
            editor.selected_field == 0,
            false,
        ),
        profile_editor_item(
            "endpoint",
            editor.draft.endpoint.as_str(),
            editor.selected_field == 1,
            false,
        ),
        profile_editor_item(
            "apikey",
            editor.draft.apikey.as_str(),
            editor.selected_field == 2,
            true,
        ),
        profile_editor_item(
            "model",
            editor.draft.model.as_str(),
            editor.selected_field == 3,
            false,
        ),
        profile_editor_item(
            "debug",
            if editor.draft.debug {
                "[x] true"
            } else {
                "[ ] false"
            },
            editor.selected_field == 4,
            false,
        ),
        profile_editor_item(
            "image support",
            if editor.draft.vision {
                "[x] true"
            } else {
                "[ ] false"
            },
            editor.selected_field == 5,
            false,
        ),
    ];

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Profile Fields"),
        ),
        inner,
    );

    if let Some(field_editor) = editor.field_editor.as_ref() {
        render_profile_field_editor(frame, field_editor);
    }
}

fn profile_editor_item<'a>(
    label: &'a str,
    value: &'a str,
    selected: bool,
    masked: bool,
) -> ListItem<'a> {
    let shown = if masked {
        mask_secret(value)
    } else {
        value.to_string()
    };
    ListItem::new(Line::raw(format!("{label}: {shown}"))).style(if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    })
}

fn render_profile_field_editor(frame: &mut Frame, editor: &ProfileFieldEdit) {
    let area = centered_rect(frame.area(), 68, 24);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit AI Profile Field | Ctrl+A Home | <- -> Move | Ctrl+C Clear");
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
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::raw(format!("Field: {label}")),
            Line::raw("Enter: save field"),
            Line::raw("Esc: cancel field editing"),
        ]))
        .wrap(Wrap { trim: false }),
        parts[0],
    );

    frame.render_widget(
        Paragraph::new(editor.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(label)),
        parts[1],
    );
    place_cursor(
        frame,
        parts[1],
        text_display_width_at(editor.input.as_str(), editor.cursor),
    );
}

fn render_chatbot_provider_editor(
    frame: &mut Frame,
    editor: &ChatbotProviderEditor,
    config: &SetupConfig,
) {
    let area = centered_rect(frame.area(), 72, 68);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("Edit Chatbot Provider | Enter Edit/Toggle | Ctrl+S Save | Esc Cancel"),
        area,
    );

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let items: Vec<ListItem> = chatbot_provider_field_ids(editor.provider.as_str())
        .iter()
        .enumerate()
        .map(|(idx, field)| {
            let value = chatbot_provider_display_value(config, editor.provider.as_str(), *field);
            let shown = if field.is_masked() {
                mask_secret(&value)
            } else {
                value
            };
            ListItem::new(Line::raw(format!(
                "{}: {}",
                field.label(editor.provider.as_str()),
                shown
            )))
            .style(if editor.selected_field == idx {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} fields", editor.provider)),
        ),
        inner,
    );

    if let Some(field_editor) = editor.field_editor.as_ref() {
        render_chatbot_field_editor(frame, field_editor, editor.provider.as_str());
    }
}

fn render_chatbot_field_editor(frame: &mut Frame, editor: &ChatbotFieldEdit, provider: &str) {
    let area = centered_rect(frame.area(), 68, 24);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit Chatbot Field | Ctrl+A Home | <- -> Move | Ctrl+C Clear");
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

    let label = editor.selected_field.label(provider);
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::raw(format!("Field: {label}")),
            Line::raw("Enter: save field"),
            Line::raw("Esc: cancel field editing"),
        ]))
        .wrap(Wrap { trim: false }),
        parts[0],
    );

    frame.render_widget(
        Paragraph::new(editor.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(label)),
        parts[1],
    );
    place_cursor(
        frame,
        parts[1],
        text_display_width_at(editor.input.as_str(), editor.cursor),
    );
}

fn chatbot_provider_field_ids(provider: &str) -> &'static [ChatbotProviderFieldId] {
    ChatbotProviderFieldId::fields_for(provider)
}

fn chatbot_provider_display_value(
    config: &SetupConfig,
    provider: &str,
    field: ChatbotProviderFieldId,
) -> String {
    match (provider, field) {
        ("telegram", ChatbotProviderFieldId::Enabled) => {
            checkbox_text(config.chatbot_telegram_enabled)
        }
        ("telegram", ChatbotProviderFieldId::ApiBase) => config.chatbot_telegram_api_base.clone(),
        ("telegram", ChatbotProviderFieldId::Token) => config.chatbot_telegram_apikey.clone(),
        ("telegram", ChatbotProviderFieldId::PollSeconds) => {
            config.chatbot_telegram_poll_interval_seconds.to_string()
        }
        ("telegram", ChatbotProviderFieldId::WhitelistUserIds) => {
            config.chatbot_telegram_whitelist_user_ids.clone()
        }
        ("feishu", ChatbotProviderFieldId::Enabled) => checkbox_text(config.chatbot_feishu_enabled),
        ("feishu", ChatbotProviderFieldId::ApiBase) => config.chatbot_feishu_api_base.clone(),
        ("feishu", ChatbotProviderFieldId::AppId) => config.chatbot_feishu_app_id.clone(),
        ("feishu", ChatbotProviderFieldId::AppSecret) => config.chatbot_feishu_app_secret.clone(),
        ("feishu", ChatbotProviderFieldId::Token) => config.chatbot_feishu_access_token.clone(),
        ("feishu", ChatbotProviderFieldId::PollSeconds) => {
            config.chatbot_feishu_poll_interval_seconds.to_string()
        }
        ("feishu", ChatbotProviderFieldId::ChatId) => config.chatbot_feishu_chat_id.clone(),
        ("weixin", ChatbotProviderFieldId::Enabled) => checkbox_text(config.chatbot_weixin_enabled),
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

fn chatbot_provider_detail_lines(config: &SetupConfig, provider: &str) -> Vec<Line<'static>> {
    chatbot_provider_field_ids(provider)
        .iter()
        .map(|field| {
            let value = chatbot_provider_display_value(config, provider, *field);
            let shown = if field.is_masked() {
                mask_secret(&value)
            } else {
                value
            };
            Line::raw(format!("{}: {}", field.label(provider), shown))
        })
        .collect()
}

fn checkbox_text(value: bool) -> String {
    if value {
        "[x] true".to_string()
    } else {
        "[ ] false".to_string()
    }
}

fn render_message_modal(frame: &mut Frame, title: &str, message: &str) {
    let area = centered_rect(frame.area(), 62, 22);
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().borders(Borders::ALL).title(title), area);
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::raw(message),
            Line::raw(""),
            Line::raw("Press Esc to close."),
        ]))
        .wrap(Wrap { trim: false }),
        inner,
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

fn text_cursor_position_wrapped(text: &str, cursor: usize, wrap_width: u16) -> (u16, u16) {
    let wrap_width = wrap_width.max(1) as usize;
    let mut row = 0u16;
    let mut col = 0usize;

    for ch in text[..cursor.min(text.len())].chars() {
        if ch == '\n' {
            row = row.saturating_add(1);
            col = 0;
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if col + ch_width > wrap_width {
            row = row.saturating_add(1);
            col = 0;
        }
        col += ch_width;
        if col >= wrap_width {
            row = row.saturating_add(1);
            col = 0;
        }
    }

    ((col.min(u16::MAX as usize - 1) as u16) + 1, row)
}

fn place_cursor(frame: &mut Frame, input_rect: Rect, desired_col: u16) {
    let max_col = input_rect.width.saturating_sub(2);
    let x = input_rect.x + desired_col.min(max_col);
    let y = input_rect.y + 1;
    frame.set_cursor_position((x, y));
}

fn place_multiline_cursor(frame: &mut Frame, input_rect: Rect, desired_col: u16, desired_row: u16) {
    let max_col = input_rect.width.saturating_sub(2);
    let max_row = input_rect.height.saturating_sub(2);
    let x = input_rect.x + desired_col.min(max_col);
    let y = input_rect.y + 1 + desired_row.min(max_row);
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
    let (has_editor, has_confirm_delete) = match app.mode() {
        Mode::SubAgent {
            editor,
            confirm_delete,
            ..
        } => (editor.is_some(), confirm_delete.is_some()),
        _ => (false, false),
    };

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

    if has_confirm_delete {
        match key.code {
            KeyCode::Esc => app.sub_agent_cancel_delete(),
            KeyCode::Enter => app.sub_agent_confirm_delete(),
            KeyCode::Char('y') | KeyCode::Char('Y') => app.sub_agent_confirm_delete(),
            KeyCode::Char('n') | KeyCode::Char('N') => app.sub_agent_cancel_delete(),
            _ => {}
        }
        return None;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('d') | KeyCode::Char('D') => app.sub_agent_request_delete(),
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

// --- Sub-agent rendering ---

fn handle_create_skill_key(
    app: &mut FrontendApp,
    key: KeyEvent,
    generation_pending: bool,
) -> Option<(String, String)> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => app.create_skill_editor_move_home(),
            KeyCode::Char('c') | KeyCode::Char('C') => app.create_skill_editor_clear(),
            KeyCode::Char('s') | KeyCode::Char('S') => app.create_skill_editor_save(),
            KeyCode::Char('g') | KeyCode::Char('G') if !generation_pending => {
                return app.create_skill_editor_begin_generate();
            }
            _ => {}
        }
        return None;
    }

    match key.code {
        KeyCode::Esc => app.close_panel(),
        KeyCode::Tab | KeyCode::BackTab => app.create_skill_editor_cycle_focus(),
        KeyCode::Left => app.create_skill_editor_move_left(),
        KeyCode::Right => app.create_skill_editor_move_right(),
        KeyCode::Backspace => app.create_skill_editor_backspace(),
        KeyCode::Delete => app.create_skill_editor_delete(),
        KeyCode::Enter => {
            if matches!(
                app.create_skill_editor().map(|editor| editor.focus),
                Some(CreateSkillEditorFocus::Purpose)
            ) {
                app.create_skill_editor_insert('\n');
            }
        }
        KeyCode::Char(c) => app.create_skill_editor_insert(c),
        KeyCode::F(2) => app.create_skill_editor_save(),
        _ => {}
    }
    None
}

fn render_sub_agent_page(app: &FrontendApp, frame: &mut Frame) {
    let Some((selected_entry, agents, editor, confirm_delete)) = app.sub_agent_state() else {
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
        Line::raw("- Ctrl+D: delete selected sub-agent"),
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
    if let Some(confirm_delete) = confirm_delete {
        render_sub_agent_delete_confirm(frame, confirm_delete);
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

fn render_sub_agent_delete_confirm(frame: &mut Frame, confirm: &SubAgentDeleteConfirm) {
    let area = centered_rect(frame.area(), 52, 22);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Delete Sub-agent?");
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let text = Paragraph::new(Text::from(vec![
        Line::raw(format!("Delete sub-agent '{}' ?", confirm.name)),
        Line::raw(""),
        Line::raw("Enter / Y: confirm delete"),
        Line::raw("Esc / N: cancel"),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(text, inner);
}

fn render_create_skill_page(app: &FrontendApp, frame: &mut Frame) {
    let Some(editor) = app.create_skill_editor() else {
        return;
    };
    let Some((normalized_name, generated_description, target_path, generating_description)) =
        app.create_skill_preview()
    else {
        return;
    };

    let area = centered_rect(frame.area(), 80, 80);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Create Custom Skill | Tab switch field | Ctrl+G generate | Ctrl+S save");
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
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(4),
        ])
        .split(inner);

    let name_title = if editor.focus == CreateSkillEditorFocus::Name {
        "Step 1: Skill Name (active)"
    } else {
        "Step 1: Skill Name"
    };
    let name_input = Paragraph::new(editor.name_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(name_title));
    frame.render_widget(name_input, parts[0]);

    let purpose_title = if editor.focus == CreateSkillEditorFocus::Purpose {
        "Step 2: Describe What The Skill Does (active)"
    } else {
        "Step 2: Describe What The Skill Does"
    };
    let purpose_input = Paragraph::new(editor.purpose_input.as_str())
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(purpose_title));
    frame.render_widget(purpose_input, parts[1]);

    let skill_description = if generating_description {
        "Generating...".to_string()
    } else if generated_description.trim().is_empty() {
        "(Press Ctrl+G to auto-generate skill description)".to_string()
    } else {
        generated_description
    };

    let preview = Paragraph::new(Text::from(vec![
        Line::raw(format!(
            "Name: {}",
            if normalized_name.is_empty() {
                "<invalid>"
            } else {
                &normalized_name
            }
        )),
        Line::raw(format!("Skill description: {skill_description}")),
        Line::raw(format!("Target file: {target_path}")),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Preview"));
    frame.render_widget(preview, parts[2]);

    let hints = Paragraph::new(Text::from(vec![
        Line::raw(
            "Tab: switch field | Enter: newline in Step 2 | Ctrl+G: auto-generate description",
        ),
        Line::raw("Ctrl+S: save | Esc: cancel"),
        Line::raw("Saved to ~/.mylittlebotty/skill/<name>.json"),
    ]))
    .wrap(Wrap { trim: false })
    .block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(hints, parts[3]);

    match editor.focus {
        CreateSkillEditorFocus::Name => place_cursor(
            frame,
            parts[0],
            text_display_width_at(editor.name_input.as_str(), editor.name_cursor),
        ),
        CreateSkillEditorFocus::Purpose => {
            let wrap_width = parts[1].width.saturating_sub(2);
            let (col, row) = text_cursor_position_wrapped(
                editor.purpose_input.as_str(),
                editor.purpose_cursor,
                wrap_width,
            );
            place_multiline_cursor(frame, parts[1], col, row);
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

fn spawn_skill_desc_gen_request(
    name: String,
    purpose: String,
) -> Receiver<io::Result<(String, String, String)>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let request_name = name.clone();
        let request_purpose = purpose.clone();
        let result = LocalFrontendRpc::connect().and_then(|mut rpc| {
            let prompt = format!(
                "Generate a short one-sentence description for a custom skill named '{}' that does the following: {}. Reply with ONLY the description text, nothing else.",
                name, purpose
            );
            rpc.send_chat(&prompt)
                .map(|description| (request_name, request_purpose, description))
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
