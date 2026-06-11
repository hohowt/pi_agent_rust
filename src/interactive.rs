//! Ratatui interactive mode entry points and shared command parsing helpers.

use anyhow::Result;
use pi_core::model::{ContentBlock, TextContent};
use pi_prompt::PromptCatalog;

use crate::agent::{AgentEvent, AgentSession};
use crate::auth::AuthStorage;
use crate::config::{Config, Language, SettingsScope};
use crate::model::{AssistantMessageEvent, Message, ThinkingLevel, UserContent};
use crate::models::{ModelEntry, ModelRegistry};
use crate::package_manager::PackageManager;
use crate::package_manager::ResolvedResource;
use crate::resources::{ResourceCliOptions, ResourceLoader, parse_command_args, substitute_args};
use crate::runtime::RuntimeHandle;
use crate::session::SessionEntry;
use crate::tools::process_file_arguments;
use pi_theme::Theme;
use serde_json::json;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::process::Command;

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
        PromptCatalog::new(language).ui_text().slash_help()
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

#[derive(Debug)]
struct InteractiveContext {
    config: Config,
    current_model: ModelEntry,
    available_models: Vec<ModelEntry>,
    resources: ResourceLoader,
    resource_cli: ResourceCliOptions,
    package_manager: PackageManager,
    auth_path: PathBuf,
    models_path: PathBuf,
    cwd: PathBuf,
    options: pi_tui::ChatOptions,
}

struct InteractiveContextInit {
    config: Config,
    current_model: ModelEntry,
    available_models: Vec<ModelEntry>,
    resources: ResourceLoader,
    resource_cli: ResourceCliOptions,
    auth_path: PathBuf,
    models_path: PathBuf,
    cwd: PathBuf,
}

impl InteractiveContext {
    fn new(init: InteractiveContextInit) -> Self {
        let mut options = pi_tui::ChatOptions::new(model_label(&init.current_model));
        options.status = "就绪".to_string();
        options.theme = Theme::resolve(&init.config, &init.cwd);
        options.resource_summary = resource_summary(&init.resources);
        options.mouse_capture = init.config.disable_mouse_capture == Some(false);
        options.command_hints = vec![
            "/help".to_string(),
            "/model".to_string(),
            "/thinking".to_string(),
            "/session".to_string(),
            "/settings".to_string(),
            "/codegraph status".to_string(),
            "/exit".to_string(),
        ];
        options.key_hints = vec![
            "Enter: 发送".to_string(),
            "Ctrl+L: 模型".to_string(),
            "Ctrl+O: 工具".to_string(),
            "Shift+Tab: 思考".to_string(),
            "Ctrl+J: newline".to_string(),
            "Ctrl+C: quit".to_string(),
            "Esc Esc: 会话".to_string(),
            "/help".to_string(),
        ];
        options.slash_commands = slash_command_items(init.config.language());
        Self {
            config: init.config,
            current_model: init.current_model,
            available_models: init.available_models,
            resources: init.resources,
            resource_cli: init.resource_cli,
            package_manager: PackageManager::new(init.cwd.clone()),
            auth_path: init.auth_path,
            models_path: init.models_path,
            cwd: init.cwd,
            options,
        }
    }

    fn language(&self) -> Language {
        self.config.language()
    }

    fn sync_footer_model(&mut self) -> pi_tui::ChatAction {
        self.options.model_label = model_label(&self.current_model);
        self.options.resource_summary = resource_summary(&self.resources);
        pi_tui::ChatAction::SetOptions(Box::new(self.options.clone()))
    }

    fn sync_theme_options(&mut self, theme: Theme) -> pi_tui::ChatAction {
        self.options.theme = theme;
        pi_tui::ChatAction::SetOptions(Box::new(self.options.clone()))
    }

    fn sync_resource_options(&mut self) -> pi_tui::ChatAction {
        self.options.model_label = model_label(&self.current_model);
        self.options.resource_summary = resource_summary(&self.resources);
        self.options.slash_commands = slash_command_items(self.config.language());
        pi_tui::ChatAction::SetOptions(Box::new(self.options.clone()))
    }
}

