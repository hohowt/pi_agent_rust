use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pi_theme::Theme;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use tokio::sync::{broadcast, mpsc};
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
    pub theme: Theme,
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
            theme: Theme::dark(),
            resource_summary: "资源: 0 技能, 0 提示, 0 主题".to_string(),
            command_hints: Vec::new(),
            key_hints: vec![
                "Enter: 发送".to_string(),
                "Ctrl+J: newline".to_string(),
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
    ReplaceLines(Vec<ChatLine>),
    Clear,
    Quit,
    SetOptions(Box<ChatOptions>),
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
    history: InputHistory,
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
            history: InputHistory::new(),
        }
    }

    fn take_submitted_input(&mut self) -> Option<String> {
        let input = self.editor.text().trim().to_string();
        if input.is_empty() {
            return None;
        }
        self.editor.clear();
        self.history.record(input.clone());
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
            ChatAction::ReplaceLines(lines) => {
                self.lines = lines;
                self.scroll.scroll_to_bottom();
                self.scroll.mark_content_changed();
            }
            ChatAction::Clear => {
                self.lines.clear();
                self.scroll.scroll_to_bottom();
            }
            ChatAction::Quit => self.should_quit = true,
            ChatAction::SetOptions(options) => self.options = *options,
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
                    match ch.to_ascii_lowercase() {
                        'a' => self.editor.move_to_line_start(),
                        'e' => self.editor.move_to_line_end(),
                        'j' => self.editor.insert_char('\n'),
                        'u' => self.editor.delete_to_line_start(),
                        'w' => self.editor.delete_word_backward(),
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
            KeyCode::Up => {
                self.handle_up();
            }
            KeyCode::Down => {
                self.handle_down();
            }
            KeyCode::PageUp => self.scroll.scroll_up(10),
            KeyCode::PageDown => self.scroll.scroll_down(10),
            KeyCode::Home => self.scroll.scroll_to_top(),
            KeyCode::End => self.scroll.scroll_to_bottom(),
            _ => {}
        }
        EventOutcome::None
    }

    fn handle_up(&mut self) {
        if self.editor.is_first_line() {
            if let Some(value) = self.history.previous(self.editor.text()) {
                self.editor.set_text(value);
            }
        } else {
            self.editor.move_up();
        }
    }

    fn handle_down(&mut self) {
        if self.editor.is_last_line() {
            if let Some(value) = self.history.next() {
                self.editor.set_text(value);
            }
        } else {
            self.editor.move_down();
        }
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
                    self.editor.clear();
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

    fn is_first_line(&self) -> bool {
        self.line_start_before(self.cursor) == 0
    }

    fn is_last_line(&self) -> bool {
        self.line_end_from(self.cursor) == self.text.len()
    }

    fn prev_boundary(&self, at: usize) -> Option<usize> {
        if at == 0 {
            return None;
        }
        self.text[..at]
            .grapheme_indices(true)
            .next_back()
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

#[derive(Debug)]
struct InputHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl InputHistory {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: None,
            draft: String::new(),
        }
    }

    fn record(&mut self, input: String) {
        if self.entries.last() != Some(&input) {
            self.entries.push(input);
        }
        self.cursor = None;
        self.draft.clear();
    }

    fn previous(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let next_cursor = if let Some(cursor) = self.cursor {
            cursor.saturating_sub(1)
        } else {
            self.draft = current.to_string();
            self.entries.len().saturating_sub(1)
        };
        self.cursor = Some(next_cursor);
        self.entries.get(next_cursor).cloned()
    }

    fn next(&mut self) -> Option<String> {
        let cursor = self.cursor?;
        if cursor + 1 >= self.entries.len() {
            self.cursor = None;
            return Some(std::mem::take(&mut self.draft));
        }
        let next_cursor = cursor + 1;
        self.cursor = Some(next_cursor);
        self.entries.get(next_cursor).cloned()
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

    const fn scroll_up(&mut self, amount: u16) {
        self.offset = self.offset.saturating_sub(amount);
        self.pinned_to_bottom = false;
    }

    const fn scroll_down(&mut self, amount: u16) {
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

pub type ChatActionSender = mpsc::UnboundedSender<ChatAction>;
pub type SubmitFuture = Pin<Box<dyn Future<Output = anyhow::Result<ChatAction>> + Send>>;

pub async fn run_minimal_chat_loop(
    options: ChatOptions,
    initial_lines: Vec<ChatLine>,
    mut on_submit: impl FnMut(String, ChatActionSender) -> SubmitFuture + Send,
) -> anyhow::Result<()> {
    let _terminal_guard = TerminalModeGuard::enter()?;
    let _alternate_scroll = AlternateScrollGuard::enable()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (draw_tx, draw_rx) = broadcast::channel(32);
    let frame_requester = FrameRequester::new(draw_tx);
    let mut events = RatatuiEventStream::new(draw_rx);
    let (action_tx, mut action_rx) = mpsc::unbounded_channel();
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

        let event = tokio::select! {
            action = action_rx.recv() => {
                if let Some(action) = action {
                    app.apply_action(action);
                    frame_requester.schedule_frame();
                    terminal.draw(|frame| render(frame, &mut app))?;
                    continue;
                }
                break;
            }
            event = events.next() => {
                let Some(event) = event else {
                    break;
                };
                event
            }
        };

        let outcome = app.handle_event(event);
        if let EventOutcome::Submit(input) = outcome {
            app.busy = true;
            frame_requester.schedule_frame();
            terminal.draw(|frame| render(frame, &mut app))?;
            match on_submit(input, action_tx.clone()).await {
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
    let palette = ChatPalette::from_theme(&app.options.theme);
    let dim = Style::default().fg(palette.muted);
    let accent = Style::default().fg(palette.accent);

    let body_lines = conversation_lines(&app.lines, &palette);
    let layout = chat_layout(area, body_lines.len(), editor_height(&app.editor));
    frame.render_widget(Clear, area);

    let body = layout.body;
    let input = layout.input;
    let footer = layout.footer;
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
    frame.render_widget(
        Paragraph::new(footer_line(app, status, footer.width, &palette)).style(dim),
        footer,
    );

    if let Some(picker) = &app.picker {
        render_picker(frame, app, picker, area, &palette);
    } else if app.editor.text().trim_start().starts_with('/') {
        render_slash_completion(frame, app, input, &palette);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChatLayout {
    body: Rect,
    input: Rect,
    footer: Rect,
}

fn chat_layout(
    area: Rect,
    conversation_line_count: usize,
    desired_input_height: u16,
) -> ChatLayout {
    let footer_height = u16::from(area.height > 1);
    let input_height = desired_input_height
        .max(1)
        .min(area.height.saturating_sub(footer_height));
    let max_body_height = area
        .height
        .saturating_sub(input_height)
        .saturating_sub(footer_height);
    let desired_body_height = u16::try_from(conversation_line_count.saturating_add(1))
        .unwrap_or(u16::MAX)
        .clamp(u16::from(max_body_height > 0), max_body_height);
    let used_height = desired_body_height
        .saturating_add(input_height)
        .saturating_add(footer_height)
        .min(area.height);
    let used = Rect {
        height: used_height,
        ..area
    };
    let [body, input, footer] = Layout::vertical([
        Constraint::Length(desired_body_height),
        Constraint::Length(input_height),
        Constraint::Length(footer_height),
    ])
    .areas(used);
    ChatLayout {
        body,
        input,
        footer,
    }
}

fn editor_height(editor: &EditorState) -> u16 {
    let rows = editor.text().bytes().filter(|byte| *byte == b'\n').count() + 1;
    u16::try_from(rows.saturating_add(2))
        .unwrap_or(8)
        .clamp(3, 8)
}

#[derive(Debug, Clone, Copy)]
struct ChatPalette {
    foreground: Color,
    muted: Color,
    accent: Color,
    success: Color,
    border: Color,
    selection: Color,
}

impl ChatPalette {
    fn from_theme(theme: &Theme) -> Self {
        let fallback = Self::default();
        Self {
            foreground: color_from_hex(&theme.colors.foreground).unwrap_or(fallback.foreground),
            muted: color_from_hex(&theme.colors.muted).unwrap_or(fallback.muted),
            accent: color_from_hex(&theme.colors.accent).unwrap_or(fallback.accent),
            success: color_from_hex(&theme.colors.success).unwrap_or(fallback.success),
            border: color_from_hex(&theme.ui.border).unwrap_or(fallback.border),
            selection: color_from_hex(&theme.ui.selection).unwrap_or(fallback.selection),
        }
    }
}

impl Default for ChatPalette {
    fn default() -> Self {
        Self {
            foreground: Color::Reset,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            success: Color::Green,
            border: Color::DarkGray,
            selection: Color::DarkGray,
        }
    }
}

fn color_from_hex(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn conversation_lines(lines: &[ChatLine], palette: &ChatPalette) -> Vec<Line<'static>> {
    let dim = Style::default().fg(palette.muted);
    let accent = Style::default().fg(palette.accent);
    let mut rendered = Vec::new();
    for line in lines {
        let role_style = match line.role {
            "Assistant" => Style::default().fg(palette.success),
            "Status" => dim,
            _ => accent,
        }
        .add_modifier(Modifier::BOLD);
        let body = if line.role == "Assistant" {
            markdown_lines(&line.text, palette)
        } else {
            plain_text_lines(&line.text)
        };
        append_role_lines(&mut rendered, line.role, role_style, body);
    }
    rendered
}

fn plain_text_lines(text: &str) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::from("")];
    }
    text.split('\n')
        .map(|line| Line::from(line.to_string()))
        .collect()
}

fn append_role_lines(
    output: &mut Vec<Line<'static>>,
    role: &str,
    role_style: Style,
    mut body: Vec<Line<'static>>,
) {
    let prefix = format!("{role}: ");
    let continuation = " ".repeat(prefix.width());
    if body.is_empty() {
        body.push(Line::from(""));
    }
    for (idx, mut line) in body.into_iter().enumerate() {
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        if idx == 0 {
            spans.push(Span::styled(prefix.clone(), role_style));
        } else {
            spans.push(Span::raw(continuation.clone()));
        }
        spans.append(&mut line.spans);
        output.push(Line::from(spans));
    }
}

#[derive(Debug, Clone, Copy)]
struct MarkdownRenderState {
    style: Style,
    quote_depth: usize,
    in_code_block: bool,
}

struct MarkdownRenderer<'a> {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    state: MarkdownRenderState,
    style_stack: Vec<Style>,
    list_stack: Vec<Option<u64>>,
    link_stack: Vec<String>,
    palette: &'a ChatPalette,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(palette: &'a ChatPalette) -> Self {
        Self {
            lines: Vec::new(),
            spans: Vec::new(),
            state: MarkdownRenderState {
                style: Style::default().fg(palette.foreground),
                quote_depth: 0,
                in_code_block: false,
            },
            style_stack: vec![Style::default().fg(palette.foreground)],
            list_stack: Vec::new(),
            link_stack: Vec::new(),
            palette,
        }
    }

    fn into_lines(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        if self.lines.is_empty() {
            self.lines.push(Line::from(""));
        }
        self.lines
    }

    fn handle_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_line();
                let marker = heading_marker(level);
                self.spans.push(Span::styled(
                    marker,
                    Style::default()
                        .fg(self.palette.accent)
                        .add_modifier(Modifier::BOLD),
                ));
                self.push_style(
                    Style::default()
                        .fg(self.palette.accent)
                        .add_modifier(Modifier::BOLD),
                );
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.state.quote_depth = self.state.quote_depth.saturating_add(1);
            }
            Tag::CodeBlock(kind) => {
                self.flush_line();
                self.state.in_code_block = true;
                if let CodeBlockKind::Fenced(info) = kind
                    && !info.is_empty()
                {
                    self.lines.push(Line::from(vec![Span::styled(
                        format!("```{info}"),
                        Style::default().fg(self.palette.muted),
                    )]));
                }
            }
            Tag::List(start) => self.list_stack.push(start),
            Tag::Item => {
                self.flush_line();
                let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
                let marker = if let Some(Some(next)) = self.list_stack.last_mut() {
                    let marker = format!("{indent}{next}. ");
                    *next = next.saturating_add(1);
                    marker
                } else {
                    format!("{indent}- ")
                };
                self.spans.push(Span::styled(
                    marker,
                    Style::default().fg(self.palette.muted),
                ));
            }
            Tag::Emphasis => self.push_style(self.state.style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(self.state.style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(self.state.style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.link_stack.push(dest_url.to_string());
                self.push_style(
                    self.state
                        .style
                        .fg(self.palette.accent)
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            _ => {}
        }
    }

    fn handle_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => {
                self.flush_line();
                if matches!(tag, TagEnd::Heading(_)) {
                    self.pop_style();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.state.quote_depth = self.state.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.state.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.flush_line();
                self.list_stack.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.pop_style();
            }
            TagEnd::Link => {
                if let Some(dest) = self.link_stack.pop()
                    && !dest.is_empty()
                {
                    self.spans.push(Span::styled(
                        format!(" ({dest})"),
                        Style::default().fg(self.palette.muted),
                    ));
                }
                self.pop_style();
            }
            _ => {}
        }
    }

    fn append_text(&mut self, text: &str) {
        if self.state.in_code_block {
            self.append_code_block_lines(text);
        } else {
            append_text_spans(&mut self.spans, text, self.state.style);
        }
    }

    fn append_code_block_lines(&mut self, text: &str) {
        let style = Style::default()
            .fg(self.palette.foreground)
            .bg(self.palette.selection);
        for line in text.split_inclusive('\n') {
            let line = line.strip_suffix('\n').unwrap_or(line);
            self.lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.to_string(), style),
            ]));
        }
    }

    fn flush_line(&mut self) {
        flush_markdown_line(&mut self.lines, &mut self.spans, self.state.quote_depth);
    }

    fn push_style(&mut self, style: Style) {
        self.style_stack.push(style);
        self.state.style = style;
    }

    fn pop_style(&mut self) {
        if self.style_stack.len() > 1 {
            self.style_stack.pop();
        }
        self.state.style = self.style_stack.last().copied().unwrap_or_default();
    }
}

