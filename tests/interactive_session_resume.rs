use async_trait::async_trait;
use futures::Stream;
use pi::agent::{Agent, AgentConfig, AgentSession};
use pi::compaction::ResolvedCompactionSettings;
use pi::interactive::{
    build_model_picker_items, expand_submitted_content_for_tui, format_agent_event,
    format_reload_status, format_session_name_status, resume_session_from_path_for_tui,
    session_picker_items,
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