fn slash_command_items(language: Language) -> Vec<pi_tui::SlashCommandItem> {
    let rows = match language {
        Language::Zh => vec![
            ("/help", "帮助"),
            ("/model", "选择/切换模型"),
            ("/thinking", "设置 thinking level"),
            ("/session", "显示会话信息"),
            ("/settings", "显示设置"),
            ("/theme", "选择主题"),
            ("/resume", "选择历史会话"),
            ("/history", "打开会话列表"),
            ("/export", "导出会话"),
            ("/copy", "复制上一条 assistant 消息"),
            ("/share", "分享会话"),
            ("/tree", "显示对话树信息"),
            ("/compact", "压缩上下文"),
            ("/clear", "清屏"),
            ("/reload", "重新加载资源"),
            ("/template", "选择 prompt template"),
            ("/language", "切换语言"),
            ("/changelog", "显示 changelog"),
            ("/codegraph", "管理 codegraph 索引"),
            ("/exit", "退出"),
        ],
        Language::En => vec![
            ("/help", "Show help"),
            ("/model", "Pick or switch model"),
            ("/thinking", "Set thinking level"),
            ("/session", "Show session info"),
            ("/settings", "Show settings"),
            ("/theme", "Pick theme"),
            ("/resume", "Pick previous session"),
            ("/history", "Open session list"),
            ("/export", "Export session"),
            ("/copy", "Copy last assistant message"),
            ("/share", "Share session"),
            ("/tree", "Show conversation tree info"),
            ("/compact", "Compact context"),
            ("/clear", "Clear screen"),
            ("/reload", "Reload resources"),
            ("/template", "Pick prompt template"),
            ("/language", "Switch language"),
            ("/changelog", "Show changelog"),
            ("/codegraph", "Manage codegraph index"),
            ("/exit", "Quit"),
        ],
    };
    rows.into_iter()
        .map(|(command, description)| pi_tui::SlashCommandItem::new(command, description))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    mut agent: AgentSession,
    _session: std::sync::Arc<crate::sync::Mutex<crate::session::Session>>,
    config: Config,
    model_entry: ModelEntry,
    _model_scope: Vec<ModelEntry>,
    available_models: Vec<ModelEntry>,
    pending_inputs: Vec<PendingInput>,
    _save_enabled: bool,
    resources: ResourceLoader,
    resource_cli: ResourceCliOptions,
    auth_path: PathBuf,
    models_path: PathBuf,
    cwd: PathBuf,
    _runtime_handle: RuntimeHandle,
) -> Result<()> {
    let context = InteractiveContext::new(InteractiveContextInit {
        config,
        current_model: model_entry,
        available_models,
        resources,
        resource_cli,
        auth_path,
        models_path,
        cwd,
    });
    let options = context.options.clone();
    let mut initial_lines = Vec::new();
    initial_lines.extend(startup_changelog_lines(&context.config));
    for pending in pending_inputs {
        let content = match pending {
            PendingInput::Text(text) => expand_submitted_content_for_tui(
                &text,
                &context.cwd,
                context.config.image_auto_resize(),
            )?,
            PendingInput::Content(content) => content,
            PendingInput::Continue => vec![ContentBlock::Text(TextContent::new("continue"))],
        };
        let input = content_blocks_to_text(&content);
        initial_lines.push(pi_tui::ChatLine::user(input.clone()));
        let assistant = agent
            .run_with_content(content, |_event: AgentEvent| {})
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

    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    let context = Arc::new(tokio::sync::Mutex::new(context));

    pi_tui::run_minimal_chat_loop(options, initial_lines, move |input, action_tx| {
        let agent = Arc::clone(&agent);
        let context = Arc::clone(&context);
        Box::pin(async move {
            let mut agent = agent.lock().await;
            let mut context = context.lock().await;
            handle_submitted_input(&mut agent, &mut context, input, action_tx).await
        })
    })
    .await
}

async fn handle_submitted_input(
    agent: &mut AgentSession,
    context: &mut InteractiveContext,
    input: String,
    action_tx: pi_tui::ChatActionSender,
) -> Result<pi_tui::ChatAction> {
    if let Some((command, args)) = SlashCommand::parse(&input) {
        return handle_slash_command(agent, context, command, args, action_tx).await;
    }

    run_user_prompt(agent, context, input, action_tx, false).await
}

async fn run_user_prompt(
    agent: &mut AgentSession,
    context: &InteractiveContext,
    input: String,
    action_tx: pi_tui::ChatActionSender,
    echo_user: bool,
) -> Result<pi_tui::ChatAction> {
    let event_sink = action_tx.clone();
    let streamed_assistant_text = Arc::new(AtomicBool::new(false));
    let streamed_assistant_text_for_events = Arc::clone(&streamed_assistant_text);
    let content =
        expand_submitted_content_for_tui(&input, &context.cwd, context.config.image_auto_resize())?;
    let user_line = content_blocks_to_text(&content);
    let assistant = agent
        .run_with_content(content, move |event| {
            if let Some(delta) = assistant_text_delta(&event) {
                streamed_assistant_text_for_events.store(true, Ordering::SeqCst);
                let _ = event_sink.send(pi_tui::ChatAction::AppendAssistantText(delta.to_string()));
                return;
            }
            if let Some(delta) = assistant_thinking_delta(&event) {
                let _ = event_sink.send(pi_tui::ChatAction::AppendThinkingText(delta.to_string()));
                return;
            }
            if let Some(line) = format_agent_event(&event) {
                let _ =
                    event_sink.send(pi_tui::ChatAction::PushLine(pi_tui::ChatLine::status(line)));
            }
        })
        .await?;
    if streamed_assistant_text.load(Ordering::SeqCst) {
        return if echo_user {
            Ok(pi_tui::ChatAction::PushLine(pi_tui::ChatLine::user(
                user_line,
            )))
        } else {
            Ok(pi_tui::ChatAction::Many(Vec::new()))
        };
    }
    let answer = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            crate::model::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let assistant_line = pi_tui::ChatAction::PushLine(pi_tui::ChatLine::assistant(answer));
    if echo_user {
        Ok(pi_tui::ChatAction::Many(vec![
            pi_tui::ChatAction::PushLine(pi_tui::ChatLine::user(user_line)),
            assistant_line,
        ]))
    } else {
        Ok(assistant_line)
    }
}

fn assistant_text_delta(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
            ..
        } if !delta.is_empty() => Some(delta.as_str()),
        _ => None,
    }
}

fn assistant_thinking_delta(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::ThinkingDelta { delta, .. },
            ..
        } if !delta.is_empty() => Some(delta.as_str()),
        _ => None,
    }
}

#[doc(hidden)]
pub fn expand_submitted_content_for_tui(
    input: &str,
    cwd: &Path,
    auto_resize_images: bool,
) -> Result<Vec<ContentBlock>> {
    let (message_text, file_args) = split_submitted_file_references(input, cwd);
    if file_args.is_empty() {
        return Ok(vec![ContentBlock::Text(TextContent::new(
            input.to_string(),
        ))]);
    }

    let processed = process_file_arguments(&file_args, cwd, auto_resize_images)?;
    let mut text = processed.text;
    if !message_text.trim().is_empty() {
        text.push_str(message_text.trim());
    }

    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentBlock::Text(TextContent::new(text)));
    }
    content.extend(processed.images.into_iter().map(ContentBlock::Image));
    Ok(content)
}

fn split_submitted_file_references(input: &str, cwd: &Path) -> (String, Vec<String>) {
    let tokens = parse_command_args(input);
    if tokens.is_empty() {
        return (String::new(), Vec::new());
    }

    if tokens.len() == 1 && token_is_single_path_reference(&tokens[0], cwd) {
        return (
            String::new(),
            vec![normalize_file_reference_token(&tokens[0])],
        );
    }

    let mut message = Vec::new();
    let mut files = Vec::new();
    for token in tokens {
        if token.starts_with('@') || token.starts_with("file://") {
            files.push(normalize_file_reference_token(&token));
        } else {
            message.push(token);
        }
    }
    (message.join(" "), files)
}

fn token_is_single_path_reference(token: &str, cwd: &Path) -> bool {
    if token.starts_with('@') || token.starts_with("file://") {
        return true;
    }
    let path = Path::new(token);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    candidate.is_file()
}

fn normalize_file_reference_token(token: &str) -> String {
    let without_at = token.strip_prefix('@').unwrap_or(token);
    let without_scheme = without_at.strip_prefix("file://").unwrap_or(without_at);
    without_scheme.replace("%20", " ")
}

