use std::io::{self, IsTerminal, Write};

/// Pi's console wrapper for non-interactive terminal output.
#[derive(Debug, Clone)]
pub struct PiConsole {
    is_tty: bool,
}

impl PiConsole {
    /// Create a new Pi console with auto-detected terminal capabilities.
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_tty: io::stdout().is_terminal(),
        }
    }

    /// Create a new Pi console with an optional theme.
    #[must_use]
    pub fn new_with_theme<T>(_theme: Option<T>) -> Self {
        Self {
            is_tty: io::stdout().is_terminal(),
        }
    }

    /// Create a console with forced color output for tests.
    #[must_use]
    pub const fn with_color() -> Self {
        Self { is_tty: true }
    }

    /// Check if stdout is a terminal.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.is_tty
    }

    /// Get the current terminal width.
    #[must_use]
    pub fn width(&self) -> usize {
        crossterm::terminal::size()
            .map(|(width, _)| usize::from(width))
            .unwrap_or(80)
    }

    /// Print plain text without styling.
    pub fn print_plain(&self, text: &str) {
        print!("{text}");
        let _ = io::stdout().flush();
    }

    /// Print simple rich-style markup. Styling tags are stripped for now.
    pub fn print_markup(&self, markup: &str) {
        print!("{}", strip_markup(markup));
        let _ = io::stdout().flush();
    }

    /// Print a newline.
    pub fn newline(&self) {
        println!();
    }

    /// Render Markdown as readable plain text.
    pub fn render_markdown(&self, markdown: &str) {
        self.render_markdown_with_indent(markdown, None);
    }

    /// Render Markdown with an optional code block indentation override.
    pub fn render_markdown_with_indent(&self, markdown: &str, code_block_indent: Option<usize>) {
        let rendered = render_markdown_plain(markdown, code_block_indent.unwrap_or(0));
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
        let _ = io::stdout().flush();
    }

    /// Render streaming text from the assistant.
    pub fn render_text_delta(&self, text: &str) {
        self.print_plain(text);
    }

    /// Render streaming thinking text.
    pub fn render_thinking_delta(&self, text: &str) {
        if self.is_tty {
            print!("\x1b[2m{text}\x1b[0m");
            let _ = io::stdout().flush();
        } else {
            self.print_plain(text);
        }
    }

    pub fn render_thinking_start(&self) {
        if self.is_tty {
            self.print_plain("\nThinking...\n");
        }
    }

    pub fn render_thinking_end(&self) {
        if self.is_tty {
            self.newline();
        }
    }

    pub fn render_tool_start(&self, name: &str, _input: &str) {
        if self.is_tty {
            println!("\n[Running {name}...]");
        }
    }

    pub fn render_tool_end(&self, name: &str, is_error: bool) {
        if self.is_tty {
            let status = if is_error { "failed" } else { "done" };
            println!("[{name} {status}]\n");
        }
    }

    pub fn render_error(&self, error: &str) {
        eprintln!("\nError: {error}");
    }

    pub fn render_warning(&self, warning: &str) {
        eprintln!("Warning: {warning}");
    }

    pub fn render_success(&self, message: &str) {
        println!("{message}");
    }

    pub fn render_info(&self, message: &str) {
        println!("{message}");
    }

    pub fn render_panel(&self, content: &str, title: &str) {
        println!("--- {title} ---");
        println!("{content}");
        println!("---");
    }

    pub fn render_table(&self, headers: &[&str], rows: &[Vec<&str>]) {
        println!("{}", headers.join("\t"));
        for row in rows {
            println!("{}", row.join("\t"));
        }
    }

    pub fn render_rule(&self, title: Option<&str>) {
        if let Some(title) = title {
            println!("--- {title} ---");
        } else {
            println!("---");
        }
    }

    pub fn render_usage(&self, input_tokens: u32, output_tokens: u32, cost_usd: Option<f64>) {
        let cost = cost_usd
            .map(|value| format!(" (${value:.4})"))
            .unwrap_or_default();
        println!("Tokens: {input_tokens} in / {output_tokens} out{cost}");
    }

    pub fn render_session_info(&self, session_path: &str, message_count: usize) {
        println!("Session: {session_path} ({message_count} messages)");
    }

    pub fn render_model_info(&self, model: &str, thinking_level: Option<&str>) {
        let thinking = thinking_level
            .map(|level| format!(" (thinking: {level})"))
            .unwrap_or_default();
        println!("Model: {model}{thinking}");
    }

    pub fn render_prompt(&self) {
        print!("> ");
        let _ = io::stdout().flush();
    }

    pub fn render_user_message(&self, message: &str) {
        println!("You: {message}\n");
    }

    pub fn render_assistant_start(&self) {
        print!("Assistant: ");
        let _ = io::stdout().flush();
    }

    pub fn clear_line(&self) {
        if self.is_tty {
            print!("\r\x1b[K");
            let _ = io::stdout().flush();
        }
    }

    pub fn cursor_up(&self, n: usize) {
        if self.is_tty && n > 0 {
            print!("\x1b[{n}A");
            let _ = io::stdout().flush();
        }
    }
}

impl Default for PiConsole {
    fn default() -> Self {
        Self::new()
    }
}

/// Spinner styles for different operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    /// Default dots spinner for general operations.
    Dots,
    /// Line spinner for file operations.
    Line,
    /// Simple ASCII spinner for compatibility.
    Simple,
}

impl SpinnerStyle {
    /// Get the spinner frames for this style.
    #[must_use]
    pub const fn frames(&self) -> &'static [&'static str] {
        match self {
            Self::Dots => &[".", "..", "..."],
            Self::Line => &["-", "_"],
            Self::Simple => &["|", "/", "-", "\\"],
        }
    }

    /// Get the frame interval in milliseconds.
    #[must_use]
    pub const fn interval_ms(&self) -> u64 {
        match self {
            Self::Dots => 80,
            Self::Line | Self::Simple => 100,
        }
    }
}

fn render_markdown_plain(markdown: &str, code_block_indent: usize) -> String {
    let mut output = String::new();
    let mut in_code = false;
    let indent = " ".repeat(code_block_indent);
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code && code_block_indent > 0 {
            output.push_str(&indent);
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn strip_markup(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut tag = String::new();
    let mut in_tag = false;

    for ch in text.chars() {
        if in_tag {
            if ch == ']' {
                if tag.is_empty() || tag.chars().all(|item| item.is_ascii_digit()) {
                    result.push('[');
                    result.push_str(&tag);
                    result.push(']');
                }
                tag.clear();
                in_tag = false;
            } else if ch == '[' {
                result.push('[');
                result.push_str(&tag);
                tag.clear();
            } else {
                tag.push(ch);
            }
        } else if ch == '[' {
            in_tag = true;
        } else {
            result.push(ch);
        }
    }

    if in_tag {
        result.push('[');
        result.push_str(&tag);
    }

    result
}