fn markdown_lines(text: &str, palette: &ChatPalette) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let mut renderer = MarkdownRenderer::new(palette);

    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(tag) => renderer.handle_start(tag),
            Event::End(tag) => renderer.handle_end(tag),
            Event::Text(value) => renderer.append_text(value.as_ref()),
            Event::Code(value) => renderer.spans.push(Span::styled(
                format!("`{value}`"),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Event::SoftBreak => renderer.spans.push(Span::raw(" ")),
            Event::HardBreak => renderer.flush_line(),
            Event::Rule => {
                renderer.flush_line();
                renderer.lines.push(Line::from(Span::styled(
                    "─".repeat(16),
                    Style::default().fg(palette.muted),
                )));
            }
            _ => {}
        }
    }
    renderer.into_lines()
}

fn heading_marker(level: HeadingLevel) -> String {
    let level = match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    };
    format!("{} ", "#".repeat(level))
}

fn append_text_spans(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    for (idx, part) in text.split('\n').enumerate() {
        if idx > 0 {
            spans.push(Span::raw(" "));
        }
        if !part.is_empty() {
            spans.push(Span::styled(part.to_string(), style));
        }
    }
}

fn flush_markdown_line(
    lines: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    quote_depth: usize,
) {
    if spans.is_empty() {
        return;
    }
    let mut line_spans = Vec::new();
    if quote_depth > 0 {
        line_spans.push(Span::raw(format!("{} ", ">".repeat(quote_depth))));
    }
    line_spans.append(spans);
    lines.push(Line::from(line_spans));
}

