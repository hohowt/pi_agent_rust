use async_trait::async_trait;
use futures::Stream;
use pi::agent::{Agent, AgentConfig, AgentSession};
use pi::auth::{AuthCredential, AuthStorage};
use pi::compaction::ResolvedCompactionSettings;
use pi::config::Config;
use pi::interactive::{
    build_model_picker_items, changelog_picker_items, expand_submitted_content_for_tui,
    export_current_session_for_tui, fork_from_user_message_for_tui, fork_picker_items,
    format_agent_event, format_changelog_entry, format_compaction_status, format_reload_status,
    format_session_name_status, last_assistant_text_for_tui, logout_picker_items,
    logout_provider_for_tui, resume_session_from_path_for_tui, select_tree_leaf_for_tui,
    session_picker_items, share_current_session_for_tui, startup_changelog_lines,
    startup_oauth_hint_lines, tree_picker_items,
};
use pi::model::{
    AssistantMessage, ContentBlock, Message, StreamEvent, TextContent, UserContent, UserMessage,
};
use pi::models::ModelEntry;
use pi::provider::{Context, InputType, Model, ModelCost, Provider, StreamOptions};
use pi::resources::ResourceLoader;
use pi::session::{Session, SessionMessage};
use pi::session_index::SessionMeta;
use pi::sync::Mutex;
use pi::tools::{ToolOutput, ToolRegistry};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(future)
}

#[derive(Debug)]
struct NoopProvider;

#[async_trait]
impl Provider for NoopProvider {
    fn name(&self) -> &'static str {
        "test-provider"
    }

    fn api(&self) -> &'static str {
        "test-api"
    }

    fn model_id(&self) -> &'static str {
        "test-model"
    }

    async fn stream(
        &self,
        _context: &Context<'_>,
        _options: &StreamOptions,
    ) -> pi::PiResult<Pin<Box<dyn Stream<Item = pi::PiResult<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

fn empty_agent_session(cwd: &Path) -> AgentSession {
    let agent = Agent::new(
        Arc::new(NoopProvider),
        ToolRegistry::new(&[], cwd, None),
        AgentConfig::default(),
    );
    AgentSession::new(
        agent,
        Arc::new(Mutex::new(Session::in_memory())),
        false,
        ResolvedCompactionSettings::default(),
    )
}

fn model_entry(provider: &str, id: &str, reasoning: bool, api_key: Option<&str>) -> ModelEntry {
    ModelEntry {
        model: Model {
            id: id.to_string(),
            name: format!("{provider} {id}"),
            api: "openai-chat".to_string(),
            provider: provider.to_string(),
            base_url: String::new(),
            reasoning,
            input: vec![InputType::Text, InputType::Image],
            cost: ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 128_000,
            max_tokens: 4096,
            headers: HashMap::new(),
        },
        api_key: api_key.map(ToString::to_string),
        headers: HashMap::new(),
        auth_header: true,
        compat: None,
    }
}

#[test]
fn picker_selected_resume_session_replaces_visible_and_agent_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut saved = Session::create_with_dir(Some(temp.path().to_path_buf()));
    saved.append_message(SessionMessage::User {
        content: UserContent::Text("restored user prompt".to_string()),
        timestamp: Some(1),
    });
    saved.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new(
                "restored assistant answer".to_string(),
            ))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 2,
            ..Default::default()
        },
    });
    run_async(saved.save()).expect("save session");
    let path = saved.path.clone().expect("saved session path");

    let mut agent = empty_agent_session(temp.path());
    agent
        .agent
        .replace_messages(vec![Message::User(UserMessage {
            content: UserContent::Text("old prompt".to_string()),
            timestamp: 0,
        })]);

    let action = run_async(resume_session_from_path_for_tui(
        &mut agent,
        path.to_string_lossy().as_ref(),
    ))
    .expect("resume selected session");

    let pi_tui::ChatAction::Many(actions) = action else {
        panic!("expected batched replace/status action");
    };
    let pi_tui::ChatAction::ReplaceLines(lines) = &actions[0] else {
        panic!("expected visible line replacement");
    };
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].role(), "You");
    assert_eq!(lines[0].text(), "restored user prompt");
    assert_eq!(lines[1].role(), "Assistant");
    assert_eq!(lines[1].text(), "restored assistant answer");

    let pi_tui::ChatAction::PushLine(status) = &actions[1] else {
        panic!("expected resume status line");
    };
    assert_eq!(status.role(), "Status");
    assert!(status.text().contains("已恢复会话"));

    let messages = agent.agent.messages();
    assert_eq!(messages.len(), 2);
    match &messages[0] {
        Message::User(user) => {
            assert!(
                matches!(&user.content, UserContent::Text(text) if text == "restored user prompt")
            );
        }
        other => panic!("expected restored user message, got {other:?}"),
    }
}

