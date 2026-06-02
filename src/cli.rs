//! CLI argument parsing using Clap.

use std::collections::HashSet;

use clap::{Parser, Subcommand};

#[derive(Debug)]
pub struct ParsedCli {
    pub cli: Cli,
}

pub fn parse_args(raw_args: Vec<String>) -> Result<ParsedCli, clap::Error> {
    if raw_args.is_empty() {
        let cli = Cli::try_parse_from(["pi"])?;
        return Ok(ParsedCli { cli });
    }

    let cli = Cli::try_parse_from(raw_args)?;
    Ok(ParsedCli { cli })
}

/// Pi - AI coding agent CLI
#[derive(Parser, Debug)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally boolean
#[command(name = "pi")]
#[command(version, about, long_about = None, disable_version_flag = true)]
#[command(after_help = "Examples:
  pi \"explain this code\"              Start new session with message
  pi @file.rs \"review this\"           Include file in context
  pi -c                                Continue previous session
  pi -r                                Resume from session picker
  pi -p \"what is 2+2\"                 Print mode (non-interactive)
  pi --model claude-opus-4 \"help\"     Use specific model
")]
pub struct Cli {
    // === Help & Version ===
    /// Print version information
    #[arg(short = 'v', long)]
    pub version: bool,

    // === Model Configuration ===
    /// LLM provider (e.g., anthropic, openai, google).
    /// Run --list-providers for canonical IDs + aliases.
    #[arg(long, env = "PI_PROVIDER")]
    pub provider: Option<String>,

    /// Model ID (e.g., claude-opus-4, gpt-4o)
    #[arg(long, env = "PI_MODEL")]
    pub model: Option<String>,

    /// API key (overrides environment variable)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Model patterns for Ctrl+P cycling (comma-separated, supports globs)
    #[arg(long)]
    pub models: Option<String>,

    /// HTTP request timeout in seconds for provider API calls.
    ///
    /// Bounds connect + request + first-response-header latency for each
    /// provider request. `0` disables the timeout entirely (unbounded).
    ///
    /// When unset, the default is provider-aware: 60s for cloud providers and
    /// 600s (10 minutes) for local providers (Ollama, LM Studio) where the
    /// first request can block while the model loads into memory. Raise this if
    /// a local model's cold start exceeds the default. Equivalent to the
    /// `PI_HTTP_REQUEST_TIMEOUT_SECS` env var and the `requestTimeoutSecs`
    /// setting. See pi_agent_rust#90.
    #[arg(long, value_name = "SECONDS", env = "PI_HTTP_REQUEST_TIMEOUT_SECS")]
    pub request_timeout: Option<u64>,

    // === Thinking/Reasoning ===
    /// Extended thinking level
    #[arg(long, value_parser = ["off", "minimal", "low", "medium", "high", "xhigh"])]
    pub thinking: Option<String>,

    // === System Prompt ===
    /// Override system prompt
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Append to system prompt (text or file path)
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    // === Session Management ===
    /// Continue previous session
    #[arg(short = 'c', long)]
    pub r#continue: bool,

    /// Select session from picker UI
    #[arg(short = 'r', long)]
    pub resume: bool,

    /// Use specific session file path
    #[arg(long)]
    pub session: Option<String>,

    /// Directory for session storage/lookup
    #[arg(long)]
    pub session_dir: Option<String>,

    /// Don't save session (ephemeral)
    #[arg(long)]
    pub no_session: bool,

