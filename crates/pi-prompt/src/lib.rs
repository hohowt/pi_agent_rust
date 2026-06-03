#![forbid(unsafe_code)]

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Zh,
    En,
}

impl Language {
    #[must_use]
    pub const fn is_english(self) -> bool {
        matches!(self, Self::En)
    }

    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value)
                if value.eq_ignore_ascii_case("en")
                    || value.eq_ignore_ascii_case("eng")
                    || value.eq_ignore_ascii_case("english")
                    || value.eq_ignore_ascii_case("en-us")
                    || value.eq_ignore_ascii_case("en_us") =>
            {
                Self::En
            }
            _ => Self::Zh,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PromptCatalog {
    language: Language,
}

impl PromptCatalog {
    #[must_use]
    pub const fn new(language: Language) -> Self {
        Self { language }
    }

    #[must_use]
    pub const fn language(self) -> Language {
        self.language
    }

    #[must_use]
    pub fn default_system_prompt(&self, input: DefaultSystemPromptInput<'_>) -> String {
        render_default_system_prompt(self.language, input)
    }

    #[must_use]
    pub fn default_system_prompt_base(&self, input: DefaultSystemPromptBaseInput<'_>) -> String {
        render_default_system_prompt_base(self.language, input)
    }

    #[must_use]
    pub fn runtime_context_prompt(&self, input: RuntimeContextPromptInput<'_>) -> String {
        render_runtime_context_prompt(self.language, input)
    }

    #[must_use]
    pub fn acp_system_prompt(&self, input: AcpSystemPromptInput<'_>) -> String {
        render_acp_system_prompt(self.language, input)
    }

    #[must_use]
    pub fn skills_prompt(&self, skills: &[SkillPromptItem<'_>]) -> String {
        render_skills_prompt(self.language, skills)
    }

    #[must_use]
    pub const fn ui_text(&self) -> UiText {
        UiText {
            language: self.language,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UiText {
    language: Language,
}

#[derive(Debug, Clone, Copy)]
pub struct CodegraphSyncReportText<'a> {
    pub title: &'a str,
    pub db: &'a str,
    pub files: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub skipped: usize,
}

impl UiText {
    #[must_use]
    pub fn slash_help(&self) -> &'static str {
        if self.language.is_english() {
            EN_SLASH_HELP
        } else {
            ZH_SLASH_HELP
        }
    }

    #[must_use]
    pub fn reloading_resources(&self) -> &'static str {
        if self.language.is_english() {
            "Reloading resources..."
        } else {
            "正在重新加载资源..."
        }
    }

    #[must_use]
    pub fn cannot_index_while_processing(&self) -> &'static str {
        if self.language.is_english() {
            "Cannot index while processing"
        } else {
            "处理中，不能执行索引"
        }
    }