#[test]
fn model_picker_items_include_current_marker_and_details_without_changing_submit_value() {
    let current = model_entry("anthropic", "claude-sonnet-4-5", true, Some("key"));
    let other = model_entry("openai", "gpt-4o", false, None);

    let items = build_model_picker_items(&current, &[current.clone(), other]);

    assert_eq!(items[0].value, "anthropic/claude-sonnet-4-5");
    assert_eq!(items[0].group.as_deref(), Some("anthropic provider"));
    assert!(items[0].label.starts_with("* claude-sonnet-4-5"));
    assert!(items[0].description.contains("auth: configured"));
    assert!(items[0].description.contains("input: text,image"));
    assert!(
        items[0]
            .description
            .contains("thinking: off,minimal,low,medium,high")
    );
    assert_eq!(items[1].value, "openai/gpt-4o");
    assert_eq!(items[1].group.as_deref(), Some("openai provider"));
    assert!(items[1].label.starts_with("  gpt-4o"));
    assert!(items[1].description.contains("auth: missing"));
    assert_eq!(
        items[1].disabled_reason.as_deref(),
        Some("missing credentials")
    );
    assert!(items[1].description.contains("thinking: off"));
}

#[test]
fn model_picker_items_group_rows_by_provider() {
    let current = model_entry("openai", "gpt-5", true, Some("key"));
    let other = model_entry("anthropic", "claude-sonnet-4", true, Some("key"));

    let items = build_model_picker_items(&current, &[other, current.clone()]);

    assert_eq!(items[0].group.as_deref(), Some("anthropic provider"));
    assert_eq!(items[1].group.as_deref(), Some("openai provider"));
    assert!(items[1].label.starts_with("* gpt-5"));
}

#[test]
fn logout_picker_items_submit_provider_values() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut auth = AuthStorage::load(temp.path().join("auth.json")).expect("load auth");
    auth.set(
        "openai",
        AuthCredential::ApiKey {
            key: "openai-key".to_string(),
        },
    );
    auth.set(
        "anthropic",
        AuthCredential::BearerToken {
            token: "anthropic-token".to_string(),
        },
    );

    let items = logout_picker_items(&auth);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "anthropic");
    assert_eq!(items[0].value, "anthropic");
    assert_eq!(items[0].description, "bearer token");
    assert_eq!(items[1].label, "openai");
    assert_eq!(items[1].value, "openai");
    assert_eq!(items[1].description, "api key");
}

