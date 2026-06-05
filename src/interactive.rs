//! Ratatui interactive mode entry points and shared command parsing helpers.

use anyhow::Result;
use pi_core::model::{ContentBlock, TextContent};
use pi_prompt::PromptCatalog;

use crate::agent::{AgentEvent, AgentSession};
use crate::config::{Config, Language, SettingsScope};
use crate::model::{Message, ThinkingLevel, UserContent};
use crate::models::ModelEntry;
use crate::package_manager::ResolvedResource;
use crate::resources::{ResourceCliOptions, ResourceLoader, parse_command_args, substitute_args};
use crate::runtime::RuntimeHandle;
use crate::session::SessionEntry;
use pi_theme::Theme;
use serde_json::json;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

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
    cwd: PathBuf,
    options: pi_tui::ChatOptions,
}

impl InteractiveContext {
    fn new(
        config: Config,
        current_model: ModelEntry,
        available_models: Vec<ModelEntry>,
        resources: ResourceLoader,
        cwd: PathBuf,
    ) -> Self {
        let mut options = pi_tui::ChatOptions::new(model_label(&current_model));
        options.status = "就绪".to_string();
        options.theme = Theme::resolve(&config, &cwd);
        options.resource_summary = resource_summary(&resources);
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
            "Shift+Enter: newline".to_string(),
            "Ctrl+C: quit".to_string(),
            "Esc Esc: 会话".to_string(),
            "/help".to_string(),
        ];
        options.slash_commands = slash_command_items(config.language());
        Self {
            config,
            current_model,
            available_models,
            resources,
            cwd,
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
            ("/tree", "显示对话树信息"),
            ("/compact", "压缩上下文"),
            ("/clear", "清屏"),
            ("/reload", "显示资源加载状态"),
            ("/template", "选择 prompt template"),
            ("/language", "切换语言"),
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
            ("/tree", "Show conversation tree info"),
            ("/compact", "Compact context"),
            ("/clear", "Clear screen"),
            ("/reload", "Show resource status"),
            ("/template", "Pick prompt template"),
            ("/language", "Switch language"),
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
    _resource_cli: ResourceCliOptions,
    cwd: PathBuf,
    _runtime_handle: RuntimeHandle,
) -> Result<()> {
    let context = InteractiveContext::new(config, model_entry, available_models, resources, cwd);
    let options = context.options.clone();
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

    run_user_prompt(agent, input, action_tx, false).await
}

async fn run_user_prompt(
    agent: &mut AgentSession,
    input: String,
    action_tx: pi_tui::ChatActionSender,
    echo_user: bool,
) -> Result<pi_tui::ChatAction> {
    let event_sink = action_tx.clone();
    let assistant = agent
        .run_with_content(
            vec![ContentBlock::Text(TextContent::new(input.clone()))],
            move |event| {
                if let Some(line) = format_agent_event(&event) {
                    let _ = event_sink
                        .send(pi_tui::ChatAction::PushLine(pi_tui::ChatLine::status(line)));
                }
            },
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
    let assistant_line = pi_tui::ChatAction::PushLine(pi_tui::ChatLine::assistant(answer));
    if echo_user {
        Ok(pi_tui::ChatAction::Many(vec![
            pi_tui::ChatAction::PushLine(pi_tui::ChatLine::user(input)),
            assistant_line,
        ]))
    } else {
        Ok(assistant_line)
    }
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
        SlashCommand::Reload => status_action(format_resource_status(context)),
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
        SlashCommand::Name => handle_name_command(agent, args).await?,
        SlashCommand::Language => handle_language_command(context, args),
        SlashCommand::Login
        | SlashCommand::Logout
        | SlashCommand::Export
        | SlashCommand::Copy
        | SlashCommand::Fork
        | SlashCommand::Share
        | SlashCommand::Changelog => status_action(format_command_unavailable(command)),
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
    context
        .available_models
        .iter()
        .map(|entry| {
            let label = model_label(entry);
            let reasoning = if entry.model.reasoning {
                "thinking"
            } else {
                "no-thinking"
            };
            pi_tui::PickerItem::new(label.clone(), label, reasoning)
        })
        .collect()
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
    let mut out = String::new();
    let _ = writeln!(out, "资源已加载");
    let _ = writeln!(out, "- skills: {}", context.resources.skills().len());
    let _ = writeln!(out, "- prompts: {}", context.resources.prompts().len());
    let _ = writeln!(out, "- themes: {}", context.resources.themes().len());
    out
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
        return run_user_prompt(agent, expanded, action_tx, true).await;
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
    agent.compact_now(|_event: AgentEvent| {}).await?;
    Ok(status_action("压缩已完成。"))
}

async fn handle_name_command(agent: &mut AgentSession, args: &str) -> Result<pi_tui::ChatAction> {
    let name = args.trim();
    if name.is_empty() {
        return Ok(status_action("用法: /name <session-name>"));
    }
    let cx = crate::agent_cx::AgentCx::for_request();
    {
        let mut session = agent
            .session
            .lock(cx.cx())
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        session.set_name(name);
    }
    agent.persist_session().await?;
    Ok(status_action(format!("会话已命名: {name}")))
}

fn handle_language_command(context: &mut InteractiveContext, args: &str) -> pi_tui::ChatAction {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
            "语言",
            "/language",
            vec![
                pi_tui::PickerItem::new("中文", "zh", "中文 UI 和 prompt"),
                pi_tui::PickerItem::new("English", "en", "English UI and prompts"),
            ],
        ));
    }

    let Some(language) = parse_language_arg(trimmed) else {
        return status_action("不支持的语言。用法: /language [zh|en]");
    };

    context.config.language = Some(match language {
        Language::Zh => "zh".to_string(),
        Language::En => "en".to_string(),
    });
    context.options.status = match language {
        Language::Zh => "就绪".to_string(),
        Language::En => "Ready".to_string(),
    };
    context.options.resource_summary = resource_summary(&context.resources);
    context.options.slash_commands = slash_command_items(language);
    pi_tui::ChatAction::Many(vec![
        status_action(
            PromptCatalog::new(language)
                .ui_text()
                .language_updated(language),
        ),
        pi_tui::ChatAction::SetOptions(Box::new(context.options.clone())),
    ])
}