    #[must_use]
    pub fn codegraph_initialized(&self) -> &'static str {
        if self.language.is_english() {
            "Codegraph initialized"
        } else {
            "Codegraph 已初始化"
        }
    }

    #[must_use]
    pub fn codegraph_synced(&self) -> &'static str {
        if self.language.is_english() {
            "Codegraph synced"
        } else {
            "Codegraph 已同步"
        }
    }

    #[must_use]
    pub fn codegraph_status(&self) -> &'static str {
        if self.language.is_english() {
            "Codegraph status"
        } else {
            "Codegraph 状态"
        }
    }

    #[must_use]
    pub fn codegraph_failed(&self, err: &str) -> String {
        if self.language.is_english() {
            format!("Codegraph failed: {err}")
        } else {
            format!("Codegraph 失败: {err}")
        }
    }

    #[must_use]
    pub fn codegraph_usage(&self) -> &'static str {
        if self.language.is_english() {
            "Usage: /codegraph [init|sync|status]"
        } else {
            "用法: /codegraph [init|sync|status]"
        }
    }

    #[must_use]
    pub fn codegraph_report(&self, report: CodegraphSyncReportText<'_>) -> String {
        if self.language.is_english() {
            format!(
                "{}\nDB: {}\nFiles: {}\nIndexed: {}  unchanged: {}  removed: {}  skipped: {}",
                report.title,
                report.db,
                report.files,
                report.indexed,
                report.unchanged,
                report.removed,
                report.skipped
            )
        } else {
            format!(
                "{}\n数据库: {}\n文件: {}\n已索引: {}  未变化: {}  已移除: {}  已跳过: {}",
                report.title,
                report.db,
                report.files,
                report.indexed,
                report.unchanged,
                report.removed,
                report.skipped
            )
        }
    }

    #[must_use]
    pub fn codegraph_status_report(
        &self,
        db: &str,
        initialized: bool,
        files: Option<usize>,
    ) -> String {
        if self.language.is_english() {
            files.map_or_else(
                || format!("Codegraph status\nInitialized: {initialized}\nDB: {db}"),
                |files| {
                    format!(
                        "Codegraph status\nInitialized: {initialized}\nDB: {db}\nFiles: {files}"
                    )
                },
            )
        } else {
            files.map_or_else(
                || format!("Codegraph 状态\n已初始化: {initialized}\n数据库: {db}"),
                |files| {
                    format!("Codegraph 状态\n已初始化: {initialized}\n数据库: {db}\n文件: {files}")
                },
            )
        }
    }

    #[must_use]
    pub fn cannot_expand_template_while_processing(&self) -> &'static str {
        if self.language.is_english() {
            "Cannot expand template while processing"
        } else {
            "处理中，不能展开模板"
        }
    }

    #[must_use]
    pub fn no_prompt_templates_loaded(&self) -> &'static str {
        if self.language.is_english() {
            "No prompt templates loaded"
        } else {
            "未加载 prompt templates"
        }
    }

    #[must_use]
    pub fn available_prompt_templates(&self) -> &'static str {
        if self.language.is_english() {
            "Available prompt templates"
        } else {
            "可用 prompt templates"
        }
    }

    #[must_use]
    pub fn template_usage(&self) -> &'static str {
        if self.language.is_english() {
            "Usage: /template <name> [args]"
        } else {
            "用法: /template <name> [args]"
        }
    }

    #[must_use]
    pub fn template_not_found(&self, name: &str) -> String {
        if self.language.is_english() {
            format!("Template not found: {name}")
        } else {
            format!("未找到模板: {name}")
        }
    }

    #[must_use]
    pub fn template_empty_output(&self) -> &'static str {
        if self.language.is_english() {
            "Template expansion produced empty output"
        } else {
            "模板展开结果为空"
        }
    }

    #[must_use]
    pub fn template_no_usable_content(&self) -> &'static str {
        if self.language.is_english() {
            "Template expansion produced no usable content"
        } else {
            "模板展开结果没有可用内容"
        }
    }

    #[must_use]
    pub fn language_usage(&self) -> &'static str {
        if self.language.is_english() {
            "Usage: /language [zh|en]"
        } else {
            "用法: /language [zh|en]"
        }
    }

    #[must_use]
    pub fn language_status(&self, language: Language) -> String {
        let value = match language {
            Language::Zh => "zh",
            Language::En => "en",
        };
        if self.language.is_english() {
            format!("Language: {value}")
        } else {
            format!("语言: {value}")
        }
    }

    #[must_use]
    pub fn language_updated(&self, language: Language) -> String {
        let value = match language {
            Language::Zh => "zh",
            Language::En => "en",
        };
        if language.is_english() {
            format!("Language switched to {value}")
        } else {
            format!("语言已切换为 {value}")
        }
    }

    #[must_use]
    pub fn language_invalid(&self, value: &str) -> String {
        if self.language.is_english() {
            format!("Unsupported language: {value}. Use zh or en.")
        } else {
            format!("不支持的语言: {value}。请使用 zh 或 en。")
        }
    }

    #[must_use]
    pub fn welcome_message(&self) -> &'static str {
        if self.language.is_english() {
            "  Welcome to Pi!\n  Type a message to begin, or /help for commands.\n"
        } else {
            "  欢迎使用 Pi!\n  输入消息开始，或输入 /help 查看命令。\n"
        }
    }

    #[must_use]
    pub fn input_placeholder(&self) -> &'static str {
        if self.language.is_english() {
            "Type a message... (/help, /exit)"
        } else {
            "输入消息...（/help, /exit）"
        }
    }

    #[must_use]
    pub fn hotkeys_title(&self) -> &'static str {
        if self.language.is_english() {
            "Keyboard Shortcuts"
        } else {
            "键盘快捷键"
        }
    }

    #[must_use]
    pub fn hotkeys_config_label(&self) -> &'static str {
        if self.language.is_english() {
            "Config"
        } else {
            "配置"
        }
    }

    #[must_use]
    pub fn settings_title(&self) -> &'static str {
        if self.language.is_english() {
            "Settings"
        } else {
            "设置"
        }
    }

    #[must_use]
    pub fn no_settings_available(&self) -> &'static str {
        if self.language.is_english() {
            "No settings available."
        } else {
            "没有可用设置。"
        }
    }

    #[must_use]
    pub fn session_delete_confirm(&self) -> &'static str {
        if self.language.is_english() {
            "Delete session? Press y/n to confirm."
        } else {
            "删除 session？按 y/n 确认。"
        }
    }

    #[must_use]
    pub fn session_picker_hint(&self) -> &'static str {
        if self.language.is_english() {
            "Type: filter  Backspace: clear  ↑/↓/j/k/PgUp/PgDn: navigate  Enter: select  Ctrl+D: delete  Esc/q: cancel"
        } else {
            "输入: 过滤  Backspace: 清空  ↑/↓/j/k/PgUp/PgDn: 导航  Enter: 选择  Ctrl+D: 删除  Esc/q: 取消"
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectContextItem<'a> {
    pub path: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultSystemPromptInput<'a> {
    pub enabled_tools: &'a [&'a str],
    pub readme_path: &'a str,
    pub docs_path: &'a str,
    pub examples_path: &'a str,
    pub project_context: &'a [ProjectContextItem<'a>],
    pub skills_prompt: Option<&'a str>,
    pub date_time: &'a str,
    pub cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultSystemPromptBaseInput<'a> {
    pub enabled_tools: &'a [&'a str],
    pub readme_path: &'a str,
    pub docs_path: &'a str,
    pub examples_path: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeContextPromptInput<'a> {
    pub project_context: &'a [ProjectContextItem<'a>],
    pub skills_prompt: Option<&'a str>,
    pub date_time: &'a str,
    pub cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpSystemPromptInput<'a> {
    pub enabled_tools: &'a [&'a str],
    pub project_context: &'a [ProjectContextItem<'a>],
    pub date_time: &'a str,
    pub cwd: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct SkillPromptItem<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub location: &'a str,
}