#[test]
fn logout_provider_removes_saved_credential_and_refreshes_models() {
    let temp = tempfile::tempdir().expect("tempdir");
    let auth_path = temp.path().join("auth.json");
    let mut auth = AuthStorage::load(auth_path.clone()).expect("load auth");
    auth.set(
        "openai",
        AuthCredential::ApiKey {
            key: "openai-key".to_string(),
        },
    );
    auth.set(
        "anthropic",
        AuthCredential::ApiKey {
            key: "anthropic-key".to_string(),
        },
    );
    auth.save().expect("save auth");
    let mut agent = empty_agent_session(temp.path());

    let (action, available_models) = run_async(logout_provider_for_tui(
        &mut agent,
        auth,
        temp.path().join("models.json"),
        "openai",
    ))
    .expect("logout provider");

    let pi_tui::ChatAction::PushLine(status) = action else {
        panic!("expected logout status");
    };
    assert!(status.text().contains("已移除凭据: openai"));
    assert!(!available_models.is_empty());
    let reloaded = AuthStorage::load(auth_path).expect("reload auth");
    assert!(!reloaded.has_stored_credential("openai"));
    assert!(reloaded.has_stored_credential("anthropic"));
}

#[test]
fn session_picker_items_include_scope_group_and_metadata() {
    let items = session_picker_items(vec![SessionMeta {
        path: "/tmp/project/session-a.jsonl".to_string(),
        id: "session-a".to_string(),
        cwd: "/tmp/project".to_string(),
        timestamp: "2026-06-11T00:00:00Z".to_string(),
        message_count: 7,
        last_modified_ms: 123,
        size_bytes: 2049,
        name: Some("release notes".to_string()),
    }]);

    assert_eq!(items[0].label, "release notes");
    assert_eq!(items[0].value, "/tmp/project/session-a.jsonl");
    assert_eq!(items[0].group.as_deref(), Some("/tmp/project"));
    assert!(items[0].description.contains("7 messages"));
    assert!(items[0].description.contains("3 KiB"));
    assert!(items[0].description.contains("session-a.jsonl"));
}

#[test]
fn tree_picker_items_list_leaves_with_current_marker_and_preview() {
    let mut session = Session::in_memory();
    let root = session.append_message(SessionMessage::User {
        content: UserContent::Text("root prompt".to_string()),
        timestamp: Some(1),
    });
    let first_leaf = session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("first answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 2,
            ..Default::default()
        },
    });
    assert!(session.create_branch_from(&root));
    let second_leaf = session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("second answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 3,
            ..Default::default()
        },
    });

    let items = tree_picker_items(&session);

    assert_eq!(items.len(), 2);
    let current = items
        .iter()
        .find(|item| item.value == second_leaf)
        .expect("current branch item");
    assert!(current.label.starts_with("* "));
    assert_eq!(current.disabled_reason.as_deref(), Some("current branch"));
    let other = items
        .iter()
        .find(|item| item.value == first_leaf)
        .expect("other branch item");
    assert!(other.label.starts_with("  "));
    assert!(other.description.contains("2 messages"));
    assert!(other.description.contains("root prompt"));
}

#[test]
fn selecting_tree_leaf_replaces_visible_and_agent_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = SessionMessage::User {
        content: UserContent::Text("root prompt".to_string()),
        timestamp: Some(1),
    };
    let mut session = Session::in_memory();
    let root_id = session.append_message(root);
    let first_leaf = session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("first answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 2,
            ..Default::default()
        },
    });
    assert!(session.create_branch_from(&root_id));
    session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("second answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 3,
            ..Default::default()
        },
    });

    let mut agent = empty_agent_session(temp.path());
    run_async(async {
        let cx = pi::agent_cx::AgentCx::for_request();
        let mut active = agent.session.lock(cx.cx()).await.expect("session lock");
        *active = session;
    });

    let action =
        run_async(select_tree_leaf_for_tui(&mut agent, &first_leaf)).expect("select tree leaf");

    let pi_tui::ChatAction::Many(actions) = action else {
        panic!("expected replace/status actions");
    };
    let pi_tui::ChatAction::ReplaceLines(lines) = &actions[0] else {
        panic!("expected visible line replacement");
    };
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text(), "root prompt");
    assert_eq!(lines[1].text(), "first answer");
    let messages = agent.agent.messages();
    assert_eq!(messages.len(), 2);
    match &messages[1] {
        Message::Assistant(assistant) => {
            let ContentBlock::Text(text) = &assistant.content[0] else {
                panic!("expected text content");
            };
            assert_eq!(text.text, "first answer");
        }
        other => panic!("expected assistant message, got {other:?}"),
    }
}

