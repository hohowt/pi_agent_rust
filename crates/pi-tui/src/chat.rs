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
use unicode_segmentation::UnicodeSegmentation;
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
    editor: EditorState,
    lines: Vec<ChatLine>,
    busy: bool,
    should_quit: bool,
    picker: Option<PickerState>,
    last_escape: Option<Instant>,
    scroll: ConversationScroll,
}

impl ChatApp {
    const fn new(options: ChatOptions) -> Self {
        Self {
            options,
            editor: EditorState::new(),
            lines: Vec::new(),
            busy: false,
            should_quit: false,
            picker: None,
            last_escape: None,
            scroll: ConversationScroll::new(),
        }
    }

    fn take_submitted_input(&mut self) -> Option<String> {
        let input = self.editor.text().trim().to_string();
        if input.is_empty() {
            return None;
        }
        self.editor.clear();
        self.lines.push(ChatLine::user(input.clone()));
        self.scroll.mark_content_changed();
        Some(input)
    }

    fn push_line(&mut self, line: ChatLine) {
        self.lines.push(line);
        self.scroll.mark_content_changed();
    }

    fn apply_action(&mut self, action: ChatAction) {
        match action {
            ChatAction::PushLine(line) => self.push_line(line),
            ChatAction::Clear => {
                self.lines.clear();
                self.scroll.scroll_to_bottom();
            }
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
                self.editor.insert_str(&normalize_pasted_text(&text));
                EventOutcome::None
            }
            RatatuiEvent::MouseScrollUp => {
                self.scroll.scroll_up(3);
                EventOutcome::None
            }
            RatatuiEvent::MouseScrollDown => {
                self.scroll.scroll_down(3);
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
                    self.editor.set_text("/history");
                    return EventOutcome::Submit(self.editor.text().to_string());
                }
            }
            KeyCode::Tab if self.editor.text().trim_start().starts_with('/') => {
                if let Some(item) = self.filtered_slash_commands().first() {
                    self.editor.set_text(format!("{} ", item.command));
                }
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.editor.insert_char('\n');
                    return EventOutcome::None;
                }
                if self.editor.text().trim_start().starts_with('/') {
                    if let Some(item) = self.filtered_slash_commands().first() {
                        let current_command = slash_command_token(self.editor.text());
                        if current_command != item.command {
                            self.editor.set_text(format!("{} ", item.command));
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
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match ch {
                        'a' => self.editor.move_to_line_start(),
                        'e' => self.editor.move_to_line_end(),
                        'u' => self.editor.delete_to_line_start(),
                        'k' => self.editor.delete_to_line_end(),
                        _ => {}
                    }
                } else if key.modifiers.contains(KeyModifiers::ALT) {
                    match ch {
                        'b' => self.editor.move_word_left(),
                        'f' => self.editor.move_word_right(),
                        'd' => self.editor.delete_word_forward(),
                        _ => {}
                    }
                } else {
                    self.editor.insert_char(ch);
                }
            }
            KeyCode::Backspace => {
                self.last_escape = None;
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.editor.delete_word_backward();
                } else {
                    self.editor.delete_backward();
                }
            }
            KeyCode::Delete => self.editor.delete_forward(),
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.editor.move_word_left();
                } else {
                    self.editor.move_left();
                }
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.editor.move_word_right();
                } else {
                    self.editor.move_right();
                }
            }
            KeyCode::Up => self.editor.move_up(),
            KeyCode::Down => self.editor.move_down(),
            KeyCode::PageUp => self.scroll.scroll_up(10),
            KeyCode::PageDown => self.scroll.scroll_down(10),
            KeyCode::Home => self.scroll.scroll_to_top(),
            KeyCode::End => self.scroll.scroll_to_bottom(),
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
                    self.editor.set_text(command.clone());
                    return EventOutcome::Submit(command);
                }
            }
            _ => {}
        }
        EventOutcome::None
    }

    fn filtered_slash_commands(&self) -> Vec<&SlashCommandItem> {
        let token = slash_command_token(self.input()).to_ascii_lowercase();
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

    fn input(&self) -> &str {
        self.editor.text()
    }
}

#[derive(Debug, Default)]
struct EditorState {
    text: String,
    cursor: usize,
}