fn footer_line(app: &ChatApp, status: &str, width: u16, palette: &ChatPalette) -> Line<'static> {
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
        return Line::from(vec![
            Span::styled("› ", Style::default().fg(palette.accent)),
            Span::styled("commands", Style::default().fg(palette.accent)),
            Span::raw("  "),
            Span::styled(hints.join("  "), Style::default().fg(palette.muted)),
        ]);
    }
    let left = format!(
        "{}  {}  模型: {}  {}",
        app.options.title, status, app.options.model_label, app.options.resource_summary
    );
    let right = app.options.key_hints.join("  ");
    let gap = usize::from(width)
        .saturating_sub(left.width())
        .saturating_sub(right.width())
        .max(2);
    Line::from(vec![
        Span::styled(left, Style::default().fg(palette.foreground)),
        Span::raw(" ".repeat(gap)),
        Span::styled(right, Style::default().fg(palette.muted)),
    ])
}

fn render_slash_completion(
    frame: &mut ratatui::Frame<'_>,
    app: &ChatApp,
    input_area: Rect,
    palette: &ChatPalette,
) {
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
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(item.command.as_str(), style),
                Span::raw("  "),
                Span::styled(
                    item.description.as_str(),
                    Style::default().fg(palette.muted),
                ),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border))
                .title(" commands "),
        ),
        area,
    );
}