#[test]
fn fork_picker_items_list_current_path_user_messages() {
    let mut session = Session::in_memory();
    let first = session.append_message(SessionMessage::User {
        content: UserContent::Text("first prompt".to_string()),
        timestamp: Some(1),
    });
    session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 2,
            ..Default::default()
        },
    });
    let second = session.append_message(SessionMessage::User {
        content: UserContent::Text("second prompt".to_string()),
        timestamp: Some(3),
    });

    let items = fork_picker_items(&session);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].value, first);
    assert!(items[0].label.contains("first prompt"));
    assert_eq!(items[1].value, second);
    assert!(items[1].label.contains("second prompt"));
}

#[test]
fn fork_from_user_message_prefills_editor_and_installs_new_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut session = Session::in_memory();
    session.append_message(SessionMessage::User {
        content: UserContent::Text("root prompt".to_string()),
        timestamp: Some(1),
    });
    session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("root answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 2,
            ..Default::default()
        },
    });
    let fork_target = session.append_message(SessionMessage::User {
        content: UserContent::Text("redo this".to_string()),
        timestamp: Some(3),
    });
    session.append_message(SessionMessage::Assistant {
        message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent::new("old answer"))],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            timestamp: 4,
            ..Default::default()
        },
    });

    let mut agent = empty_agent_session(temp.path());
    run_async(async {
        let cx = pi::agent_cx::AgentCx::for_request();
        let mut active = agent.session.lock(cx.cx()).await.expect("session lock");
        *active = session;
    });

    let action = run_async(fork_from_user_message_for_tui(&mut agent, &fork_target))
        .expect("fork from user message");

    let pi_tui::ChatAction::Many(actions) = action else {
        panic!("expected fork actions");
    };
    assert!(matches!(actions[0], pi_tui::ChatAction::ReplaceLines(_)));
    let pi_tui::ChatAction::SetEditorText(text) = &actions[1] else {
        panic!("expected editor prefill");
    };
    assert_eq!(text, "redo this");
    let messages = agent.agent.messages();
    assert_eq!(messages.len(), 2);
    match &messages[1] {
        Message::Assistant(assistant) => {
            let ContentBlock::Text(text) = &assistant.content[0] else {
                panic!("expected text content");
            };
            assert_eq!(text.text, "root answer");
        }
        other => panic!("expected assistant message, got {other:?}"),
    }
}

#[test]
fn reload_status_reports_resource_model_counts_and_diagnostics() {
    let resources = ResourceLoader::empty(true);

    let status = format_reload_status(&resources, 2, Some("bad models.json"), Some("old/model"));

    assert!(status.contains("资源已重新加载"));
    assert!(status.contains("- skills: 0"));
    assert!(status.contains("- prompts: 0"));
    assert!(status.contains("- themes: 0"));
    assert!(status.contains("- diagnostics: 0"));
    assert!(status.contains("- models: 2"));
    assert!(status.contains("- models.json: bad models.json"));
    assert!(status.contains("- current model retained: old/model"));
}

#[test]
fn session_name_status_includes_visible_name_and_entry_id() {
    let status = format_session_name_status("release notes", "entry-42");

    assert!(status.contains("会话已命名"));
    assert!(status.contains("名称: release notes"));
    assert!(status.contains("记录: entry-42"));
}