    /// Session durability mode: strict, balanced, or throughput
    #[arg(
        long,
        value_parser = ["strict", "balanced", "throughput"]
    )]
    pub session_durability: Option<String>,

    /// Skip startup migrations for legacy config/session/layout paths
    #[arg(long)]
    pub no_migrations: bool,

    /// Disable terminal mouse capture in the interactive TUI.
    ///
    /// Pi normally captures all mouse motion to enable in-app wheel scrolling.
    /// On Windows / CMD.exe / Windows Terminal that capture blocks the
    /// terminal-native click-to-select / right-click-paste / Shift-Insert
    /// behaviour, making it effectively impossible to copy out the OAuth
    /// authorization URL (which is ~600 characters). Setting this flag (or
    /// `disable_mouse_capture: true` in settings, or `PI_NO_MOUSE_CAPTURE=1`)
    /// turns the capture off so terminal-native copy/paste keeps working.
    /// In-app mouse wheel scrolling is sacrificed; users can still scroll
    /// with Page Up/Down or arrow keys.
    ///
    /// Note: the env-var path is intentionally read in `run_interactive`
    /// (not via `#[arg(env = "...")]` here) so the truthiness semantics
    /// stay "only `=1` is truthy", matching how `PI_HARDWARE_CURSOR`
    /// behaves and avoiding clap's bool-env ambiguity where `=0` /
    /// `=false` may otherwise set the flag to true.
    #[arg(long)]
    pub no_mouse_capture: bool,

    // === Mode & Output ===
    /// Output mode for print mode (text, json, rpc)
    #[arg(long, value_parser = ["text", "json", "rpc"])]
    pub mode: Option<String>,

    /// Non-interactive mode (process & exit)
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Start in RPC mode (alias for --mode rpc)
    #[arg(long, conflicts_with_all = ["mode", "print"])]
    pub rpc: bool,

    /// Start in ACP (Agent Client Protocol) mode for Zed editor integration.
    /// Reads JSON-RPC 2.0 requests from stdin and writes responses to stdout.
    #[arg(long)]
    pub acp: bool,

    /// Force verbose startup
    #[arg(long)]
    pub verbose: bool,

    // === Tools ===
    /// Disable all built-in tools
    #[arg(long)]
    pub no_tools: bool,

    /// Specific tools to enable (comma-separated: read,write,edit,bash,grep,find,ls,hashline_edit)
    #[arg(
        long,
        default_value = "read,bash,edit,write,grep,find,ls,hashline_edit"
    )]
    pub tools: String,

    // === Skills ===
    /// Load skill file/directory (can use multiple times)
    #[arg(long, action = clap::ArgAction::Append)]
    pub skill: Vec<String>,

    /// Disable skill discovery
    #[arg(long)]
    pub no_skills: bool,

    // === Prompt Templates ===
    /// Load prompt template file/directory (can use multiple times)
    #[arg(long, action = clap::ArgAction::Append)]
    pub prompt_template: Vec<String>,

    /// Disable prompt template discovery
    #[arg(long)]
    pub no_prompt_templates: bool,

    // === Themes ===
    /// Select active theme (built-in name, discovered theme name, or theme JSON path)
    #[arg(long)]
    pub theme: Option<String>,

    /// Add theme file/directory to discovery (can use multiple times)
    #[arg(long = "theme-path", action = clap::ArgAction::Append)]
    pub theme_path: Vec<String>,

    /// Disable theme discovery
    #[arg(long)]
    pub no_themes: bool,

    // === System prompt modifiers ===
    /// Hide the current working directory from the system prompt.
    #[arg(long, env = "PI_HIDE_CWD_IN_PROMPT")]
    pub hide_cwd_in_prompt: bool,

    /// Maximum tool-call iterations per agent turn before stopping.
    /// Default: 50. Clamped to [1, 1000]; values outside the range fall back
    /// to 50 with a warning. Pairs with the iteration-aware-handoff protocol —
    /// at 80% of the cap, a one-shot steering message is injected so the agent
    /// can begin a graceful handoff rather than being silently killed at the
    /// ceiling. Override per-invocation via this flag, or globally via the
    /// `PI_MAX_TOOL_ITERATIONS` env var (read at agent start; invalid values
    /// fall back to the default with a warning, never abort startup).
    //
    // NOTE: `env =` is intentionally NOT set here. Clap's env wiring is strict
    // (an unparseable value aborts startup with a clap error), which would
    // defeat the lenient resolver semantics expected for this knob. The env
    // var is read inside `resolve_max_tool_iterations` instead, where bad
    // values warn-and-fall-back rather than fail the run.
    #[arg(long, value_name = "N")]
    pub max_tool_iterations: Option<usize>,

    // === Export & Listing ===
    /// Export session file to HTML
    #[arg(long)]
    pub export: Option<String>,

    /// List available models (optional fuzzy search pattern)
    #[arg(long)]
    #[allow(clippy::option_option)]
    // This is intentional: None = not set, Some(None) = set without value, Some(Some(x)) = set with value
    pub list_models: Option<Option<String>>,

    /// List all supported providers with aliases and auth env keys
    #[arg(long)]
    pub list_providers: bool,

    /// Fetch the live model catalog from a provider's `/v1/models` endpoint
    /// (OpenAI-compatible providers only). Falls back to the static registry
    /// when the live call fails. Results are cached in-memory for 5 minutes;
    /// set `PI_DISABLE_MODEL_CACHE=1` to bypass.
    #[arg(long, value_name = "PROVIDER")]
    pub fetch_models: Option<String>,

    /// When used with `--fetch-models`, ignore any cached entry and force a
    /// fresh network call (still falls back to the static registry on error).
    #[arg(long, requires = "fetch_models")]
    pub refresh_models: bool,

    // === Subcommands ===
    #[command(subcommand)]
    pub command: Option<Commands>,

    // === Positional Arguments ===
    /// Messages and @file references
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,
}