impl EditorState {
    const fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn delete_backward(&mut self) {
        let Some(prev) = self.prev_boundary(self.cursor) else {
            return;
        };
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
    }

    fn delete_forward(&mut self) {
        let Some(next) = self.next_boundary(self.cursor) else {
            return;
        };
        self.text.drain(self.cursor..next);
    }

    fn move_left(&mut self) {
        if let Some(prev) = self.prev_boundary(self.cursor) {
            self.cursor = prev;
        }
    }

    fn move_right(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.cursor = next;
        }
    }

    fn move_up(&mut self) {
        let (line_start, column) = self.line_start_and_column();
        if line_start == 0 {
            return;
        }
        let prev_end = line_start.saturating_sub(1);
        let prev_start = self.line_start_before(prev_end);
        self.cursor = self.cursor_for_column(prev_start, prev_end, column);
    }

    fn move_down(&mut self) {
        let (_, column) = self.line_start_and_column();
        let Some(next_start) = self.text[self.cursor..]
            .find('\n')
            .map(|idx| self.cursor + idx + 1)
        else {
            return;
        };
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |idx| next_start + idx);
        self.cursor = self.cursor_for_column(next_start, next_end, column);
    }

    fn move_word_left(&mut self) {
        self.cursor = word_left_boundary(&self.text, self.cursor);
    }

    fn move_word_right(&mut self) {
        self.cursor = word_right_boundary(&self.text, self.cursor);
    }

    fn delete_word_backward(&mut self) {
        let start = word_left_boundary(&self.text, self.cursor);
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    fn delete_word_forward(&mut self) {
        let end = word_right_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..end);
    }

    fn move_to_line_start(&mut self) {
        self.cursor = self.line_start_and_column().0;
    }

    fn move_to_line_end(&mut self) {
        self.cursor = self.line_end_from(self.cursor);
    }

    fn delete_to_line_start(&mut self) {
        let start = self.line_start_and_column().0;
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        let end = self.line_end_from(self.cursor);
        self.text.drain(self.cursor..end);
    }

    fn cursor_position(&self) -> (usize, usize) {
        let (line_start, column) = self.line_start_and_column();
        let row = self.text[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        (row, column)
    }

    fn prev_boundary(&self, at: usize) -> Option<usize> {
        if at == 0 {
            return None;
        }
        self.text[..at]
            .grapheme_indices(true)
            .last()
            .map(|(idx, _)| idx)
    }

    fn next_boundary(&self, at: usize) -> Option<usize> {
        if at >= self.text.len() {
            return None;
        }
        self.text[at..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(Some(self.text.len()), |(idx, _)| Some(at + idx))
    }

    fn line_start_and_column(&self) -> (usize, usize) {
        let line_start = self.line_start_before(self.cursor);
        let column = self.text[line_start..self.cursor].width();
        (line_start, column)
    }

    fn line_start_before(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |idx| idx + 1)
    }

    fn line_end_from(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |idx| at + idx)
    }

    fn cursor_for_column(&self, start: usize, end: usize, target_column: usize) -> usize {
        let mut width = 0;
        for (idx, grapheme) in self.text[start..end].grapheme_indices(true) {
            let next_width = width + grapheme.width();
            if next_width > target_column {
                return start + idx;
            }
            width = next_width;
        }
        end
    }
}

fn word_left_boundary(text: &str, cursor: usize) -> usize {
    let mut boundary = 0;
    for (idx, word) in text[..cursor].unicode_word_indices() {
        if idx + word.len() < cursor {
            boundary = idx;
        } else {
            return idx;
        }
    }
    boundary
}

fn word_right_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .unicode_word_indices()
        .nth(1)
        .map_or(text.len(), |(idx, _)| cursor + idx)
}

fn normalize_pasted_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.split_whitespace().count() == 1
        && (trimmed.starts_with("file://")
            || (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return normalize_pasted_token(trimmed);
    }
    text.to_string()
}

fn normalize_pasted_token(token: &str) -> String {
    let unquoted = token
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            token
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(token);
    let without_file_scheme = unquoted.strip_prefix("file://").unwrap_or(unquoted);
    without_file_scheme.replace("%20", " ")
}