#[test]
fn live_tool_progress_events_render_status_lines() {
    let start = pi::agent::AgentEvent::ToolExecutionStart {
        tool_call_id: "call-1".to_string(),
        tool_name: "read".to_string(),
        args: json!({ "path": "Cargo.toml" }),
    };
    assert_eq!(
        format_agent_event(&start).as_deref(),
        Some(r#"tool: read 开始 {"path":"Cargo.toml"}"#)
    );

    let update = pi::agent::AgentEvent::ToolExecutionUpdate {
        tool_call_id: "call-1".to_string(),
        tool_name: "read".to_string(),
        args: json!({ "path": "Cargo.toml" }),
        partial_result: ToolOutput {
            content: vec![ContentBlock::Text(TextContent::new("loaded 32 lines"))],
            details: None,
            is_error: false,
        },
    };
    assert_eq!(
        format_agent_event(&update).as_deref(),
        Some("tool: read 更新 loaded 32 lines")
    );

    let end = pi::agent::AgentEvent::ToolExecutionEnd {
        tool_call_id: "call-1".to_string(),
        tool_name: "read".to_string(),
        result: ToolOutput {
            content: Vec::new(),
            details: Some(json!({ "bytes": 1234 })),
            is_error: false,
        },
        is_error: false,
    };
    assert_eq!(
        format_agent_event(&end).as_deref(),
        Some(r#"tool: read 完成 {"bytes":1234}"#)
    );
}

#[test]
fn compaction_events_render_progress_summary_and_errors() {
    let start = pi::agent::AgentEvent::AutoCompactionStart {
        reason: "threshold".to_string(),
    };
    assert_eq!(
        format_compaction_status(&start).as_deref(),
        Some("上下文压缩开始: threshold")
    );
    assert_eq!(format_agent_event(&start), format_compaction_status(&start));

    let end = pi::agent::AgentEvent::AutoCompactionEnd {
        result: Some(json!({
            "summary": "Earlier work was summarized.",
            "firstKeptEntryId": "entry-12",
            "tokensBefore": 42000,
            "details": {
                "readFiles": ["Cargo.toml", "src/main.rs"],
                "modifiedFiles": ["src/main.rs"]
            }
        })),
        aborted: false,
        will_retry: false,
        error_message: None,
    };
    let status = format_compaction_status(&end).expect("compaction status");
    assert!(status.contains("上下文压缩完成"));
    assert!(status.contains("tokens before: 42000"));
    assert!(status.contains("first kept entry: entry-12"));
    assert!(status.contains("files: 2 read, 1 modified"));
    assert!(status.contains("Earlier work was summarized."));
    assert_eq!(format_agent_event(&end), format_compaction_status(&end));

    let failed = pi::agent::AgentEvent::AutoCompactionEnd {
        result: None,
        aborted: false,
        will_retry: false,
        error_message: Some("Missing API key".to_string()),
    };
    assert_eq!(
        format_compaction_status(&failed).as_deref(),
        Some("上下文压缩失败: aborted=false retry=false error=Missing API key")
    );
}

#[test]
fn last_assistant_text_for_tui_returns_latest_text_message() {
    let temp = tempfile::tempdir().expect("tempdir");
    let agent = empty_agent_session(temp.path());
    run_async(async {
        let cx = pi::agent_cx::AgentCx::for_request();
        let mut session = agent.session.lock(cx.cx()).await.expect("session lock");
        session.append_message(SessionMessage::Assistant {
            message: AssistantMessage {
                content: vec![ContentBlock::Text(TextContent::new("older answer"))],
                api: "test-api".to_string(),
                provider: "test-provider".to_string(),
                model: "test-model".to_string(),
                timestamp: 1,
                ..Default::default()
            },
        });
        session.append_message(SessionMessage::Assistant {
            message: AssistantMessage {
                content: vec![
                    ContentBlock::Text(TextContent::new("latest")),
                    ContentBlock::Text(TextContent::new("answer")),
                ],
                api: "test-api".to_string(),
                provider: "test-provider".to_string(),
                model: "test-model".to_string(),
                timestamp: 2,
                ..Default::default()
            },
        });
    });

    let text = run_async(last_assistant_text_for_tui(&agent)).expect("last assistant");

    assert_eq!(text.as_deref(), Some("latest\nanswer"));
}

#[test]
fn export_current_session_for_tui_writes_html_and_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let agent = empty_agent_session(temp.path());
    run_async(async {
        let cx = pi::agent_cx::AgentCx::for_request();
        let mut session = agent.session.lock(cx.cx()).await.expect("session lock");
        session.append_message(SessionMessage::User {
            content: UserContent::Text("export this".to_string()),
            timestamp: Some(1),
        });
        session.append_message(SessionMessage::Assistant {
            message: AssistantMessage {
                content: vec![ContentBlock::Text(TextContent::new("exported answer"))],
                api: "test-api".to_string(),
                provider: "test-provider".to_string(),
                model: "test-model".to_string(),
                timestamp: 2,
                ..Default::default()
            },
        });
    });

    let html = run_async(export_current_session_for_tui(
        &agent,
        temp.path(),
        "chat.html",
    ))
    .expect("html export");
    let html_body = std::fs::read_to_string(&html.path).expect("read html export");
    assert_eq!(html.format, "HTML");
    assert!(html_body.contains("export this"));
    assert!(html_body.contains("exported answer"));

    let json = run_async(export_current_session_for_tui(
        &agent,
        temp.path(),
        "chat.json",
    ))
    .expect("json export");
    let json_body = std::fs::read_to_string(&json.path).expect("read json export");
    assert_eq!(json.format, "JSON");
    assert!(json_body.contains("export this"));
    assert!(json_body.contains("exported answer"));
}

#[test]
fn share_current_session_for_tui_uses_gh_and_returns_viewer_url() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mock_bin = temp.path().join("bin");
    std::fs::create_dir_all(&mock_bin).expect("create mock bin");
    let gh_path = mock_bin.join("gh");
    let args_log = mock_bin.join("args.log");
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> "{args_log}"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 0
fi
if [ "$1" = "gist" ] && [ "$2" = "create" ]; then
  echo "https://gist.github.com/testuser/share_id_789"
  exit 0
fi
exit 2
"#,
        args_log = args_log.display()
    );
    std::fs::write(&gh_path, script).expect("write mock gh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod mock gh");
    }

    let agent = empty_agent_session(temp.path());
    run_async(async {
        let cx = pi::agent_cx::AgentCx::for_request();
        let mut session = agent.session.lock(cx.cx()).await.expect("session lock");
        session.append_message(SessionMessage::User {
            content: UserContent::Text("share this".to_string()),
            timestamp: Some(1),
        });
    });

    let config = Config {
        gh_path: Some(gh_path.display().to_string()),
        ..Config::default()
    };
    let result = run_async(share_current_session_for_tui(
        &agent,
        &config,
        temp.path(),
        true,
    ))
    .expect("share session");

    assert!(result.public);
    assert_eq!(
        result.gist_url,
        "https://gist.github.com/testuser/share_id_789"
    );
    assert_eq!(
        result.viewer_url,
        "https://buildwithpi.ai/session/#share_id_789"
    );
    let args = std::fs::read_to_string(args_log).expect("read gh args");
    assert!(args.contains("auth status"));
    assert!(args.contains("gist create"));
    assert!(args.contains("--public=true"));
    assert!(args.contains("--desc"));
}