fn render_default_system_prompt(language: Language, input: DefaultSystemPromptInput<'_>) -> String {
    let mut prompt = render_default_system_prompt_base(
        language,
        DefaultSystemPromptBaseInput {
            enabled_tools: input.enabled_tools,
            readme_path: input.readme_path,
            docs_path: input.docs_path,
            examples_path: input.examples_path,
        },
    );
    prompt.push_str(&render_runtime_context_prompt(
        language,
        RuntimeContextPromptInput {
            project_context: input.project_context,
            skills_prompt: input.skills_prompt,
            date_time: input.date_time,
            cwd: input.cwd,
        },
    ));
    prompt
}

fn render_default_system_prompt_base(
    language: Language,
    input: DefaultSystemPromptBaseInput<'_>,
) -> String {
    let tools_list = tool_list(language, input.enabled_tools, ToolSet::Default);
    let guidelines = default_guidelines(language, input.enabled_tools);

    if language.is_english() {
        format!(
            "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n{tools_list}\n\nGuidelines:\n{guidelines}\n\nPi documentation (read only when the user asks about pi itself, its SDK, themes, skills, or TUI):\n- Main documentation: {}\n- Additional docs: {}\n- Examples: {} (custom tools, SDK)\n- When asked about: themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), adding models (docs/models.md), pi packages (docs/packages.md)\n- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing\n- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)",
            input.readme_path, input.docs_path, input.examples_path
        )
    } else {
        format!(
            "你是运行在 pi 编码代理中的资深编码助手。你通过读取文件、执行命令、编辑代码和创建文件来帮助用户。\n\n可用工具:\n{tools_list}\n\n准则:\n{guidelines}\n\nPi 文档（仅当用户询问 pi、SDK、主题、skills 或 TUI 时阅读）:\n- 主文档: {}\n- 其他文档: {}\n- 示例: {}（自定义工具、SDK）\n- 相关主题: themes 看 docs/themes.md，skills 看 docs/skills.md，prompt templates 看 docs/prompt-templates.md，TUI 组件看 docs/tui.md，快捷键看 docs/keybindings.md，SDK 看 docs/sdk.md，模型配置看 docs/models.md，packages 看 docs/packages.md\n- 处理 pi 相关任务时，先读对应文档和引用链，再实现\n- 读取 pi 的 .md 文档时要完整阅读，并跟随相关链接",
            input.readme_path, input.docs_path, input.examples_path
        )
    }
}