async fn handle_slash_command(
    agent: &mut AgentSession,
    context: &mut InteractiveContext,
    command: SlashCommand,
    args: &str,
    action_tx: pi_tui::ChatActionSender,
) -> Result<pi_tui::ChatAction> {
    let action = match command {
        SlashCommand::Help => status_action(SlashCommand::help_text(context.language())),
        SlashCommand::Exit => pi_tui::ChatAction::Quit,
        SlashCommand::Clear => pi_tui::ChatAction::Clear,
        SlashCommand::Model => handle_model_command(agent, context, args).await?,
        SlashCommand::Thinking => handle_thinking_command(agent, context, args).await?,
        SlashCommand::Session => status_action(format_session_status(agent).await?),
        SlashCommand::Settings => status_action(format_settings_status(context)),
        SlashCommand::Reload => handle_reload_command(agent, context).await?,
        SlashCommand::ScopedModels => status_action(format_scoped_models(context, args)),
        SlashCommand::Codegraph => status_action(handle_codegraph_command(context, args)?),
        SlashCommand::Hotkeys => status_action(context.options.key_hints.join("\n")),
        SlashCommand::Tree => status_action(format_tree_status(agent).await?),
        SlashCommand::New => pi_tui::ChatAction::Many(vec![
            pi_tui::ChatAction::Clear,
            status_action("已清空当前屏幕。要创建新的持久会话，请退出后使用 pi --new。"),
        ]),
        SlashCommand::Resume | SlashCommand::History => {
            handle_history_command(agent, context, args).await?
        }
        SlashCommand::Theme => handle_theme_command(context, args).await?,
        SlashCommand::Template => {
            return handle_template_command(agent, context, args, action_tx).await;
        }
        SlashCommand::Compact => handle_compact_command(agent).await?,
        SlashCommand::Copy => handle_copy_command(agent).await?,
        SlashCommand::Export => handle_export_command(agent, context, args).await?,
        SlashCommand::Name => handle_name_command(agent, args).await?,
        SlashCommand::Changelog => handle_changelog_command(args),
        SlashCommand::Language => handle_language_command(context, args)?,
        SlashCommand::Share => handle_share_command(agent, context, args).await?,
        SlashCommand::Login | SlashCommand::Logout | SlashCommand::Fork => {
            status_action(format_command_unavailable(command))
        }
    };
    Ok(action)
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

fn user_content_to_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => content_blocks_to_text(blocks),
    }
}

fn chat_lines_from_messages(messages: &[Message]) -> Vec<pi_tui::ChatLine> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => {
                Some(pi_tui::ChatLine::user(user_content_to_text(&user.content)))
            }
            Message::Assistant(assistant) => Some(pi_tui::ChatLine::assistant(
                content_blocks_to_text(&assistant.content),
            )),
            Message::ToolResult(result) => Some(pi_tui::ChatLine::status(format!(
                "tool: {} {}",
                result.tool_name,
                if result.is_error { "失败" } else { "完成" }
            ))),
            Message::Custom(custom) if custom.display => {
                Some(pi_tui::ChatLine::status(custom.content.clone()))
            }
            Message::Custom(_) => None,
        })
        .collect()
}

fn status_action(text: impl Into<String>) -> pi_tui::ChatAction {
    pi_tui::ChatAction::PushLine(pi_tui::ChatLine::status(text))
}

fn chat_action_contains_text(action: &pi_tui::ChatAction, needle: &str) -> bool {
    match action {
        pi_tui::ChatAction::PushLine(line) => line.text().contains(needle),
        pi_tui::ChatAction::Many(actions) => actions
            .iter()
            .any(|action| chat_action_contains_text(action, needle)),
        _ => false,
    }
}

fn model_label(entry: &ModelEntry) -> String {
    format!("{}/{}", entry.model.provider, entry.model.id)
}

fn resource_summary(resources: &ResourceLoader) -> String {
    format!(
        "资源: {} 技能, {} 提示, {} 主题",
        resources.skills().len(),
        resources.prompts().len(),
        resources.themes().len()
    )
}

async fn handle_model_command(
    agent: &mut AgentSession,
    context: &mut InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    if args.trim().is_empty() {
        return Ok(pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
            "模型",
            "/model",
            model_picker_items(context),
        )));
    }

    let Some(entry) = find_model_entry(&context.available_models, args) else {
        return Ok(status_action(format!("未找到模型: {args}")));
    };
    agent
        .set_provider_model(&entry.model.provider, &entry.model.id)
        .await?;
    context.current_model = entry;
    Ok(pi_tui::ChatAction::Many(vec![
        status_action(format!(
            "已切换模型: {}",
            model_label(&context.current_model)
        )),
        context.sync_footer_model(),
    ]))
}

fn model_picker_items(context: &InteractiveContext) -> Vec<pi_tui::PickerItem> {
    build_model_picker_items(&context.current_model, &context.available_models)
}

#[doc(hidden)]
pub fn build_model_picker_items(
    current_model: &ModelEntry,
    available_models: &[ModelEntry],
) -> Vec<pi_tui::PickerItem> {
    let current = model_label(current_model);
    available_models
        .iter()
        .map(|entry| {
            let value = model_label(entry);
            let marker = if value == current { "* " } else { "  " };
            let label = format!("{marker}{}", entry.model.id);
            let mut item = pi_tui::PickerItem::new(label, value, model_picker_description(entry))
                .with_group(provider_picker_group(entry));
            if let Some(reason) = model_unavailable_reason(entry) {
                item = item.disabled(reason);
            }
            item
        })
        .collect()
}

fn provider_picker_group(entry: &ModelEntry) -> String {
    format!("{} provider", entry.model.provider)
}

fn model_picker_description(entry: &ModelEntry) -> String {
    format!(
        "{} | auth: {} | input: {} | ctx: {} | max: {} | thinking: {}",
        entry.model.name,
        model_auth_status(entry),
        model_input_summary(entry),
        entry.model.context_window,
        entry.model.max_tokens,
        entry
            .available_thinking_levels()
            .into_iter()
            .map(|level| level.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn model_auth_status(entry: &ModelEntry) -> &'static str {
    if !entry.auth_header {
        return "keyless";
    }
    if entry
        .api_key
        .as_ref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        "configured"
    } else {
        "missing"
    }
}

fn model_unavailable_reason(entry: &ModelEntry) -> Option<&'static str> {
    (model_auth_status(entry) == "missing").then_some("missing credentials")
}

fn model_input_summary(entry: &ModelEntry) -> String {
    if entry.model.input.is_empty() {
        return "text".to_string();
    }
    entry
        .model
        .input
        .iter()
        .map(|input| match input {
            crate::provider::InputType::Text => "text",
            crate::provider::InputType::Image => "image",
        })
        .collect::<Vec<_>>()
        .join(",")
}