#[test]
fn share_current_session_for_tui_reports_missing_gh() {
    let temp = tempfile::tempdir().expect("tempdir");
    let agent = empty_agent_session(temp.path());
    let config = Config {
        gh_path: Some(temp.path().join("missing-gh").display().to_string()),
        ..Config::default()
    };

    let err = run_async(share_current_session_for_tui(
        &agent,
        &config,
        temp.path(),
        false,
    ))
    .expect_err("missing gh should fail");

    assert!(err.to_string().contains("cli.github.com"));
}

#[test]
fn changelog_picker_lists_versions_and_formats_selected_entry() {
    let items = changelog_picker_items();

    assert!(!items.is_empty());
    assert!(items[0].label.contains("Unreleased"));
    assert_eq!(items[0].value, "0");
    assert!(!items[0].description.is_empty());

    let entry = format_changelog_entry("0").expect("first changelog entry");
    assert!(entry.contains("Unreleased"));
    assert!(entry.contains("Features") || entry.contains("Bug Fixes"));
}

#[test]
fn startup_changelog_respects_quiet_collapse_and_seen_version() {
    let lines = startup_changelog_lines(&Config::default());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].role(), "Status");
    assert!(lines[0].text().contains("Changelog:"));

    let mut config = Config {
        quiet_startup: Some(true),
        ..Config::default()
    };
    assert!(startup_changelog_lines(&config).is_empty());

    config.quiet_startup = None;
    config.collapse_changelog = Some(true);
    assert!(startup_changelog_lines(&config).is_empty());

    let latest_title = changelog_picker_items()
        .into_iter()
        .next()
        .expect("latest changelog item")
        .label;
    config.collapse_changelog = None;
    config.last_changelog_version = Some(latest_title);
    assert!(startup_changelog_lines(&config).is_empty());
}