fn render_picker(
    frame: &mut ratatui::Frame<'_>,
    _app: &ChatApp,
    picker: &PickerState,
    body: Rect,
    palette: &ChatPalette,
) {
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
        Span::styled("filter: ", Style::default().fg(palette.muted)),
        Span::raw(picker.query.as_str()),
    ]))
    .chain(items.iter().take(visible).enumerate().map(|(idx, item)| {
        let selected = idx == picker.selected.min(items.len().saturating_sub(1));
        let style = if selected {
            Style::default()
                .fg(palette.foreground)
                .bg(palette.selection)
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
                Style::default().fg(palette.muted),
            ),
        ])
    }))
    .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border))
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
    use super::{
        ChatAction, ChatApp, ChatLine, ChatOptions, ChatPalette, ChatPicker, ConversationScroll,
        EditorState, EventOutcome, PickerItem, chat_layout, conversation_lines, footer_line,
        normalize_pasted_text,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
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
        app.handle_key(modified_key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('d')));

        assert_eq!(app.editor.text(), "abc\nd");
        assert_eq!(app.editor.cursor_position(), (1, 1));
    }

    #[test]
    fn chat_app_ctrl_w_deletes_previous_word() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("alpha beta gamma");

        app.handle_key(modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL));

        assert_eq!(app.editor.text(), "alpha beta ");
        assert_eq!(app.editor.cursor_position(), (0, "alpha beta ".width()));
    }

    #[test]
    fn shift_enter_submits_instead_of_inserting_newline() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("send me");

        let outcome = app.handle_key(modified_key(KeyCode::Enter, KeyModifiers::SHIFT));

        assert_eq!(outcome, EventOutcome::Submit("send me".to_string()));
        assert_eq!(app.editor.text(), "");
    }

    #[test]
    fn up_down_navigate_submitted_input_history() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("first");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            EventOutcome::Submit("first".to_string())
        );
        app.editor.set_text("second");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            EventOutcome::Submit("second".to_string())
        );

        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "second");
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "first");
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "first");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.editor.text(), "second");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.editor.text(), "");
    }

    #[test]
    fn history_navigation_restores_current_draft() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("previous");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            EventOutcome::Submit("previous".to_string())
        );
        app.editor.set_text("draft");

        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "previous");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.editor.text(), "draft");
    }

    #[test]
    fn multiline_up_down_prefer_vertical_cursor_movement_inside_editor() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("history");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            EventOutcome::Submit("history".to_string())
        );
        app.editor.set_text("top\nbottom");
        app.editor.move_to_line_start();

        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "top\nbottom");
        assert_eq!(app.editor.cursor_position(), (0, 0));

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.editor.text(), "top\nbottom");
        assert_eq!(app.editor.cursor_position(), (1, 0));
    }

    #[test]
    fn chat_layout_keeps_composer_below_content_not_screen_bottom() {
        let area = Rect::new(0, 0, 80, 30);

        let layout = chat_layout(area, 2, 3);

        assert_eq!(layout.body, Rect::new(0, 0, 80, 3));
        assert_eq!(layout.input, Rect::new(0, 3, 80, 3));
        assert_eq!(layout.footer, Rect::new(0, 6, 80, 1));
        assert!(layout.footer.bottom() < area.bottom());
    }

    #[test]
    fn chat_layout_uses_full_height_when_content_overflows() {
        let area = Rect::new(0, 0, 80, 12);

        let layout = chat_layout(area, 100, 3);

        assert_eq!(layout.body.height, 8);
        assert_eq!(layout.input.y, 8);
        assert_eq!(layout.footer.y, 11);
    }

    #[test]
    fn assistant_markdown_renders_core_blocks() {
        let palette = ChatPalette::default();
        let lines = conversation_lines(
            &[ChatLine::assistant(
                "# 标题\n- item\n[link](https://example.com)\n```rust\nlet x = 1;\n```",
            )],
            &palette,
        );
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Assistant: # 标题"));
        assert!(rendered.contains("- item"));
        assert!(rendered.contains("link (https://example.com)"));
        assert!(rendered.contains("  let x = 1;"));
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn footer_line_uses_statusline_shape_and_ctrl_j_hint() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.options.key_hints = vec!["Enter: 发送".to_string(), "Ctrl+J: newline".to_string()];

        let line = footer_line(&app, "就绪", 80, &ChatPalette::default());
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("Pi  就绪  模型: model"));
        assert!(rendered.contains("Ctrl+J: newline"));
        assert!(!rendered.contains("Shift+Enter"));
    }

    #[test]
    fn picker_selection_submits_without_leaving_command_in_editor() {
        let mut app = ChatApp::new(ChatOptions::new("model"));
        app.editor.set_text("/model");
        app.apply_action(ChatAction::OpenPicker(ChatPicker::new(
            "模型",
            "/model",
            vec![PickerItem::new(
                "deepseek/deepseek-reasoner",
                "deepseek/deepseek-reasoner",
                "reasoning",
            )],
        )));

        let outcome = app.handle_key(key(KeyCode::Enter));

        assert_eq!(
            outcome,
            EventOutcome::Submit("/model deepseek/deepseek-reasoner".to_string())
        );
        assert_eq!(app.editor.text(), "");
        assert!(app.picker.is_none());
    }
}
