use async_trait::async_trait;
use futures::Stream;
use pi::agent::{Agent, AgentConfig, AgentSession};
use pi::compaction::ResolvedCompactionSettings;
use pi::interactive::resume_session_from_path_for_tui;
use pi::model::{
    AssistantMessage, ContentBlock, Message, StreamEvent, TextContent, UserContent, UserMessage,
};
use pi::provider::{Context, Provider, StreamOptions};
use pi::session::{Session, SessionMessage};
use pi::sync::Mutex;
use pi::tools::ToolRegistry;
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