#[test]
fn startup_oauth_hint_reports_missing_current_model_credentials() {
    let temp = tempfile::tempdir().expect("tempdir");
    let auth = AuthStorage::load(temp.path().join("auth.json")).expect("load auth");
    let current = model_entry("openai", "gpt-5", true, None);
    let configured = model_entry("anthropic", "claude-sonnet-4", true, Some("key"));

    let lines = startup_oauth_hint_lines(&Config::default(), &current, &[configured], &auth);

    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].role(), "Status");
    assert!(lines[0].text().contains("openai 当前缺少凭据"));
    assert!(lines[0].text().contains("OPENAI_API_KEY"));
    assert!(lines[0].text().contains("pi --provider openai"));
}

#[test]
fn startup_oauth_hint_respects_quiet_startup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let auth = AuthStorage::load(temp.path().join("auth.json")).expect("load auth");
    let current = model_entry("openai", "gpt-5", true, None);
    let config = Config {
        quiet_startup: Some(true),
        ..Config::default()
    };

    assert!(startup_oauth_hint_lines(&config, &current, &[], &auth).is_empty());
}

#[test]
fn submitted_file_references_expand_into_text_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("notes.txt");
    std::fs::write(&file, "alpha beta").expect("write text file");

    let content = expand_submitted_content_for_tui(
        &format!("review @{}", file.display()),
        temp.path(),
        false,
    )
    .expect("expand file reference");

    assert_eq!(content.len(), 1);
    let ContentBlock::Text(text) = &content[0] else {
        panic!("expected text content block");
    };
    assert!(text.text.contains("<file name="));
    assert!(text.text.contains("alpha beta"));
    assert!(text.text.ends_with("review"));
}

#[test]
fn submitted_single_path_reference_expands_image_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let image = temp.path().join("tiny.png");
    let png_header: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52,
    ];
    std::fs::write(&image, png_header).expect("write png file");

    let content = expand_submitted_content_for_tui(&image.to_string_lossy(), temp.path(), false)
        .expect("expand image reference");

    assert_eq!(content.len(), 2);
    assert!(matches!(&content[0], ContentBlock::Text(text) if text.text.contains("<file name=")));
    assert!(matches!(&content[1], ContentBlock::Image(image) if image.mime_type == "image/png"));
}

#[test]
fn ordinary_submitted_text_stays_plain_text() {
    let temp = tempfile::tempdir().expect("tempdir");
    let content = expand_submitted_content_for_tui("explain ./missing.txt", temp.path(), false)
        .expect("plain text should not be expanded");

    assert_eq!(content.len(), 1);
    assert!(
        matches!(&content[0], ContentBlock::Text(text) if text.text == "explain ./missing.txt")
    );
}