async fn handle_thinking_command(
    agent: &mut AgentSession,
    context: &mut InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    let current = agent
        .agent
        .stream_options()
        .thinking_level
        .unwrap_or_default();
    if args.trim().is_empty() {
        return Ok(status_action(format!(
            "当前 thinking: {current}\n用法: /thinking [off|minimal|low|medium|high|xhigh]"
        )));
    }
    let requested = args
        .trim()
        .parse::<ThinkingLevel>()
        .map_err(|err| anyhow::anyhow!("Invalid thinking level: {err}"))?;
    let effective = agent.clamp_thinking_level_for_model(
        &context.current_model.model.provider,
        &context.current_model.model.id,
        requested,
    );
    agent.agent.stream_options_mut().thinking_level = Some(effective);
    {
        let cx = crate::agent_cx::AgentCx::for_request();
        let mut session = agent
            .session
            .lock(cx.cx())
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let previous = session
            .effective_thinking_level_for_current_path()
            .as_deref()
            .and_then(|value| value.parse::<ThinkingLevel>().ok());
        session.set_model_header(None, None, Some(effective.to_string()));
        if previous != Some(effective) {
            session.append_thinking_level_change(effective.to_string());
        }
    }
    agent.persist_session().await?;
    let effective = agent
        .agent
        .stream_options()
        .thinking_level
        .unwrap_or_default();
    Ok(pi_tui::ChatAction::Many(vec![
        status_action(format!("thinking 已设置为: {effective}")),
        context.sync_footer_model(),
    ]))
}

fn format_model_listing(context: &InteractiveContext) -> String {
    let mut out = String::from("可用模型:\n");
    let current = model_label(&context.current_model);
    for entry in context.available_models.iter().take(80) {
        let label = model_label(entry);
        let marker = if label == current { "*" } else { " " };
        let reasoning = if entry.model.reasoning {
            "thinking"
        } else {
            "no-thinking"
        };
        let _ = writeln!(out, "{marker} {label}  {reasoning}");
    }
    if context.available_models.len() > 80 {
        let _ = writeln!(
            out,
            "... 还有 {} 个模型",
            context.available_models.len() - 80
        );
    }
    out.push_str("\n用法: /model <provider/model 或 model-id>");
    out
}

fn find_model_entry(available_models: &[ModelEntry], pattern: &str) -> Option<ModelEntry> {
    let patterns = parse_scoped_model_patterns(pattern);
    if patterns.is_empty() {
        return None;
    }
    let resolved = resolve_scoped_model_entries(available_models, &patterns);
    if resolved.len() == 1 {
        return resolved.into_iter().next();
    }
    let normalized = strip_thinking_level_suffix(pattern.trim()).to_ascii_lowercase();
    available_models
        .iter()
        .find(|entry| {
            let full = format!("{}/{}", entry.model.provider, entry.model.id).to_ascii_lowercase();
            full == normalized || entry.model.id.eq_ignore_ascii_case(&normalized)
        })
        .cloned()
}

async fn format_session_status(agent: &AgentSession) -> Result<String> {
    let cx = crate::agent_cx::AgentCx::for_request();
    let session = agent
        .session
        .lock(cx.cx())
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let model = session.effective_model_for_current_path().map_or_else(
        || "unknown".to_string(),
        |(provider, model)| format!("{provider}/{model}"),
    );
    let thinking = session
        .effective_thinking_level_for_current_path()
        .unwrap_or_else(|| "unknown".to_string());
    let path = session
        .path
        .as_ref()
        .map_or_else(|| "memory".to_string(), |path| path.display().to_string());
    Ok(format!(
        "会话信息\n路径: {path}\n消息: {}\n条目: {}\n模型: {model}\nthinking: {thinking}\n保存: {}",
        session
            .entries
            .iter()
            .filter(|entry| matches!(entry, SessionEntry::Message(_)))
            .count(),
        session.entries.len(),
        agent.save_enabled()
    ))
}

async fn format_tree_status(agent: &AgentSession) -> Result<String> {
    let cx = crate::agent_cx::AgentCx::for_request();
    let session = agent
        .session
        .lock(cx.cx())
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(format!(
        "对话树\n当前 leaf: {}\n条目: {}\n当前路径条目: {}",
        session.leaf_id().unwrap_or("<none>"),
        session.entries.len(),
        session.entries_for_current_path().len()
    ))
}

fn format_settings_status(context: &InteractiveContext) -> String {
    format!(
        "设置\n语言: {:?}\ncodegraph.autoInit: {}\ncodegraph.watch: {}\ncodegraph.debounceMs: {}",
        context.config.language(),
        context.config.codegraph_auto_init(),
        context.config.codegraph_watch(),
        context.config.codegraph_watch_debounce_ms()
    )
}

fn format_resource_status(context: &InteractiveContext) -> String {
    format_resource_status_for_loader("资源已加载", &context.resources)
}

async fn handle_reload_command(
    agent: &mut AgentSession,
    context: &mut InteractiveContext,
) -> Result<pi_tui::ChatAction> {
    let (resources, auth_result) = futures::future::join(
        ResourceLoader::load(
            &context.package_manager,
            &context.cwd,
            &context.config,
            &context.resource_cli,
        ),
        AuthStorage::load_async(context.auth_path.clone()),
    )
    .await;
    let resources = resources?;
    let mut auth = auth_result?;
    auth.refresh_expired_oauth_tokens().await?;
    let model_registry = ModelRegistry::load(&auth, Some(context.models_path.clone()));
    let models_error = model_registry.error().map(ToString::to_string);
    let available_models = model_registry.get_available();
    let previous_model = model_label(&context.current_model);
    let current_model_refreshed = if let Some(refreshed_current) = model_registry.find(
        &context.current_model.model.provider,
        &context.current_model.model.id,
    ) {
        context.current_model = refreshed_current;
        true
    } else {
        false
    };
    agent.set_auth_storage(auth);
    agent.set_model_registry(model_registry);
    context.available_models = available_models;
    context.resources = resources;
    let status = format_reload_status(
        &context.resources,
        context.available_models.len(),
        models_error.as_deref(),
        (!current_model_refreshed).then_some(previous_model.as_str()),
    );
    Ok(pi_tui::ChatAction::Many(vec![
        status_action(status),
        context.sync_resource_options(),
    ]))
}

#[doc(hidden)]
pub fn format_reload_status(
    resources: &ResourceLoader,
    model_count: usize,
    models_error: Option<&str>,
    retained_model: Option<&str>,
) -> String {
    let mut out = format_resource_status_for_loader("资源已重新加载", resources);
    let _ = writeln!(out, "- models: {model_count}");
    if let Some(error) = models_error {
        let _ = writeln!(out, "- models.json: {error}");
    }
    if let Some(model) = retained_model {
        let _ = writeln!(out, "- current model retained: {model}");
    }
    out
}

#[doc(hidden)]
pub fn format_resource_status_for_loader(prefix: &str, resources: &ResourceLoader) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{prefix}");
    let _ = writeln!(out, "- skills: {}", resources.skills().len());
    let _ = writeln!(out, "- prompts: {}", resources.prompts().len());
    let _ = writeln!(out, "- themes: {}", resources.themes().len());
    let _ = writeln!(
        out,
        "- diagnostics: {}",
        resource_diagnostic_count(resources)
    );
    out
}