fn render_runtime_context_prompt(
    language: Language,
    input: RuntimeContextPromptInput<'_>,
) -> String {
    let mut prompt = String::new();
    append_project_context(&mut prompt, language, input.project_context);
    if let Some(skills_prompt) = input.skills_prompt {
        prompt.push_str(skills_prompt);
    }
    append_datetime_and_cwd(&mut prompt, language, input.date_time, input.cwd);
    prompt
}

fn render_acp_system_prompt(language: Language, input: AcpSystemPromptInput<'_>) -> String {
    let tools_list = tool_list(language, input.enabled_tools, ToolSet::Acp);
    let mut prompt = if language.is_english() {
        format!(
            "You are a helpful AI coding assistant integrated into the user's editor via ACP (Agent Client Protocol). You have access to the following tools:\n\n{tools_list}\n\nUse these tools to help the user with coding tasks. Be concise and precise. When making file changes, explain what you're doing.\n"
        )
    } else {
        format!(
            "你是通过 ACP 集成到用户编辑器中的编码助手。可用工具如下:\n\n{tools_list}\n\n使用这些工具完成编码任务。回答要简洁、准确；修改文件时说明你正在做什么。\n"
        )
    };

    for item in input.project_context {
        let _ = write!(prompt, "\n## {}\n\n{}\n\n", item.path, item.content);
    }

    append_datetime_and_cwd(&mut prompt, language, input.date_time, Some(input.cwd));
    prompt
}