#[derive(Debug)]
struct ConversationScroll {
    offset: u16,
    pinned_to_bottom: bool,
    content_changed: bool,
}

impl Default for ConversationScroll {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationScroll {
    const fn new() -> Self {
        Self {
            offset: 0,
            pinned_to_bottom: true,
            content_changed: false,
        }
    }

    fn scroll_up(&mut self, amount: u16) {
        self.offset = self.offset.saturating_sub(amount);
        self.pinned_to_bottom = false;
    }

    fn scroll_down(&mut self, amount: u16) {
        self.offset = self.offset.saturating_add(amount);
        self.pinned_to_bottom = false;
    }

    const fn scroll_to_top(&mut self) {
        self.offset = 0;
        self.pinned_to_bottom = false;
    }

    const fn scroll_to_bottom(&mut self) {
        self.offset = u16::MAX;
        self.pinned_to_bottom = true;
    }

    const fn mark_content_changed(&mut self) {
        self.content_changed = true;
    }

    fn resolve(&mut self, total_lines: usize, viewport_height: u16) -> u16 {
        let visible = usize::from(viewport_height);
        let max_offset = u16::try_from(total_lines.saturating_sub(visible)).unwrap_or(u16::MAX);
        if self.content_changed && self.pinned_to_bottom {
            self.offset = max_offset;
        }
        self.content_changed = false;
        self.offset = self.offset.min(max_offset);
        self.pinned_to_bottom = self.offset == max_offset;
        self.offset
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

    terminal.draw(|frame| render(frame, &mut app))?;
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
            terminal.draw(|frame| render(frame, &mut app))?;
            match on_submit(input).await {
                Ok(action) => app.apply_action(action),
                Err(err) => app.push_line(ChatLine::status(format!("Error: {err}"))),
            }
            app.busy = false;
        }
        frame_requester.schedule_frame();

        terminal.draw(|frame| render(frame, &mut app))?;
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &mut ChatApp) {
    let area = frame.area();
    let input_height = editor_height(&app.editor);
    let [body, input, footer] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);

    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default().fg(Color::Cyan);

    let body_lines = conversation_lines(&app.lines);
    let conversation_height = body.height.saturating_sub(1);
    let scroll_offset = app.scroll.resolve(body_lines.len(), conversation_height);
    frame.render_widget(
        Paragraph::new(body_lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(dim)
                    .border_type(BorderType::Plain),
            )
            .scroll((scroll_offset, 0))
            .wrap(Wrap { trim: false }),
        body,
    );

    frame.render_widget(
        Paragraph::new(app.editor.text()).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(dim)
                .title(Span::styled(" 输入 ", accent)),
        ),
        input,
    );
    let (cursor_row, cursor_column) = app.editor.cursor_position();
    let input_cursor_x = input
        .x
        .saturating_add(u16::try_from(cursor_column).unwrap_or(u16::MAX))
        .min(input.right().saturating_sub(1));
    let input_cursor_y = input
        .y
        .saturating_add(1)
        .saturating_add(u16::try_from(cursor_row).unwrap_or(u16::MAX))
        .min(input.bottom().saturating_sub(1));
    frame.set_cursor_position(Position::new(input_cursor_x, input_cursor_y));

    let status = if app.busy {
        "运行中"
    } else {
        app.options.status.as_str()
    };
    frame.render_widget(Paragraph::new(footer_line(app, status)).style(dim), footer);

    if let Some(picker) = &app.picker {
        render_picker(frame, app, picker, body);
    } else if app.editor.text().trim_start().starts_with('/') {
        render_slash_completion(frame, app, input);
    }
}

fn editor_height(editor: &EditorState) -> u16 {
    let rows = editor.text().bytes().filter(|byte| *byte == b'\n').count() + 1;
    u16::try_from(rows.saturating_add(2))
        .unwrap_or(8)
        .clamp(3, 8)
}

fn conversation_lines(lines: &[ChatLine]) -> Vec<Line<'_>> {
    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default().fg(Color::Cyan);
    lines
        .iter()
        .map(|line| {
            Line::from(vec![
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
            ])
        })
        .collect()
}

