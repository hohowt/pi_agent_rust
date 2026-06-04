//! Ratatui interactive mode entry points and shared command parsing helpers.

use anyhow::Result;
use pi_core::model::{ContentBlock, TextContent};

use crate::agent::{AgentEvent, AgentSession};
use crate::config::{Config, Language};
use crate::models::ModelEntry;
use crate::package_manager::ResolvedResource;
use crate::resources::{ResourceCliOptions, ResourceLoader};
use crate::runtime::RuntimeHandle;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum PendingInput {
    Text(String),
    Content(Vec<ContentBlock>),
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Login,
    Logout,
    Clear,
    Model,
    Thinking,
    ScopedModels,
    Exit,
    History,
    Export,
    Session,
    Settings,
    Theme,
    Resume,
    New,
    Copy,
    Name,
    Hotkeys,
    Changelog,
    Tree,
    Fork,
    Compact,
    Reload,
    Template,
    Language,
    Share,
    Codegraph,
}

impl SlashCommand {
    #[must_use]
    pub fn parse(input: &str) -> Option<(Self, &str)> {
        let trimmed = input.trim();
        let rest = trimmed.strip_prefix('/')?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let command = parts.next()?.to_ascii_lowercase();
        if command.is_empty() {
            return None;
        }
        let args = parts.next().unwrap_or_default().trim();
        let command = match command.as_str() {
            "help" | "h" | "?" => Self::Help,
            "login" => Self::Login,
            "logout" => Self::Logout,
            "clear" | "cls" => Self::Clear,
            "model" | "m" => Self::Model,
            "thinking" | "think" | "t" => Self::Thinking,
            "scoped-models" | "scoped" => Self::ScopedModels,
            "exit" | "quit" | "q" => Self::Exit,
            "history" | "hist" => Self::History,
            "export" => Self::Export,
            "session" | "info" => Self::Session,
            "settings" => Self::Settings,
            "theme" => Self::Theme,
            "resume" | "r" => Self::Resume,
            "new" => Self::New,
            "copy" | "cp" => Self::Copy,
            "name" => Self::Name,
            "hotkeys" | "keys" | "keybindings" => Self::Hotkeys,
            "changelog" => Self::Changelog,
            "tree" => Self::Tree,
            "fork" => Self::Fork,
            "compact" => Self::Compact,
            "reload" => Self::Reload,
            "template" => Self::Template,
            "language" => Self::Language,
            "share" => Self::Share,
            "codegraph" => Self::Codegraph,
            _ => return None,
        };
        Some((command, args))
    }

    #[must_use]
    pub fn help_text(language: Language) -> &'static str {
        match language {
            Language::Zh => {
                "/help 帮助\n/model 切换模型\n/thinking 设置思考强度\n/session 会话信息\n/tree 对话树\n/exit 退出"
            }
            Language::En => {
                "/help help\n/model switch model\n/thinking set thinking level\n/session session info\n/tree conversation tree\n/exit quit"
            }
        }
    }
}

#[must_use]
pub fn strip_thinking_level_suffix(pattern: &str) -> &str {
    let Some((model, suffix)) = pattern.rsplit_once(':') else {
        return pattern;
    };
    match suffix {
        "off" | "low" | "medium" | "high" | "auto" => model,
        _ => pattern,
    }
}

#[must_use]
pub fn parse_scoped_model_patterns(input: &str) -> Vec<String> {
    input
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

#[must_use]
pub fn model_entry_matches(entry: &ModelEntry, pattern: &str) -> bool {
    let pattern = strip_thinking_level_suffix(pattern).to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    let provider = entry.model.provider.to_ascii_lowercase();
    let id = entry.model.id.to_ascii_lowercase();
    let name = entry.model.name.to_ascii_lowercase();
    let full = format!("{provider}/{id}");
    if pattern.contains('*') {
        return wildcard_match(&pattern, &id)
            || wildcard_match(&pattern, &name)
            || wildcard_match(&pattern, &full);
    }
    id.eq(&pattern) || name.eq(&pattern) || full.eq(&pattern)
}

#[must_use]
pub fn resolve_scoped_model_entries(
    available_models: &[ModelEntry],
    patterns: &[String],
) -> Vec<ModelEntry> {
    let mut resolved = Vec::new();
    for pattern in patterns {
        for entry in available_models {
            if model_entry_matches(entry, pattern)
                && !resolved
                    .iter()
                    .any(|existing: &ModelEntry| existing.model.id == entry.model.id)
            {
                resolved.push(entry.clone());
            }
        }
    }
    resolved
}

#[must_use]
pub fn model_entry_resource_origin(_entry: &ModelEntry) -> Option<&ResolvedResource> {
    None
}

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    mut agent: AgentSession,
    _session: std::sync::Arc<crate::sync::Mutex<crate::session::Session>>,
    _config: Config,
    model_entry: ModelEntry,
    _model_scope: Vec<ModelEntry>,
    _available_models: Vec<ModelEntry>,
    pending_inputs: Vec<PendingInput>,
    _save_enabled: bool,
    _resources: ResourceLoader,
    _resource_cli: ResourceCliOptions,
    _cwd: PathBuf,
    _runtime_handle: RuntimeHandle,
) -> Result<()> {
    let model_label = format!("{}/{}", model_entry.model.provider, model_entry.model.id);
    let mut initial_lines = Vec::new();
    for pending in pending_inputs {
        let input = match pending {
            PendingInput::Text(text) => text,
            PendingInput::Content(content) => content_blocks_to_text(&content),
            PendingInput::Continue => "continue".to_string(),
        };
        initial_lines.push(pi_tui::ChatLine::user(input.clone()));
        let assistant = agent
            .run_with_content(
                vec![ContentBlock::Text(TextContent::new(input))],
                |_event: AgentEvent| {},
            )
            .await?;
        let answer = assistant
            .content
            .iter()
            .filter_map(|block| match block {
                crate::model::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        initial_lines.push(pi_tui::ChatLine::assistant(answer));
    }

    pi_tui::run_minimal_chat_loop(model_label, initial_lines).await
}

fn content_blocks_to_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    let mut remainder = text;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        let Some(stripped) = remainder.strip_prefix(first) else {
            return false;
        };
        remainder = stripped;
    }
    for part in parts
        .iter()
        .skip(usize::from(!pattern.starts_with('*')))
        .filter(|part| !part.is_empty())
    {
        let Some(index) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[index + part.len()..];
    }
    pattern.ends_with('*') || parts.last().is_none_or(|last| remainder.ends_with(last))
}