/// Package management subcommands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Install skill/prompt/theme package from source
    Install {
        /// Package source (npm:pkg, git:url, or local path)
        source: String,
        /// Install locally (project) instead of globally
        #[arg(short = 'l', long)]
        local: bool,
    },

    /// Remove package from settings
    Remove {
        /// Package source to remove
        source: String,
        /// Remove from local (project) settings
        #[arg(short = 'l', long)]
        local: bool,
    },

    /// Update packages
    Update {
        /// Specific source to update (or all if omitted)
        source: Option<String>,
    },

    /// Preview the semantic context bundle Pi would use for a task
    #[command(name = "context-preview")]
    ContextPreview {
        /// Output format: text (default) or json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
        /// Bead ID to anchor the preview around
        #[arg(long)]
        bead: Option<String>,
        /// Changed path to anchor related context; repeatable
        #[arg(long = "changed-path", action = clap::ArgAction::Append)]
        changed_paths: Vec<String>,
        /// Failing command to match validation context
        #[arg(long = "failing-command")]
        failing_command: Option<String>,
        /// Maximum selected bundle items
        #[arg(long, default_value_t = 24)]
        max_items: usize,
        /// Maximum selected bundle bytes
        #[arg(long, default_value_t = 32 * 1024)]
        max_bytes: u64,
        /// Task query text used to score candidate context
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },

    /// List installed packages
    List,

    /// Open configuration UI
    Config {
        /// Print configuration summary as text (non-interactive)
        #[arg(long)]
        show: bool,
        /// Print path and precedence details only
        #[arg(long)]
        paths: bool,
        /// Print configuration details as JSON
        #[arg(long)]
        json: bool,
    },

    /// Migrate session files from JSONL v1 to v2 segment format
    Migrate {
        /// Path to specific session JSONL file (or directory to migrate all)
        path: String,
        /// Dry-run: validate migration without persisting changes
        #[arg(long)]
        dry_run: bool,
    },
}

impl Cli {
    /// Get file arguments (prefixed with @)
    pub fn file_args(&self) -> Vec<&str> {
        self.args
            .iter()
            .filter(|a| a.starts_with('@'))
            .map(|a| a.strip_prefix('@').unwrap_or(a))
            .collect()
    }

    /// Get message arguments (not prefixed with @)
    pub fn message_args(&self) -> Vec<&str> {
        self.args
            .iter()
            .filter(|a| !a.starts_with('@'))
            .map(String::as_str)
            .collect()
    }

    /// Get enabled tools as a list
    pub fn enabled_tools(&self) -> Vec<&str> {
        if self.no_tools {
            vec![]
        } else {
            let mut seen = HashSet::new();
            self.tools
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .filter(|name| seen.insert(*name))
                .collect()
        }
    }
}