fn resource_diagnostic_count(resources: &ResourceLoader) -> usize {
    resources.skill_diagnostics().len()
        + resources.prompt_diagnostics().len()
        + resources.theme_diagnostics().len()
}

fn format_scoped_models(context: &InteractiveContext, args: &str) -> String {
    let patterns = parse_scoped_model_patterns(args);
    if patterns.is_empty() {
        return "用法: /scoped-models <pattern...>".to_string();
    }
    let resolved = resolve_scoped_model_entries(&context.available_models, &patterns);
    if resolved.is_empty() {
        return "没有匹配的模型。".to_string();
    }
    let mut out = String::from("匹配模型:\n");
    for entry in resolved {
        let _ = writeln!(out, "- {}", model_label(&entry));
    }
    out
}

fn handle_codegraph_command(context: &InteractiveContext, args: &str) -> Result<String> {
    let command = args.trim();
    match command {
        "" => Ok("用法: /codegraph [init|sync|status]".to_string()),
        "init" | "sync" => {
            let index = pi_codegraph::CodeGraphIndex::open(&context.cwd)?;
            let sync = index.sync_project()?;
            let files = index.indexed_files()?.len();
            Ok(format!(
                "Codegraph {}\n数据库: {}\n文件: {}\n已索引: {}  未变化: {}  已移除: {}  已跳过: {}",
                command,
                index.db_path().display(),
                files,
                sync.indexed_files,
                sync.unchanged_files,
                sync.removed_files,
                sync.skipped_files
            ))
        }
        "status" => {
            let db_path = pi_codegraph::project_db_path(&context.cwd);
            match pi_codegraph::CodeGraphIndex::open_existing(&context.cwd) {
                Ok(index) => Ok(format!(
                    "Codegraph 状态\n已初始化: true\n数据库: {}\n文件: {}",
                    index.db_path().display(),
                    index.indexed_files()?.len()
                )),
                Err(pi_codegraph::CodeGraphError::IndexNotInitialized(_)) => Ok(format!(
                    "Codegraph 状态\n已初始化: false\n数据库: {}",
                    db_path.display()
                )),
                Err(err) => Err(err.into()),
            }
        }
        other => Ok(format!(
            "不支持的 codegraph 命令: {other}\n用法: /codegraph [init|sync|status]"
        )),
    }
}

fn format_theme_status(context: &InteractiveContext) -> String {
    let mut out = String::from("主题:\n");
    out.push_str("- dark (built-in)\n- light (built-in)\n- solarized (built-in)\n");
    for theme in context.resources.themes().iter().take(50) {
        let _ = writeln!(out, "- {} ({})", theme.name, theme.source);
    }
    out
}

async fn handle_theme_command(
    context: &mut InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    if !args.trim().is_empty() {
        let theme_name = args.trim();
        let Some(theme) = resolve_interactive_theme(context, theme_name) else {
            return Ok(status_action(format!("未找到主题: {theme_name}")));
        };
        Config::patch_settings_with_roots(
            SettingsScope::Project,
            &Config::global_dir(),
            &context.cwd,
            json!({ "theme": theme_name }),
        )?;
        context.config.theme = Some(theme_name.to_string());
        return Ok(pi_tui::ChatAction::Many(vec![
            status_action(format!("主题已切换为: {}", theme.name)),
            context.sync_theme_options(theme),
        ]));
    }
    let mut items = vec![
        pi_tui::PickerItem::new("dark", "dark", "built-in"),
        pi_tui::PickerItem::new("light", "light", "built-in"),
        pi_tui::PickerItem::new("solarized", "solarized", "built-in"),
    ];
    items.extend(context.resources.themes().iter().map(|theme| {
        pi_tui::PickerItem::new(theme.name.clone(), theme.name.clone(), theme.source.clone())
    }));
    Ok(pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
        "主题", "/theme", items,
    )))
}

fn resolve_interactive_theme(context: &InteractiveContext, theme_name: &str) -> Option<Theme> {
    match theme_name {
        name if name.eq_ignore_ascii_case("dark") => Some(Theme::dark()),
        name if name.eq_ignore_ascii_case("light") => Some(Theme::light()),
        name if name.eq_ignore_ascii_case("solarized") => Some(Theme::solarized()),
        name => context.resources.resolve_theme(Some(name)),
    }
}

fn format_template_status(context: &InteractiveContext) -> String {
    let mut out = String::from("Prompt templates:\n");
    for prompt in context.resources.prompts().iter().take(50) {
        let _ = writeln!(out, "- {} ({})", prompt.name, prompt.source);
    }
    if context.resources.prompts().is_empty() {
        out.push_str("未加载 prompt template。");
    }
    out
}

async fn handle_template_command(
    agent: &mut AgentSession,
    context: &InteractiveContext,
    args: &str,
    action_tx: pi_tui::ChatActionSender,
) -> Result<pi_tui::ChatAction> {
    if !args.trim().is_empty() {
        let mut parsed = parse_command_args(args);
        let Some(name) = parsed.first().cloned() else {
            return Ok(status_action("Prompt template 名称不能为空。"));
        };
        let Some(template) = context
            .resources
            .prompts()
            .iter()
            .find(|prompt| prompt.name == name)
        else {
            return Ok(status_action(format!("未找到 prompt template: {name}")));
        };
        parsed.remove(0);
        let expanded = substitute_args(&template.content, &parsed);
        let expanded = expanded.trim().to_string();
        if expanded.is_empty() {
            return Ok(status_action(format!(
                "Prompt template 展开为空: {}",
                template.name
            )));
        }
        return run_user_prompt(agent, context, expanded, action_tx, true).await;
    }
    let items = context
        .resources
        .prompts()
        .iter()
        .map(|prompt| {
            pi_tui::PickerItem::new(
                prompt.name.clone(),
                prompt.name.clone(),
                prompt.description.clone(),
            )
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return Ok(status_action("未加载 prompt template。"));
    }
    Ok(pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
        "Prompt templates",
        "/template",
        items,
    )))
}

async fn handle_compact_command(agent: &mut AgentSession) -> Result<pi_tui::ChatAction> {
    let events = Arc::new(StdMutex::new(Vec::new()));
    let events_for_callback = Arc::clone(&events);
    let result = agent
        .compact_now(move |event: AgentEvent| {
            if let Ok(mut events) = events_for_callback.lock() {
                events.push(event);
            }
        })
        .await;

    let mut actions = Vec::new();
    let captured_events = events
        .lock()
        .map(|events| events.clone())
        .unwrap_or_default();
    for event in &captured_events {
        if let Some(status) = format_compaction_status(event) {
            actions.push(status_action(status));
        }
    }

    match result {
        Ok(()) if actions.is_empty() => {
            actions.push(status_action("上下文无需压缩：没有可压缩的历史记录。"));
            Ok(pi_tui::ChatAction::Many(actions))
        }
        Ok(()) => Ok(pi_tui::ChatAction::Many(actions)),
        Err(err) => {
            if actions
                .iter()
                .all(|action| !chat_action_contains_text(action, &err.to_string()))
            {
                actions.push(status_action(format!("上下文压缩失败: {err}")));
            }
            Ok(pi_tui::ChatAction::Many(actions))
        }
    }
}

