use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use unicode_width::UnicodeWidthStr;

use crate::event::{RatatuiEvent, RatatuiEventStream};
use crate::frame::FrameRequester;
use crate::terminal::{AlternateScrollGuard, TerminalModeGuard};

#[derive(Debug, Clone)]
pub struct ChatLine {
    role: &'static str,
    text: String,
}

impl ChatLine {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "You",
            text: text.into(),
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "Assistant",
            text: text.into(),
        }
    }

    #[must_use]
    pub fn status(text: impl Into<String>) -> Self {
        Self {
            role: "Status",
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub title: String,
    pub model_label: String,
    pub status: String,
    pub resource_summary: String,
    pub command_hints: Vec<String>,
    pub key_hints: Vec<String>,
    pub slash_commands: Vec<SlashCommandItem>,
}

impl ChatOptions {
    #[must_use]
    pub fn new(model_label: impl Into<String>) -> Self {
        Self {
            title: "Pi".to_string(),
            model_label: model_label.into(),
            status: "就绪".to_string(),
            resource_summary: "资源: 0 技能, 0 提示, 0 主题".to_string(),
            command_hints: Vec::new(),
            key_hints: vec![
                "Enter: 发送".to_string(),
                "Shift+Enter: newline".to_string(),
                "Ctrl+C/Esc: 退出".to_string(),
                "/help".to_string(),
            ],
            slash_commands: default_slash_commands(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlashCommandItem {
    pub command: String,
    pub description: String,
}

impl SlashCommandItem {
    #[must_use]
    pub fn new(command: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PickerItem {
    pub label: String,
    pub value: String,
    pub description: String,
}

impl PickerItem {
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatPicker {
    pub title: String,
    pub prompt_prefix: String,
    pub items: Vec<PickerItem>,
}

impl ChatPicker {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        prompt_prefix: impl Into<String>,
        items: Vec<PickerItem>,
    ) -> Self {
        Self {
            title: title.into(),
            prompt_prefix: prompt_prefix.into(),
            items,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChatAction {
    PushLine(ChatLine),
    Clear,
    Quit,
    SetOptions(ChatOptions),
    OpenPicker(ChatPicker),
    Many(Vec<Self>),
}

#[derive(Debug)]
struct ChatApp {
    options: ChatOptions,
    input: String,
    lines: Vec<ChatLine>,
    busy: bool,
    should_quit: bool,
    picker: Option<PickerState>,
    last_escape: Option<Instant>,
}

impl ChatApp {
    const fn new(options: ChatOptions) -> Self {
        Self {
            options,
            input: String::new(),
            lines: Vec::new(),
            busy: false,
            should_quit: false,
            picker: None,
            last_escape: None,
        }
    }

    fn take_submitted_input(&mut self) -> Option<String> {
        let input = self.input.trim().to_string();
        if input.is_empty() {
            return None;
        }
        self.input.clear();
        self.lines.push(ChatLine::user(input.clone()));
        Some(input)
    }

    fn push_line(&mut self, line: ChatLine) {
        self.lines.push(line);
    }

    fn apply_action(&mut self, action: ChatAction) {
        match action {
            ChatAction::PushLine(line) => self.push_line(line),
            ChatAction::Clear => self.lines.clear(),
            ChatAction::Quit => self.should_quit = true,
            ChatAction::SetOptions(options) => self.options = options,
            ChatAction::OpenPicker(picker) => self.picker = Some(PickerState::new(picker)),
            ChatAction::Many(actions) => {
                for action in actions {
                    self.apply_action(action);
                }
            }
        }
    }

    fn handle_event(&mut self, event: RatatuiEvent) -> EventOutcome {
        match event {
            RatatuiEvent::Key(key) => self.handle_key(key),
            RatatuiEvent::Paste(text) => {
                self.input.push_str(&text);
                EventOutcome::None
            }
            RatatuiEvent::Resize | RatatuiEvent::Draw | RatatuiEvent::FocusGained => {
                EventOutcome::None
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> EventOutcome {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return EventOutcome::None;
        }

        if self.picker.is_some() {
            return self.handle_picker_key(key);
        }

        match key.code {
            KeyCode::Esc => {
                let now = Instant::now();
                let double_escape = self
                    .last_escape
                    .is_some_and(|last| now.duration_since(last) <= Duration::from_millis(450));
                self.last_escape = Some(now);
                if double_escape {
                    self.input = "/history".to_string();
                    return EventOutcome::Submit(self.input.clone());
                }
            }
            KeyCode::Tab if self.input.trim_start().starts_with('/') => {
                if let Some(item) = self.filtered_slash_commands().first() {
                    self.input = item.command.clone();
                    self.input.push(' ');
                }
            }
            KeyCode::Enter => {
                if self.input.trim_start().starts_with('/') {
                    if let Some(item) = self.filtered_slash_commands().first() {
                        let current_command = slash_command_token(&self.input);
                        if current_command != item.command {
                            self.input = item.command.clone();
                            self.input.push(' ');
                            return EventOutcome::None;
                        }
                    }
                }
                if let Some(input) = self.take_submitted_input() {
                    return EventOutcome::Submit(input);
                }
            }
            KeyCode::Char(ch) => {
                self.last_escape = None;
                self.input.push(ch);
            }
            KeyCode::Backspace => {
                self.last_escape = None;
                self.input.pop();
            }
            _ => {}
        }
        EventOutcome::None
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> EventOutcome {
        let Some(picker) = self.picker.as_mut() else {
            return EventOutcome::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
            }
            KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                picker.selected = picker
                    .selected
                    .saturating_add(1)
                    .min(picker.filtered_items().len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.clamp_selection();
            }
            KeyCode::Char(ch) => {
                picker.query.push(ch);
                picker.selected = 0;
            }
            KeyCode::Tab | KeyCode::Enter => {
                let command = picker.selected_item().map(|item| {
                    format!("{} {}", picker.picker.prompt_prefix, item.value)
                        .trim()
                        .to_string()
                });
                self.picker = None;
                if let Some(command) = command {
                    self.input.clone_from(&command);
                    return EventOutcome::Submit(command);
                }
            }
            _ => {}
        }
        EventOutcome::None
    }

    fn filtered_slash_commands(&self) -> Vec<&SlashCommandItem> {
        let token = slash_command_token(&self.input).to_ascii_lowercase();
        self.options
            .slash_commands
            .iter()
            .filter(|item| {
                token.is_empty() || item.command.to_ascii_lowercase().starts_with(&token)
            })
            .collect()
    }

    const fn should_quit(&self) -> bool {
        self.should_quit
    }
}

fn slash_command_token(input: &str) -> String {
    input
        .trim_start()
        .split_once(char::is_whitespace)
        .map_or_else(
            || input.trim().to_string(),
            |(command, _)| command.to_string(),
        )
}

#[derive(Debug)]
struct PickerState {
    picker: ChatPicker,
    query: String,
    selected: usize,
}

impl PickerState {
    const fn new(picker: ChatPicker) -> Self {
        Self {
            picker,
            query: String::new(),
            selected: 0,
        }
    }

    fn filtered_items(&self) -> Vec<&PickerItem> {
        let query = self.query.trim().to_ascii_lowercase();
        let mut items = self
            .picker
            .items
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.label.to_ascii_lowercase().contains(&query)
                    || item.value.to_ascii_lowercase().contains(&query)
                    || item.description.to_ascii_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        items.truncate(50);
        items
    }

    fn selected_item(&self) -> Option<&PickerItem> {
        let items = self.filtered_items();
        items
            .get(self.selected.min(items.len().saturating_sub(1)))
            .copied()
    }

    fn clamp_selection(&mut self) {
        let len = self.filtered_items().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EventOutcome {
    None,
    Submit(String),
}

pub type SubmitFuture = Pin<Box<dyn Future<Output = anyhow::Result<ChatAction>> + Send>>;

pub async fn run_minimal_chat_loop(
    options: ChatOptions,
    initial_lines: Vec<ChatLine>,
    mut on_submit: impl FnMut(String) -> SubmitFuture + Send,
) -> anyhow::Result<()> {
    let _terminal_guard = TerminalModeGuard::enter()?;
    let _alternate_scroll = AlternateScrollGuard::enable()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (draw_tx, draw_rx) = broadcast::channel(32);
    let frame_requester = FrameRequester::new(draw_tx);
    let mut events = RatatuiEventStream::new(draw_rx);
    let mut app = ChatApp::new(options);

    for line in initial_lines {
        app.push_line(line);
    }
    frame_requester.schedule_frame();

    terminal.draw(|frame| render(frame, &app))?;
    loop {
        if app.should_quit() {
            break;
        }

        let Some(event) = events.next().await else {
            break;
        };

        let outcome = app.handle_event(event);
        if let EventOutcome::Submit(input) = outcome {
            app.busy = true;
            frame_requester.schedule_frame();
            terminal.draw(|frame| render(frame, &app))?;
            match on_submit(input).await {
                Ok(action) => app.apply_action(action),
                Err(err) => app.push_line(ChatLine::status(format!("Error: {err}"))),
            }
            app.busy = false;
        }
        frame_requester.schedule_frame();

        terminal.draw(|frame| render(frame, &app))?;
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &ChatApp) {
    let area = frame.area();
    let [body, input, footer] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);

    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default().fg(Color::Cyan);

    let mut body_lines = Vec::new();
    for line in &app.lines {
        body_lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", line.role),
                match line.role {
                    "Assistant" => Style::default().fg(Color::Green),
                    "Status" => dim,
                    _ => accent,
                }
                .add_modifier(Modifier::BOLD),
            ),
            Span::raw(line.text.as_str()),
        ]));
    }
    frame.render_widget(
        Paragraph::new(body_lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(dim)
                    .border_type(BorderType::Plain),
            )
            .wrap(Wrap { trim: false }),
        body,
    );

    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(dim)
                .title(Span::styled(" 输入 ", accent)),
        ),
        input,
    );
    let input_cursor_x = input
        .x
        .saturating_add(u16::try_from(app.input.as_str().width()).unwrap_or(u16::MAX))
        .min(input.right().saturating_sub(1));
    frame.set_cursor_position(Position::new(input_cursor_x, input.y.saturating_add(1)));

    let status = if app.busy {
        "运行中"
    } else {
        app.options.status.as_str()
    };
    frame.render_widget(Paragraph::new(footer_line(app, status)).style(dim), footer);

    if let Some(picker) = &app.picker {
        render_picker(frame, app, picker, body);
    } else if app.input.trim_start().starts_with('/') {
        render_slash_completion(frame, app, input);
    }
}

fn footer_line(app: &ChatApp, status: &str) -> String {
    if app.input.trim_start().starts_with('/') {
        let hints = if app.options.command_hints.is_empty() {
            vec![
                "/help".to_string(),
                "/model".to_string(),
                "/thinking".to_string(),
                "/session".to_string(),
                "/tree".to_string(),
                "/codegraph status".to_string(),
            ]
        } else {
            app.options.command_hints.clone()
        };
        return format!("commands: {}", hints.join("  "));
    }
    let hints = if app.options.key_hints.is_empty() {
        String::new()
    } else {
        format!("  |  {}", app.options.key_hints.join("  |  "))
    };
    format!(
        "{}  |  {}  |  模型: {}  |  {}{}",
        app.options.title, status, app.options.model_label, app.options.resource_summary, hints
    )
}

fn render_slash_completion(frame: &mut ratatui::Frame<'_>, app: &ChatApp, input_area: Rect) {
    let items = app.filtered_slash_commands();
    if items.is_empty() {
        return;
    }
    let height = u16::try_from(items.len().min(8) + 2).unwrap_or(10);
    let area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(height),
        width: input_area.width.min(96),
        height,
    };
    let lines = items
        .iter()
        .take(8)
        .enumerate()
        .map(|(idx, item)| {
            let style = if idx == 0 {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(item.command.as_str(), style),
                Span::raw("  "),
                Span::styled(
                    item.description.as_str(),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" commands "),
        ),
        area,
    );
}

fn render_picker(frame: &mut ratatui::Frame<'_>, _app: &ChatApp, picker: &PickerState, body: Rect) {
    let width = body.width.saturating_sub(4).min(110);
    let height = body.height.saturating_sub(2).clamp(6, 18);
    let area = Rect {
        x: body.x + (body.width.saturating_sub(width)) / 2,
        y: body.y + (body.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let items = picker.filtered_items();
    let visible = usize::from(height.saturating_sub(4));
    let lines = std::iter::once(Line::from(vec![
        Span::styled("filter: ", Style::default().fg(Color::DarkGray)),
        Span::raw(picker.query.as_str()),
    ]))
    .chain(items.iter().take(visible).enumerate().map(|(idx, item)| {
        let selected = idx == picker.selected.min(items.len().saturating_sub(1));
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, style),
            Span::styled(item.label.as_str(), style),
            Span::raw("  "),
            Span::styled(
                item.description.as_str(),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }))
    .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(format!(" {} ", picker.picker.title)),
        ),
        area,
    );
}

fn default_slash_commands() -> Vec<SlashCommandItem> {
    [
        ("/help", "帮助"),
        ("/model", "切换模型"),
        ("/thinking", "设置 thinking level"),
        ("/session", "会话信息"),
        ("/settings", "设置"),
        ("/theme", "主题"),
        ("/resume", "恢复会话"),
        ("/history", "会话列表"),
        ("/tree", "对话树"),
        ("/compact", "压缩上下文"),
        ("/clear", "清屏"),
        ("/reload", "重载资源"),
        ("/template", "Prompt templates"),
        ("/language", "切换语言"),
        ("/codegraph", "管理 codegraph 索引"),
        ("/exit", "退出"),
    ]
    .into_iter()
    .map(|(command, description)| SlashCommandItem::new(command, description))
    .collect()
}