fn render_skills_prompt(language: Language, skills: &[SkillPromptItem<'_>]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut lines = if language.is_english() {
        vec![
            "\n\nThe following skills provide specialized instructions for specific tasks."
                .to_string(),
            "Use the read tool to load a skill's file when the task matches its description."
                .to_string(),
            "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
            String::new(),
            "<available_skills>".to_string(),
        ]
    } else {
        vec![
            "\n\n以下 skills 为特定任务提供专用说明。".to_string(),
            "当任务匹配某个 skill 描述时，用 read 读取该 skill 文件。".to_string(),
            "如果 skill 文件引用相对路径，按 skill 目录（SKILL.md 的父目录）解析，并在工具命令中使用绝对路径。".to_string(),
            String::new(),
            "<available_skills>".to_string(),
        ]
    };

    for skill in skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(skill.location)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

#[derive(Debug, Clone, Copy)]
enum ToolSet {
    Default,
    Acp,
}

fn tool_list(language: Language, enabled_tools: &[&str], set: ToolSet) -> String {
    let descriptions = match (language, set) {
        (Language::En, ToolSet::Default) => DEFAULT_TOOL_DESCRIPTIONS_EN,
        (Language::Zh, ToolSet::Default) => DEFAULT_TOOL_DESCRIPTIONS_ZH,
        (Language::En, ToolSet::Acp) => ACP_TOOL_DESCRIPTIONS_EN,
        (Language::Zh, ToolSet::Acp) => ACP_TOOL_DESCRIPTIONS_ZH,
    };

    let tools = enabled_tools
        .iter()
        .filter_map(|tool| {
            descriptions
                .iter()
                .find(|(name, _)| name == tool)
                .map(|(_, description)| match set {
                    ToolSet::Default => format!("- {tool}: {description}"),
                    ToolSet::Acp => format!("- **{tool}**: {description}"),
                })
        })
        .collect::<Vec<_>>();

    if tools.is_empty() {
        if language.is_english() {
            "(none)".to_string()
        } else {
            "(无)".to_string()
        }
    } else {
        tools.join("\n")
    }
}

fn default_guidelines(language: Language, enabled_tools: &[&str]) -> String {
    let has_tool = |name: &str| enabled_tools.contains(&name);
    let has_bash = has_tool("bash");
    let has_edit = has_tool("edit");
    let has_write = has_tool("write");
    let has_grep = has_tool("grep");
    let has_find = has_tool("find");
    let has_ls = has_tool("ls");
    let has_read = has_tool("read");
    let has_hashline_edit = has_tool("hashline_edit");

    let mut guidelines = Vec::new();
    if language.is_english() {
        if has_bash && !has_grep && !has_find && !has_ls {
            guidelines.push("Use bash for file operations like ls, rg, find");
        } else if has_bash && (has_grep || has_find || has_ls) {
            guidelines.push(
                "Prefer grep/find/ls tools over bash for file exploration (faster, respects .gitignore)",
            );
        }
        if has_read && has_edit {
            guidelines.push(
                "Use read to examine files before editing. You must use this tool instead of cat or sed.",
            );
        }
        if has_edit {
            guidelines.push("Use edit for precise changes (old text must match exactly)");
        }
        if has_hashline_edit && has_read {
            guidelines.push(
                "For large files or complex multi-site edits, use read or grep with hashline=true to get LINE#HASH tags, then use hashline_edit for precise line-addressed edits",
            );
        }
        if has_write {
            guidelines.push("Use write only for new files or complete rewrites");
        }
        if has_edit || has_write {
            guidelines.push(
                "When summarizing your actions, output plain text directly - do NOT use cat or bash to display what you did",
            );
        }
        guidelines.push("Be concise in your responses");
        guidelines.push("Show file paths clearly when working with files");
    } else {
        if has_bash && !has_grep && !has_find && !has_ls {
            guidelines.push("使用 bash 执行 ls、rg、find 等文件操作");
        } else if has_bash && (has_grep || has_find || has_ls) {
            guidelines
                .push("探索文件时优先用 grep/find/ls 工具，而不是 bash；它们更快且遵守 .gitignore");
        }
        if has_read && has_edit {
            guidelines.push("编辑前先用 read 查看文件；不要用 cat 或 sed 代替");
        }
        if has_edit {
            guidelines.push("精确修改使用 edit，old text 必须完全匹配");
        }
        if has_hashline_edit && has_read {
            guidelines.push("大文件或多处复杂修改时，用 read 或 grep 的 hashline=true 获取 LINE#HASH，再用 hashline_edit 精确编辑");
        }
        if has_write {
            guidelines.push("write 只用于新文件或整体重写");
        }
        if has_edit || has_write {
            guidelines.push("总结改动时直接输出文本，不要用 cat 或 bash 展示你做了什么");
        }
        guidelines.push("回答保持简洁");
        guidelines.push("涉及文件时清楚写出路径");
    }

    guidelines
        .into_iter()
        .map(|guideline| format!("- {guideline}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn append_project_context(
    prompt: &mut String,
    language: Language,
    project_context: &[ProjectContextItem<'_>],
) {
    if project_context.is_empty() {
        return;
    }

    if language.is_english() {
        prompt.push_str("\n\n# Project Context\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
    } else {
        prompt.push_str("\n\n# 项目上下文\n\n");
        prompt.push_str("项目专属说明和准则:\n\n");
    }

    for item in project_context {
        let _ = write!(prompt, "## {}\n\n{}\n\n", item.path, item.content);
    }
}

fn append_datetime_and_cwd(
    prompt: &mut String,
    language: Language,
    date_time: &str,
    cwd: Option<&str>,
) {
    if language.is_english() {
        let _ = write!(prompt, "\nCurrent date and time: {date_time}");
        if let Some(cwd) = cwd {
            let _ = write!(prompt, "\nCurrent working directory: {cwd}");
        }
    } else {
        let _ = write!(prompt, "\n当前日期和时间: {date_time}");
        if let Some(cwd) = cwd {
            let _ = write!(prompt, "\n当前工作目录: {cwd}");
        }
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const DEFAULT_TOOL_DESCRIPTIONS_EN: &[(&str, &str)] = &[
    ("read", "Read file contents"),
    ("bash", "Execute bash commands (ls, grep, find, etc.)"),
    (
        "edit",
        "Make surgical edits to files (find exact text and replace)",
    ),
    ("write", "Create or overwrite files"),
    (
        "grep",
        "Search file contents for patterns (respects .gitignore, supports hashline=true for use with hashline_edit)",
    ),
    ("find", "Find files by glob pattern (respects .gitignore)"),
    ("ls", "List directory contents"),
    (
        "hashline_edit",
        "Apply precise file edits using LINE#HASH tags from read or grep with hashline=true",
    ),
];

const DEFAULT_TOOL_DESCRIPTIONS_ZH: &[(&str, &str)] = &[
    ("read", "读取文件内容"),
    ("bash", "执行 bash 命令，如 ls、grep、find"),
    ("edit", "精确编辑文件，要求 old text 完全匹配"),
    ("write", "创建或整体重写文件"),
    (
        "grep",
        "搜索文件内容，遵守 .gitignore，可配合 hashline=true 与 hashline_edit 使用",
    ),
    ("find", "按 glob 查找文件，遵守 .gitignore"),
    ("ls", "列出目录内容"),
    (
        "hashline_edit",
        "使用 read/grep 的 LINE#HASH 标记做精确行编辑",
    ),
];

const ACP_TOOL_DESCRIPTIONS_EN: &[(&str, &str)] = &[
    ("read", "Read file contents"),
    ("bash", "Execute bash commands"),
    ("edit", "Make surgical edits to files"),
    ("write", "Write file contents"),
    ("grep", "Search file contents with regex"),
    ("find", "Find files by name pattern"),
    ("ls", "List directory contents"),
];

const ACP_TOOL_DESCRIPTIONS_ZH: &[(&str, &str)] = &[
    ("read", "读取文件内容"),
    ("bash", "执行 bash 命令"),
    ("edit", "精确编辑文件"),
    ("write", "写入文件内容"),
    ("grep", "用 regex 搜索文件内容"),
    ("find", "按名称模式查找文件"),
    ("ls", "列出目录内容"),
];

const EN_SLASH_HELP: &str = r"Available commands:
  /help, /h, /?      - Show this help message
  /login [provider]  - Login/setup credentials; without provider shows status table
  /logout [provider] - Remove stored credentials
  /clear, /cls       - Clear conversation history
  /model, /m [id|provider/id] - Open model selector or switch directly
  /thinking, /t [level] - Set thinking level (off/minimal/low/medium/high/xhigh)
  /scoped-models [patterns|clear] - Show or set scoped models for cycling
  /history, /hist    - Show input history
  /export [path]     - Export conversation to HTML
  /session, /info    - Show session info (path, tokens, cost)
  /settings          - Open settings selector
  /theme [name]      - List or switch themes (dark/light/custom)
  /resume, /r        - Pick and resume a previous session
  /new               - Start a new session
  /copy, /cp         - Copy last assistant message to clipboard
  /name <name>       - Set session display name
  /hotkeys, /keys    - Show keyboard shortcuts
  /changelog         - Show changelog entries
  /tree              - Show session branch tree summary
  /fork [id|index]   - Fork from a user message (default: last on current path)
  /compact [notes]   - Compact older context with optional instructions
  /reload            - Reload skills/prompts from disk
  /template <name> [args] - Expand a prompt template by name
  /language [zh|en] - Show or switch UI and default prompt language
  /share             - Upload session HTML to a secret GitHub gist and show URL
  /codegraph [init|sync|status] - Manage the project codegraph index
  /exit, /quit, /q   - Exit Pi

  Tips:
    • Use ↑/↓ arrows to navigate input history
    • Use Ctrl+L to open model selector
    • Use Ctrl+P to cycle scoped models
    • Use Shift+Enter (Ctrl+Enter on Windows) to insert a newline
    • Use PageUp/PageDown to scroll conversation history
    • Use Escape to cancel current input
    • Use /skill:name or /template to expand resources";

const ZH_SLASH_HELP: &str = r"可用命令:
  /help, /h, /?      - 显示帮助
  /login [provider]  - 登录或配置凭证；不填 provider 时显示状态表
  /logout [provider] - 移除已保存凭证
  /clear, /cls       - 清空对话历史
  /model, /m [id|provider/id] - 打开模型选择器或直接切换
  /thinking, /t [level] - 设置 thinking level（off/minimal/low/medium/high/xhigh）
  /scoped-models [patterns|clear] - 查看或设置 scoped models 轮换规则
  /history, /hist    - 显示输入历史
  /export [path]     - 导出对话为 HTML
  /session, /info    - 显示 session 信息（路径、tokens、费用）
  /settings          - 打开设置选择器
  /theme [name]      - 列出或切换主题（dark/light/custom）
  /resume, /r        - 选择并恢复历史 session
  /new               - 开始新 session
  /copy, /cp         - 复制上一条 assistant 消息到剪贴板
  /name <name>       - 设置 session 显示名
  /hotkeys, /keys    - 显示快捷键
  /changelog         - 显示 changelog
  /tree              - 显示 session 分支树摘要
  /fork [id|index]   - 从用户消息 fork（默认当前路径最后一条）
  /compact [notes]   - 压缩较早上下文，可附加说明
  /reload            - 从磁盘重新加载 skills/prompts
  /template <name> [args] - 按名称展开 prompt template
  /language [zh|en] - 查看或切换 UI 与默认 prompt 语言
  /share             - 上传 session HTML 到 secret GitHub gist 并显示 URL
  /codegraph [init|sync|status] - 管理项目 codegraph 索引
  /exit, /quit, /q   - 退出 Pi

  提示:
    • 使用 ↑/↓ 浏览输入历史
    • 使用 Ctrl+L 打开模型选择器
    • 使用 Ctrl+P 轮换 scoped models
    • 使用 Shift+Enter（Windows 上 Ctrl+Enter）插入换行
    • 使用 PageUp/PageDown 滚动对话历史
    • 使用 Escape 取消当前输入
    • 使用 /skill:name 或 /template 展开资源";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_parse_defaults_to_chinese() {
        assert_eq!(Language::parse(None), Language::Zh);
        assert_eq!(Language::parse(Some("zh")), Language::Zh);
        assert_eq!(Language::parse(Some("en")), Language::En);
        assert_eq!(Language::parse(Some("english")), Language::En);
    }

    #[test]
    fn default_prompt_uses_chinese_by_default_language() {
        let catalog = PromptCatalog::new(Language::Zh);
        let prompt = catalog.default_system_prompt(DefaultSystemPromptInput {
            enabled_tools: &["read", "edit"],
            readme_path: "README.md",
            docs_path: "docs",
            examples_path: "examples",
            project_context: &[],
            skills_prompt: None,
            date_time: "<TIME>",
            cwd: Some("<CWD>"),
        });

        assert!(prompt.contains("你是运行在 pi 编码代理中的资深编码助手"));
        assert!(prompt.contains("可用工具"));
        assert!(prompt.contains("当前工作目录: <CWD>"));
        assert!(prompt.contains("read"));
    }

    #[test]
    fn default_prompt_can_render_english() {
        let catalog = PromptCatalog::new(Language::En);
        let prompt = catalog.default_system_prompt(DefaultSystemPromptInput {
            enabled_tools: &["read", "edit"],
            readme_path: "README.md",
            docs_path: "docs",
            examples_path: "examples",
            project_context: &[],
            skills_prompt: None,
            date_time: "<TIME>",
            cwd: Some("<CWD>"),
        });

        assert!(prompt.contains("You are an expert coding assistant"));
        assert!(prompt.contains("Available tools"));
        assert!(prompt.contains("Current working directory: <CWD>"));
    }

    #[test]
    fn skills_prompt_escapes_xml() {
        let catalog = PromptCatalog::new(Language::Zh);
        let prompt = catalog.skills_prompt(&[SkillPromptItem {
            name: "a&b",
            description: "<desc>",
            location: "/tmp/a/SKILL.md",
        }]);

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("a&amp;b"));
        assert!(prompt.contains("&lt;desc&gt;"));
    }
}