async fn handle_copy_command(agent: &AgentSession) -> Result<pi_tui::ChatAction> {
    let Some(text) = last_assistant_text_for_tui(agent).await? else {
        return Ok(status_action("没有可复制的 assistant 消息。"));
    };

    match copy_text_to_clipboard(&text) {
        Ok(()) => Ok(status_action(format!(
            "已复制上一条 assistant 消息到剪贴板（{} chars）。",
            text.chars().count()
        ))),
        Err(err) => Ok(status_action(format!("复制失败: {err}"))),
    }
}

async fn handle_export_command(
    agent: &AgentSession,
    context: &InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    let export = export_current_session_for_tui(agent, &context.cwd, args.trim()).await?;
    Ok(status_action(format!(
        "已导出当前会话为 {}: {}",
        export.format,
        export.path.display()
    )))
}

async fn handle_share_command(
    agent: &AgentSession,
    context: &InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    let public = parse_share_public_arg(args);
    match share_current_session_for_tui(agent, &context.config, &context.cwd, public).await {
        Ok(result) => Ok(status_action(format!(
            "Created {} gist.\nShare URL: {}",
            if result.public { "public" } else { "private" },
            result.viewer_url
        ))),
        Err(err) => Ok(status_action(format!("分享失败: {err}"))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiShareResult {
    pub gist_url: String,
    pub viewer_url: String,
    pub public: bool,
}

#[doc(hidden)]
pub async fn share_current_session_for_tui(
    agent: &AgentSession,
    config: &Config,
    cwd: &Path,
    public: bool,
) -> Result<TuiShareResult> {
    let export = export_current_session_for_tui(agent, cwd, "").await?;
    let gh_path = config.gh_path.as_deref().unwrap_or("gh");
    ensure_gh_available(gh_path).await?;

    let description = format!(
        "Pi session share {}",
        export
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session")
    );
    let output = Command::new(gh_path)
        .arg("gist")
        .arg("create")
        .arg(&export.path)
        .arg("--desc")
        .arg(description)
        .arg(format!("--public={public}"))
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh gist create failed: {}", stderr.trim());
    }

    let gist_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let gist_id = gist_id_from_url(&gist_url).unwrap_or(gist_url.as_str());
    let viewer_url = crate::session::get_share_viewer_url(gist_id);
    Ok(TuiShareResult {
        gist_url,
        viewer_url,
        public,
    })
}

async fn ensure_gh_available(gh_path: &str) -> Result<()> {
    let output = Command::new(gh_path)
        .arg("auth")
        .arg("status")
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("GitHub CLI auth failed: {}", stderr.trim());
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "gh not found. Install GitHub CLI from https://cli.github.com/ and run `gh auth login`."
            )
        }
        Err(err) => Err(err.into()),
    }
}

fn parse_share_public_arg(args: &str) -> bool {
    args.split_whitespace()
        .any(|arg| matches!(arg, "public" | "--public" | "--public=true"))
}

fn gist_id_from_url(url: &str) -> Option<&str> {
    url.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiExportResult {
    pub path: PathBuf,
    pub format: &'static str,
}

#[doc(hidden)]
pub async fn export_current_session_for_tui(
    agent: &AgentSession,
    cwd: &Path,
    output_arg: &str,
) -> Result<TuiExportResult> {
    let (snapshot, messages) = {
        let cx = crate::agent_cx::AgentCx::for_request();
        let session = agent
            .session
            .lock(cx.cx())
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        (
            session.export_snapshot(),
            session.to_messages_for_current_path(),
        )
    };
    let path = export_output_path(&snapshot, cwd, output_arg);
    let format = export_format_for_path(&path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        crate::fs::create_dir_all(parent).await?;
    }
    match format {
        "JSON" => {
            let json = serde_json::to_string_pretty(&messages)?;
            crate::fs::write(&path, json).await?;
        }
        "HTML" => {
            crate::fs::write(&path, snapshot.to_html()).await?;
        }
        _ => unreachable!("known export format"),
    }
    Ok(TuiExportResult { path, format })
}

fn export_output_path(
    snapshot: &crate::session::ExportSnapshot,
    cwd: &Path,
    output_arg: &str,
) -> PathBuf {
    let path = if output_arg.is_empty() {
        let id = snapshot.header.id.trim();
        let basename = if id.is_empty() { "session" } else { id };
        PathBuf::from(format!("pi-session-{basename}.html"))
    } else {
        PathBuf::from(output_arg)
    };
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn export_format_for_path(path: &Path) -> &'static str {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        "JSON"
    } else {
        "HTML"
    }
}

fn handle_changelog_command(args: &str) -> pi_tui::ChatAction {
    let selector = args.trim();
    if selector.is_empty() {
        let items = changelog_picker_items();
        if items.is_empty() {
            return status_action("没有可显示的 changelog 条目。");
        }
        return pi_tui::ChatAction::OpenPicker(
            pi_tui::ChatPicker::new("Changelog", "/changelog", items)
                .with_subtitle("选择一个版本查看完整条目")
                .with_empty_message("没有匹配的 changelog 条目"),
        );
    }

    match format_changelog_entry(selector) {
        Some(entry) => status_action(entry),
        None => status_action(format!("未找到 changelog 条目: {selector}")),
    }
}

#[doc(hidden)]
pub fn startup_changelog_lines(config: &Config) -> Vec<pi_tui::ChatLine> {
    if config.quiet_startup.unwrap_or(false) || config.collapse_changelog.unwrap_or(false) {
        return Vec::new();
    }

    let Some(latest) = changelog_sections().into_iter().next() else {
        return Vec::new();
    };
    let current_version = latest.title.trim();
    if config
        .last_changelog_version
        .as_deref()
        .is_some_and(|version| version == current_version)
    {
        return Vec::new();
    }

    vec![pi_tui::ChatLine::status(format!(
        "Changelog: {current_version}\n输入 /changelog 查看完整更新记录。"
    ))]
}

#[doc(hidden)]
pub fn changelog_picker_items() -> Vec<pi_tui::PickerItem> {
    changelog_sections()
        .into_iter()
        .enumerate()
        .map(|(index, section)| {
            pi_tui::PickerItem::new(
                section.title.clone(),
                index.to_string(),
                changelog_section_description(&section.body),
            )
        })
        .collect()
}