async fn handle_history_command(
    agent: &mut AgentSession,
    context: &InteractiveContext,
    args: &str,
) -> Result<pi_tui::ChatAction> {
    if !args.trim().is_empty() {
        let selected = args.trim();
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
        return Ok(pi_tui::ChatAction::Many(vec![
            pi_tui::ChatAction::ReplaceLines(visible_lines),
            status_action(format!("已恢复会话: {selected}")),
        ]));
    }
    let index = crate::session_index::SessionIndex::new();
    let cwd = context.cwd.display().to_string();
    let sessions = index.list_sessions(Some(&cwd)).unwrap_or_default();
    if sessions.is_empty() {
        return Ok(status_action("当前项目没有可恢复会话。"));
    }
    let items = sessions
        .into_iter()
        .take(100)
        .map(|session| {
            let label = session
                .name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| session.id.clone());
            let description = format!("{} messages  {}", session.message_count, session.timestamp);
            pi_tui::PickerItem::new(label, session.path, description)
        })
        .collect::<Vec<_>>();
    Ok(pi_tui::ChatAction::OpenPicker(pi_tui::ChatPicker::new(
        "会话列表",
        "/resume",
        items,
    )))
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
        SlashCommand::Export => "/export 需要选择导出目标；当前请使用会话导出 CLI。".to_string(),
        SlashCommand::Copy => "/copy 需要剪贴板接入；当前请用终端选择文本复制。".to_string(),
        SlashCommand::Fork => {
            "/fork 需要分支选择器；当前请使用 /tree 查看当前路径信息。".to_string()
        }
        SlashCommand::Share => {
            "/share 需要分享后端/浏览器流程；当前 TUI 不自动上传会话。".to_string()
        }
        SlashCommand::Changelog => include_str!("../CHANGELOG.md")
            .lines()
            .take(80)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => format!("/{command:?} 当前不可用。"),
    }
}

fn format_agent_event(event: &AgentEvent) -> Option<String> {
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
        AgentEvent::AutoCompactionStart { reason } => Some(format!("自动压缩开始: {reason}")),
        AgentEvent::AutoCompactionEnd {
            aborted,
            will_retry,
            error_message,
            ..
        } => Some(format!(
            "自动压缩结束: aborted={aborted} retry={will_retry}{}",
            error_message
                .as_ref()
                .map_or(String::new(), |err| format!(" error={err}"))
        )),
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