fn footer_line(app: &ChatApp, status: &str) -> String {
    if app.editor.text().trim_start().starts_with('/') {
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

#[cfg(test)]
mod tests {
    use super::{ConversationScroll, EditorState, normalize_pasted_text};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use unicode_width::UnicodeWidthStr;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn conversation_scroll_pins_to_bottom_on_new_output() {
        let mut scroll = ConversationScroll::default();

        scroll.mark_content_changed();
        let offset = scroll.resolve(40, 10);

        assert_eq!(offset, 30);
        assert!(scroll.pinned_to_bottom);
    }

    #[test]
    fn conversation_scroll_preserves_position_after_user_scrolls_up() {
        let mut scroll = ConversationScroll::default();
        scroll.mark_content_changed();
        assert_eq!(scroll.resolve(40, 10), 30);

        scroll.scroll_up(5);
        assert_eq!(scroll.resolve(40, 10), 25);
        scroll.mark_content_changed();
        assert_eq!(scroll.resolve(41, 10), 25);

        assert!(!scroll.pinned_to_bottom);
    }

    #[test]
    fn conversation_scroll_home_and_end_jump_to_edges() {
        let mut scroll = ConversationScroll::default();
        scroll.mark_content_changed();
        assert_eq!(scroll.resolve(40, 10), 30);

        scroll.scroll_to_top();
        assert_eq!(scroll.resolve(40, 10), 0);
        assert!(!scroll.pinned_to_bottom);

        scroll.scroll_to_bottom();
        assert_eq!(scroll.resolve(40, 10), 30);
        assert!(scroll.pinned_to_bottom);
    }

    #[test]
    fn editor_inserts_and_deletes_at_cursor() {
        let mut editor = EditorState::new();
        editor.insert_str("ac");
        editor.move_left();
        editor.insert_char('b');
        assert_eq!(editor.text(), "abc");

        editor.delete_backward();
        assert_eq!(editor.text(), "ac");
        editor.delete_forward();
        assert_eq!(editor.text(), "a");
    }

    #[test]
    fn editor_supports_multiline_vertical_cursor_movement() {
        let mut editor = EditorState::new();
        editor.insert_str("abcd\nef\nghij");
        editor.move_to_line_start();
        assert_eq!(editor.cursor_position(), (2, 0));

        editor.move_up();
        assert_eq!(editor.cursor_position(), (1, 0));
        editor.move_down();
        assert_eq!(editor.cursor_position(), (2, 0));
    }

    #[test]
    fn editor_word_movement_and_deletion_use_word_boundaries() {
        let mut editor = EditorState::new();
        editor.insert_str("alpha beta gamma");

        editor.move_word_left();
        assert_eq!(editor.cursor_position(), (0, "alpha beta ".width()));
        editor.delete_word_backward();
        assert_eq!(editor.text(), "alpha gamma");

        editor.move_to_line_start();
        editor.delete_word_forward();
        assert_eq!(editor.text(), "gamma");
    }

    #[test]
    fn editor_tracks_newline_inserted_at_cursor() {
        let mut editor = EditorState::new();
        editor.insert_str("ab");
        editor.move_left();
        editor.insert_char('\n');

        assert_eq!(editor.text(), "a\nb");
        assert_eq!(editor.cursor_position(), (1, 0));
    }

    #[test]
    fn pasted_text_preserves_multiline_content() {
        assert_eq!(normalize_pasted_text("a\nb c"), "a\nb c");
    }

    #[test]
    fn pasted_drag_drop_path_normalizes_file_url_and_quotes() {
        assert_eq!(
            normalize_pasted_text("\"file:///tmp/a%20b.txt\""),
            "/tmp/a b.txt"
        );
    }

    #[test]
    fn chat_app_maps_crossterm_keys_to_editor_actions() {
        let mut app = super::ChatApp::new(super::ChatOptions::new("model"));

        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('c')));
        app.handle_key(key(KeyCode::Left));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(modified_key(KeyCode::Char('e'), KeyModifiers::CONTROL));
        app.handle_key(modified_key(KeyCode::Enter, KeyModifiers::SHIFT));
        app.handle_key(key(KeyCode::Char('d')));

        assert_eq!(app.editor.text(), "abc\nd");
        assert_eq!(app.editor.cursor_position(), (1, 1));
    }
}