#[doc(hidden)]
pub fn format_changelog_entry(selector: &str) -> Option<String> {
    let sections = changelog_sections();
    let selected = selector
        .parse::<usize>()
        .ok()
        .and_then(|index| sections.get(index))
        .or_else(|| {
            sections.iter().find(|section| {
                section
                    .title
                    .to_ascii_lowercase()
                    .contains(&selector.to_ascii_lowercase())
            })
        })?;
    Some(format!("{}\n\n{}", selected.title, selected.body.trim()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangelogSection {
    title: String,
    body: String,
}

fn changelog_sections() -> Vec<ChangelogSection> {
    let mut sections = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_body = Vec::new();

    for line in include_str!("../CHANGELOG.md").lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(title) = current_title.take() {
                sections.push(ChangelogSection {
                    title,
                    body: current_body.join("\n"),
                });
                current_body.clear();
            }
            current_title = Some(title.trim().to_string());
        } else if current_title.is_some() {
            current_body.push(line.to_string());
        }
    }

    if let Some(title) = current_title {
        sections.push(ChangelogSection {
            title,
            body: current_body.join("\n"),
        });
    }

    sections
}

fn changelog_section_description(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map_or_else(|| "No details".to_string(), |line| truncate_line(line, 120))
}

#[doc(hidden)]
pub async fn last_assistant_text_for_tui(agent: &AgentSession) -> Result<Option<String>> {
    let cx = crate::agent_cx::AgentCx::for_request();
    let session = agent
        .session
        .lock(cx.cx())
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    for entry in session.entries_for_current_path().into_iter().rev() {
        let crate::session::SessionEntry::Message(message_entry) = entry else {
            continue;
        };
        let crate::session::SessionMessage::Assistant { message } = &message_entry.message else {
            continue;
        };
        let text = content_blocks_to_text(&message.content);
        if !text.trim().is_empty() {
            return Ok(Some(text));
        }
    }

    Ok(None)
}

#[cfg(feature = "clipboard")]
fn copy_text_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

#[cfg(not(feature = "clipboard"))]
fn copy_text_to_clipboard(_text: &str) -> Result<()> {
    anyhow::bail!("当前构建未启用 clipboard feature，无法写入系统剪贴板。")
}

async fn handle_name_command(agent: &mut AgentSession, args: &str) -> Result<pi_tui::ChatAction> {
    let name = args.trim();
    if let Some(error) = validate_session_name_for_tui(name) {
        return Ok(status_action(error));
    }
    let cx = crate::agent_cx::AgentCx::for_request();
    let entry_id;
    {
        let mut session = agent
            .session
            .lock(cx.cx())
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        entry_id = session.set_name(name);
    }
    agent.persist_session().await?;
    Ok(status_action(format_session_name_status(name, &entry_id)))
}

fn validate_session_name_for_tui(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("用法: /name <session-name>".to_string());
    }
    if name.chars().any(|ch| ch == '\n' || ch == '\r') {
        return Some("会话名称不能包含换行。".to_string());
    }
    if name.chars().count() > 120 {
        return Some("会话名称过长，最多 120 个字符。".to_string());
    }
    None
}

#[doc(hidden)]
pub fn format_session_name_status(name: &str, entry_id: &str) -> String {
    format!("会话已命名\n名称: {name}\n记录: {entry_id}")
}

fn handle_language_command(
    context: &mut InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
            "语言",
            "/language",
            vec![
                pi_tui::PickerItem::new("中文", "zh", "中文 UI 和 prompt"),
                pi_tui::PickerItem::new("English", "en", "English UI and prompts"),
            ],
        )));
    }

    let Some(language) = parse_language_arg(trimmed) else {
        return Ok(status_action("不支持的语言。用法: /language [zh|en]"));
    };

    let language_code = match language {
        Language::Zh => "zh".to_string(),
        Language::En => "en".to_string(),
    };
    Config::patch_settings_with_roots(
        SettingsScope::Project,
        &Config::global_dir(),
        &context.cwd,
        json!({ "language": language_code }),
    )?;
    apply_language_selection(context, language, language_code);
    Ok(pi_tui::ChatAction::Many(vec![
        status_action(
            PromptCatalog::new(language)
                .ui_text()
                .language_updated(language),
        ),
        pi_tui::ChatAction::SetOptions(Box::new(context.options.clone())),
    ]))
}

fn apply_language_selection(
    context: &mut InteractiveContext,
    language: Language,
    language_code: String,
) {
    context.config.language = Some(language_code);
    context.options.status = match language {
        Language::Zh => "就绪".to_string(),
        Language::En => "Ready".to_string(),
    };
    context.options.resource_summary = resource_summary(&context.resources);
    context.options.command_hints = vec![
        "/help".to_string(),
        "/model".to_string(),
        "/thinking".to_string(),
        "/session".to_string(),
        "/settings".to_string(),
        "/codegraph status".to_string(),
        "/exit".to_string(),
    ];
    context.options.slash_commands = slash_command_items(language);
}

async fn handle_history_command(
    agent: &mut AgentSession,
    context: &InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    let trimmed = args.trim();
    if !trimmed.is_empty() && !matches!(trimmed, "all" | "--all") {
        return resume_session_from_path_for_tui(agent, trimmed).await;
    }
    let index = crate::session_index::SessionIndex::new();
    let cwd = context.cwd.display().to_string();
    let scope = if matches!(trimmed, "all" | "--all") {
        SessionPickerScope::All
    } else {
        SessionPickerScope::Project
    };
    let sessions = index
        .list_sessions(scope.cwd_filter(&cwd))
        .unwrap_or_default();
    if sessions.is_empty() {
        return Ok(status_action(scope.empty_status()));
    }
    Ok(pi_tui::ChatAction::OpenPicker(
        pi_tui::ChatPicker::new("会话列表", "/resume", session_picker_items(sessions))
            .with_subtitle(scope.subtitle())
            .with_empty_message("没有匹配的会话。输入其他关键字，或用 /history all 查看全部会话。"),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPickerScope {
    Project,
    All,
}

impl SessionPickerScope {
    fn cwd_filter(self, cwd: &str) -> Option<&str> {
        match self {
            Self::Project => Some(cwd),
            Self::All => None,
        }
    }

    const fn subtitle(self) -> &'static str {
        match self {
            Self::Project => {
                "scope: current project  |  type to search  |  /history all for all sessions"
            }
            Self::All => "scope: all sessions  |  type to search",
        }
    }

    const fn empty_status(self) -> &'static str {
        match self {
            Self::Project => "当前项目没有可恢复会话。使用 /history all 查看全部会话。",
            Self::All => "没有可恢复会话。",
        }
    }
}

#[doc(hidden)]
pub fn session_picker_items(
    sessions: Vec<crate::session_index::SessionMeta>,
) -> Vec<pi_tui::PickerItem> {
    sessions
        .into_iter()
        .take(100)
        .map(session_picker_item)
        .collect()
}

fn session_picker_item(session: crate::session_index::SessionMeta) -> pi_tui::PickerItem {
    let label = session
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| session.id.clone());
    let size_kib = session.size_bytes.div_ceil(1024);
    let file_name = Path::new(&session.path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(session.path.as_str());
    let description = format!(
        "{} messages  {} KiB  {}  {}",
        session.message_count, size_kib, session.timestamp, file_name
    );
    pi_tui::PickerItem::new(label, session.path, description).with_group(session.cwd)
}

#[doc(hidden)]
pub async fn resume_session_from_path_for_tui(
    agent: &mut AgentSession,
    selected: &str,
) -> Result<pi_tui::ChatAction> {
    let mut session = crate::session::Session::open(selected).await?;
    let history = session.to_messages_for_current_path();
    let visible_lines = chat_lines_from_messages(&history);
    let cx = crate::agent_cx::AgentCx::for_request();
    {
        let mut active = agent
            .session
            .lock(cx.cx())
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        session.set_autosave_durability_mode(active.autosave_durability_mode());
        *active = session;
    }
    agent.agent.replace_messages(history);
    Ok(pi_tui::ChatAction::Many(vec![
        pi_tui::ChatAction::ReplaceLines(visible_lines),
        status_action(format!("已恢复会话: {selected}")),
    ]))
}

fn parse_language_arg(value: &str) -> Option<Language> {
    match value.to_ascii_lowercase().as_str() {
        "zh" | "cn" | "chinese" | "中文" => Some(Language::Zh),
        "en" | "english" => Some(Language::En),
        _ => None,
    }
}

fn format_command_unavailable(command: SlashCommand) -> String {
    match command {
        SlashCommand::Login => {
            "/login 需要 OAuth/API key 专用交互流程；当前请使用非交互 CLI 登录流程。".to_string()
        }
        SlashCommand::Logout => {
            "/logout 需要认证状态写入确认；当前请使用配置/auth 文件管理命令。".to_string()
        }
        SlashCommand::Fork => {
            "/fork 需要分支选择器；当前请使用 /tree 查看当前路径信息。".to_string()
        }
        _ => format!("/{command:?} 当前不可用。"),
    }
}

#[doc(hidden)]
pub fn format_agent_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::TurnStart { turn_index, .. } => Some(format!("turn {turn_index}: 开始")),
        AgentEvent::TurnEnd {
            turn_index,
            tool_results,
            latency_breakdown,
            ..
        } => {
            let latency = latency_breakdown
                .as_ref()
                .map_or(String::new(), |breakdown| {
                    format!("  {}ms", breakdown.total_ms)
                });
            Some(format!(
                "turn {turn_index}: 完成，工具结果 {}{}",
                tool_results.len(),
                latency
            ))
        }
        AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => Some(format!("tool: {tool_name} 开始 {}", compact_json(args))),
        AgentEvent::ToolExecutionUpdate {
            tool_name,
            partial_result,
            ..
        } => Some(format!(
            "tool: {tool_name} 更新 {}",
            summarize_tool_output(partial_result)
        )),
        AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } => Some(format!(
            "tool: {tool_name} {} {}",
            if *is_error { "失败" } else { "完成" },
            summarize_tool_output(result)
        )),
        AgentEvent::AutoCompactionStart { .. } | AgentEvent::AutoCompactionEnd { .. } => {
            format_compaction_status(event)
        }
        AgentEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
            error_message,
        } => Some(format!(
            "自动重试 {attempt}/{max_attempts}: {delay_ms}ms 后重试，原因: {error_message}"
        )),
        AgentEvent::AutoRetryEnd {
            success,
            attempt,
            final_error,
        } => Some(format!(
            "自动重试结束 attempt={attempt} success={success}{}",
            final_error
                .as_ref()
                .map_or(String::new(), |err| format!(" error={err}"))
        )),
        AgentEvent::AgentStart { .. }
        | AgentEvent::AgentEnd { .. }
        | AgentEvent::MessageStart { .. }
        | AgentEvent::MessageUpdate { .. }
        | AgentEvent::MessageEnd { .. } => None,
    }
}

#[doc(hidden)]
pub fn format_compaction_status(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::AutoCompactionStart { reason } => Some(format!("上下文压缩开始: {reason}")),
        AgentEvent::AutoCompactionEnd {
            result,
            aborted,
            will_retry,
            error_message,
        } => {
            if let Some(error) = error_message {
                return Some(format!(
                    "上下文压缩失败: aborted={aborted} retry={will_retry} error={error}"
                ));
            }
            let Some(result) = result else {
                return Some(format!(
                    "上下文压缩结束: aborted={aborted} retry={will_retry}"
                ));
            };

            let summary = result
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(trim_compaction_summary);
            let first_kept = result
                .get("firstKeptEntryId")
                .and_then(serde_json::Value::as_str);
            let tokens_before = result
                .get("tokensBefore")
                .and_then(serde_json::Value::as_u64);
            let read_files = result
                .pointer("/details/readFiles")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
            let modified_files = result
                .pointer("/details/modifiedFiles")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);

            let mut status = String::from("上下文压缩完成");
            if let Some(tokens_before) = tokens_before {
                let _ = write!(status, "\n- tokens before: {tokens_before}");
            }
            if let Some(first_kept) = first_kept {
                let _ = write!(status, "\n- first kept entry: {first_kept}");
            }
            let _ = write!(
                status,
                "\n- files: {read_files} read, {modified_files} modified"
            );
            if let Some(summary) = summary
                && !summary.is_empty()
            {
                let _ = write!(status, "\n\n{summary}");
            }
            Some(status)
        }
        _ => None,
    }
}

fn trim_compaction_summary(summary: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 1_200;
    let trimmed_summary = summary.trim();
    let mut trimmed = trimmed_summary
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect::<String>();
    if trimmed_summary.chars().count() > MAX_SUMMARY_CHARS {
        trimmed.push_str("...");
    }
    trimmed
}

fn compact_json(value: &serde_json::Value) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| "<json>".to_string());
    truncate_line(&raw, 180)
}

fn summarize_tool_output(output: &crate::tools::ToolOutput) -> String {
    let content = content_blocks_to_text(&output.content);
    let content = if content.trim().is_empty() {
        output
            .details
            .as_ref()
            .map_or_else(String::new, compact_json)
    } else {
        content
    };
    truncate_line(content.trim(), 220)
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r')
        .enumerate()
    {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
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
