use super::conversation::{
    add_usage, build_content_blocks_for_input, content_blocks_to_text, last_assistant_message,
};
use super::*;

pub(super) fn build_user_message(text: String) -> ModelMessage {
    ModelMessage::User(UserMessage {
        content: UserContent::Text(text),
        timestamp: Utc::now().timestamp_millis(),
    })
}

const UI_STREAM_DELTA_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(45);
const UI_STREAM_DELTA_MAX_BUFFER_BYTES: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamDeltaKind {
    Text,
    Thinking,
}

struct UiStreamDeltaBatcher {
    sender: mpsc::Sender<PiMsg>,
    pending: std::collections::VecDeque<PiMsg>,
    pending_bytes: usize,
    flush_interval: std::time::Duration,
    max_pending_bytes: usize,
    last_flush: std::time::Instant,
    pending_tool_update: Option<PiMsg>,
    pending_tool_update_bytes: usize,
    pending_tool_update_events: usize,
    last_tool_update_flush: std::time::Instant,
}

impl UiStreamDeltaBatcher {
    fn new(sender: mpsc::Sender<PiMsg>) -> Self {
        let now = std::time::Instant::now();
        let flush_interval = UI_STREAM_DELTA_FLUSH_INTERVAL;
        Self {
            sender,
            pending: std::collections::VecDeque::new(),
            pending_bytes: 0,
            flush_interval,
            max_pending_bytes: UI_STREAM_DELTA_MAX_BUFFER_BYTES,
            // Prime the first delta flush so the UI shows immediate output.
            last_flush: now.checked_sub(flush_interval).unwrap_or(now),
            pending_tool_update: None,
            pending_tool_update_bytes: 0,
            pending_tool_update_events: 0,
            last_tool_update_flush: now,
        }
    }

    fn push_delta(&mut self, kind: StreamDeltaKind, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if let Some(last) = self.pending.back_mut() {
            match (kind, last) {
                (StreamDeltaKind::Text, PiMsg::TextDelta(text))
                | (StreamDeltaKind::Thinking, PiMsg::ThinkingDelta(text)) => {
                    text.push_str(delta);
                    self.pending_bytes += delta.len();
                    self.flush(false);
                    return;
                }
                _ => {}
            }
        }

        let msg = match kind {
            StreamDeltaKind::Text => PiMsg::TextDelta(delta.to_string()),
            StreamDeltaKind::Thinking => PiMsg::ThinkingDelta(delta.to_string()),
        };
        self.pending.push_back(msg);
        self.pending_bytes += delta.len();
        self.flush(false);
    }

    fn send_immediate(&mut self, msg: PiMsg) {
        if matches!(msg, PiMsg::ToolUpdate { .. }) {
            self.push_tool_update(msg);
            return;
        }
        self.flush_tool_update(true);
        self.pending.push_back(msg);
        self.flush(true);
    }

    fn delta_bytes_for_msg(msg: &PiMsg) -> usize {
        match msg {
            PiMsg::TextDelta(text) | PiMsg::ThinkingDelta(text) => text.len(),
            _ => 0,
        }
    }

    fn push_tool_update(&mut self, msg: PiMsg) {
        self.flush_tool_update(true);
        self.pending.push_back(msg);
        self.flush(true);
    }

    fn enqueue_pending_tool_update(&mut self) {
        if let Some(msg) = self.pending_tool_update.take() {
            self.pending.push_back(msg);
            self.pending_tool_update_bytes = 0;
            self.pending_tool_update_events = 0;
            self.last_tool_update_flush = std::time::Instant::now();
        }
    }

    fn flush_tool_update(&mut self, force_channel_flush: bool) {
        self.enqueue_pending_tool_update();
        if force_channel_flush {
            self.flush(true);
        }
    }

    fn flush(&mut self, force: bool) {
        if force {
            self.enqueue_pending_tool_update();
        }

        if self.pending.is_empty() {
            return;
        }

        if !force
            && self.pending_bytes < self.max_pending_bytes
            && self.last_flush.elapsed() < self.flush_interval
        {
            return;
        }

        let mut sent_any = false;

        while let Some(msg) = self.pending.pop_front() {
            let delta_bytes = Self::delta_bytes_for_msg(&msg);
            match self.sender.try_send(msg) {
                Ok(()) => {
                    self.pending_bytes = self.pending_bytes.saturating_sub(delta_bytes);
                    sent_any = true;
                }
                Err(err) => {
                    match err {
                        mpsc::SendError::Full(msg) => {
                            self.pending.push_front(msg);
                        }
                        mpsc::SendError::Disconnected(_) | mpsc::SendError::Cancelled(_) => {
                            self.pending.clear();
                            self.pending_bytes = 0;
                        }
                    }
                    break;
                }
            }
        }

        if sent_any {
            self.last_flush = std::time::Instant::now();
        }
    }
}

fn build_agent_done_pi_msg(messages: &[ModelMessage]) -> PiMsg {
    let last = last_assistant_message(messages);
    let mut usage = Usage::default();
    for message in messages {
        if let ModelMessage::Assistant(assistant) = message {
            add_usage(&mut usage, &assistant.usage);
        }
    }
    PiMsg::AgentDone {
        usage: Some(usage),
        stop_reason: last
            .as_ref()
            .map_or(StopReason::Stop, |msg| msg.stop_reason),
        error_message: last.as_ref().and_then(|msg| msg.error_message.clone()),
    }
}

fn dispatch_agent_event_to_ui(event: &AgentEvent, batcher: &mut UiStreamDeltaBatcher) {
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                batcher.push_delta(StreamDeltaKind::Text, delta);
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                batcher.push_delta(StreamDeltaKind::Thinking, delta);
            }
            _ => {}
        },
        AgentEvent::AgentStart { .. } => {
            batcher.send_immediate(PiMsg::AgentStart);
        }
        AgentEvent::ToolExecutionStart {
            tool_name,
            tool_call_id,
            ..
        } => {
            batcher.send_immediate(PiMsg::ToolStart {
                name: tool_name.clone(),
                tool_id: tool_call_id.clone(),
            });
        }
        AgentEvent::ToolExecutionUpdate {
            tool_name,
            tool_call_id,
            partial_result,
            ..
        } => {
            batcher.send_immediate(PiMsg::ToolUpdate {
                name: tool_name.clone(),
                tool_id: tool_call_id.clone(),
                content: partial_result.content.clone(),
                details: partial_result.details.clone(),
            });
        }
        AgentEvent::ToolExecutionEnd {
            tool_name,
            tool_call_id,
            is_error,
            ..
        } => {
            batcher.send_immediate(PiMsg::ToolEnd {
                name: tool_name.clone(),
                tool_id: tool_call_id.clone(),
                is_error: *is_error,
            });
        }
        AgentEvent::AgentEnd { messages, .. } => {
            batcher.send_immediate(build_agent_done_pi_msg(messages));
        }
        _ => {}
    }
}

async fn flush_ui_stream_batcher_with_backpressure(batcher: &StdMutex<UiStreamDeltaBatcher>) {
    let (sender, pending) = {
        let mut guard = match batcher.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.flush(true);
        if guard.pending.is_empty() {
            return;
        }
        let sender = guard.sender.clone();
        let pending = std::mem::take(&mut guard.pending);
        guard.pending_bytes = 0;
        drop(guard);
        (sender, pending)
    };

    let cx = Cx::for_request();
    for msg in pending {
        if sender.send(&cx, msg).await.is_err() {
            break;
        }
    }
}

impl PiApp {
    /// Handle custom Pi messages from the agent.
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_pi_message(&mut self, msg: PiMsg) -> Option<Cmd> {
        match msg {
            PiMsg::AgentStart => {
                self.agent_state = AgentState::Processing;
                self.current_response.clear();
                self.current_thinking.clear();
            }
            PiMsg::RunPending => {
                return self.run_next_pending();
            }
            PiMsg::EnqueuePendingInput(input) => {
                self.pending_inputs.push_back(input);
                if self.agent_state == AgentState::Idle {
                    return self.run_next_pending();
                }
            }
            PiMsg::UiShutdown => {
                // Internal signal for shutting down the async→UI bridge; should not normally reach
                // the UI event loop, but handle it defensively.
            }
            PiMsg::AutocompleteRefresh => {
                self.autocomplete.provider.refresh_background();
                return Self::autocomplete_refresh_cmd();
            }
            PiMsg::TextDelta(text) => {
                self.current_response.push_str(&text);
                // While tail-following, `view()` computes the bottom slice
                // directly, so we can skip full viewport rebuilds on every
                // token to reduce redraw jitter.
                if !self.follow_stream_tail {
                    self.refresh_conversation_viewport(false);
                }
            }
            PiMsg::ThinkingDelta(text) => {
                self.current_thinking.push_str(&text);
                if !self.follow_stream_tail {
                    self.refresh_conversation_viewport(false);
                }
            }
            PiMsg::ToolStart { name, .. } => {
                self.agent_state = AgentState::ToolRunning;
                self.current_tool = Some(name);
                self.tool_progress = Some(ToolProgress::new());
                self.pending_tool_output = None;
            }
            PiMsg::ToolUpdate {
                name,
                content,
                details,
                ..
            } => {
                // Update progress metrics from details if present.
                if let Some(ref mut progress) = self.tool_progress {
                    progress.update_from_details(details.as_ref());
                } else {
                    let mut progress = ToolProgress::new();
                    progress.update_from_details(details.as_ref());
                    self.tool_progress = Some(progress);
                }
                if let Some(output) = format_tool_output(
                    &content,
                    details.as_ref(),
                    self.config.terminal_show_images(),
                ) {
                    self.pending_tool_output = Some(format!("Tool {name} output:\n{output}"));
                }
            }
            PiMsg::ToolEnd { .. } => {
                self.agent_state = AgentState::Processing;
                self.current_tool = None;
                self.tool_progress = None;
                if let Some(output) = self.pending_tool_output.take() {
                    self.messages.push(ConversationMessage::tool(output));
                    self.scroll_to_bottom();
                }
            }
            PiMsg::AgentDone {
                usage,
                stop_reason,
                error_message,
            } => {
                // Snapshot follow-tail *before* we mutate conversation state so
                // we preserve the user's scroll intent.
                let follow_tail = self.follow_stream_tail;

                // Finalize the response: move streaming buffers into the
                // permanent message list and clear them so they are not
                // double-rendered by build_conversation_content().
                let had_response =
                    !self.current_response.is_empty() || !self.current_thinking.is_empty();
                if had_response {
                    self.messages.push(ConversationMessage::new(
                        MessageRole::Assistant,
                        std::mem::take(&mut self.current_response),
                        if self.current_thinking.is_empty() {
                            None
                        } else {
                            Some(std::mem::take(&mut self.current_thinking))
                        },
                    ));
                }
                // Defensively clear both buffers even if they were already
                // taken — this prevents a stale streaming section from
                // appearing in the next view() frame.
                self.current_response.clear();
                self.current_thinking.clear();

                // Update usage
                if let Some(ref u) = usage {
                    add_usage(&mut self.total_usage, u);
                }

                self.agent_state = AgentState::Idle;
                self.current_tool = None;
                self.abort_handle = None;

                // Refresh VCS info (may have changed during tool execution)
                self.vcs_info = super::read_vcs_info(&self.cwd);

                if stop_reason == StopReason::Aborted {
                    self.status_message = Some("Request aborted".to_string());
                } else if stop_reason == StopReason::Error {
                    let message = error_message.unwrap_or_else(|| "Request failed".to_string());
                    self.status_message = Some(message.clone());
                    if !had_response {
                        self.messages.push(ConversationMessage {
                            role: MessageRole::System,
                            content: format!("Error: {message}"),
                            thinking: None,
                            collapsed: false,
                        });
                    }
                }

                // Re-focus input BEFORE syncing the viewport — focus()
                // can change the input height, and the viewport offset
                // calculation depends on view_effective_conversation_height()
                // which accounts for the input area.
                self.input.focus();

                // Sync the viewport so the finalized (markdown-rendered)
                // message is visible. This is critical: without it the
                // viewport's stored content would still reflect the raw
                // streaming text, causing the final message to appear
                // overwritten or missing.
                self.refresh_conversation_viewport(follow_tail);

                if !self.pending_inputs.is_empty() {
                    return Some(Cmd::new(|| Message::new(PiMsg::RunPending)));
                }
            }
            PiMsg::AgentError(error) => {
                self.current_response.clear();
                self.current_thinking.clear();
                let content = if error.contains('\n') || error.starts_with("Error:") {
                    error
                } else {
                    format!("Error: {error}")
                };
                self.messages.push(ConversationMessage {
                    role: MessageRole::System,
                    content,
                    thinking: None,
                    collapsed: false,
                });
                self.agent_state = AgentState::Idle;
                self.current_tool = None;
                self.abort_handle = None;
                self.input.focus();
                self.refresh_conversation_viewport(true);

                if !self.pending_inputs.is_empty() {
                    return Some(Cmd::new(|| Message::new(PiMsg::RunPending)));
                }
            }
            PiMsg::CredentialUpdated { provider } => {
                self.sync_active_provider_credentials(&provider);
            }
            PiMsg::UpdateLastUserMessage(content) => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|message| message.role == MessageRole::User)
                {
                    message.content = content;
                }
                self.scroll_to_bottom();
            }
            PiMsg::System(message) => {
                self.messages.push(ConversationMessage {
                    role: MessageRole::System,
                    content: message,
                    thinking: None,
                    collapsed: false,
                });
                self.agent_state = AgentState::Idle;
                self.current_tool = None;
                self.abort_handle = None;
                self.scroll_to_bottom();
                self.input.focus();

                if !self.pending_inputs.is_empty() {
                    return Some(Cmd::new(|| Message::new(PiMsg::RunPending)));
                }
            }
            PiMsg::SystemNote(message) => {
                self.messages.push(ConversationMessage {
                    role: MessageRole::System,
                    content: message,
                    thinking: None,
                    collapsed: false,
                });
                self.scroll_to_bottom();
            }
            PiMsg::BashResult {
                display,
                content_for_agent,
            } => {
                self.bash_running = false;
                self.current_tool = None;
                self.agent_state = AgentState::Idle;

                if let Some(content) = content_for_agent {
                    self.scroll_to_bottom();
                    return self.submit_content(content);
                }

                self.messages.push(ConversationMessage {
                    role: MessageRole::System,
                    content: display,
                    thinking: None,
                    collapsed: false,
                });
                self.scroll_to_bottom();
                self.input.focus();

                if !self.pending_inputs.is_empty() {
                    return Some(Cmd::new(|| Message::new(PiMsg::RunPending)));
                }
            }
            PiMsg::OAuthDeviceFlowStarted {
                provider,
                device_code,
                user_code,
                verification_uri,
                expires_in,
            } => {
                let message = format!(
                    "OAuth login: {provider}\n\n\
Open this URL:\n{verification_uri}\n\n\
If prompted, enter this code: {user_code}\n\
Code expires in {expires_in} seconds.\n\n\
After approving access in the browser, press Enter in Pi to complete login."
                );
                self.messages.push(ConversationMessage {
                    role: MessageRole::System,
                    content: message,
                    thinking: None,
                    collapsed: false,
                });
                self.scroll_to_bottom();
                self.pending_oauth = Some(PendingOAuth {
                    provider,
                    kind: PendingLoginKind::DeviceFlow,
                    verifier: String::new(),
                    device_code: Some(device_code),
                    redirect_uri: None,
                });
                self.input_mode = InputMode::SingleLine;
                self.set_input_height(3);
                self.input.focus();
                self.status_message = None;
            }
            PiMsg::ConversationReset {
                messages,
                usage,
                status,
            } => {
                self.messages = messages;
                self.total_usage = usage;
                self.current_response.clear();
                self.current_thinking.clear();
                self.agent_state = AgentState::Idle;
                self.current_tool = None;
                self.abort_handle = None;
                self.status_message = status;
                if let Err(message) = self.sync_runtime_selection_from_session_header() {
                    self.status_message = Some(message);
                }
                self.scroll_to_bottom();
                self.input.focus();
            }
            PiMsg::SetEditorText(text) => {
                self.input.set_value(&text);
                self.input.focus();
            }
            PiMsg::OpenTree {
                initial_selected_id,
                label,
            } => {
                if self.agent_state != AgentState::Idle {
                    self.status_message = Some("Cannot open tree while processing".to_string());
                    return None;
                }

                let Ok(session_guard) = self.session.try_lock() else {
                    self.status_message = Some("Session busy; try again".to_string());
                    return None;
                };
                let selector = TreeSelectorState::new(
                    &session_guard,
                    self.term_height,
                    initial_selected_id.as_deref(),
                    label,
                );
                self.tree_ui = Some(TreeUiState::Selector(selector));
            }
            PiMsg::ResourcesReloaded {
                resources,
                status,
                diagnostics,
            } => {
                let autocomplete_catalog = AutocompleteCatalog::from_resources(&resources);
                self.autocomplete.provider.set_catalog(autocomplete_catalog);
                self.autocomplete.close();
                self.resources = resources;
                self.apply_theme(Theme::resolve(&self.config, &self.cwd));
                self.agent_state = AgentState::Idle;
                self.current_tool = None;
                self.abort_handle = None;
                self.status_message = Some(status);
                if let Some(message) = diagnostics {
                    self.messages.push(ConversationMessage {
                        role: MessageRole::System,
                        content: message,
                        thinking: None,
                        collapsed: false,
                    });
                    self.scroll_to_bottom();
                }
                self.input.focus();
            }
            PiMsg::OAuthCallbackReceived(callback_url) => {
                // Auto-submit the OAuth code received from the local callback server.
                if let Some(pending) = self.pending_oauth.take() {
                    self.messages.push(ConversationMessage {
                        role: MessageRole::System,
                        content: "Authorization callback received from browser.".to_string(),
                        thinking: None,
                        collapsed: false,
                    });
                    self.scroll_to_bottom();
                    return self.submit_oauth_code(&callback_url, pending);
                }
            }
        }
        None
    }

    fn run_next_pending(&mut self) -> Option<Cmd> {
        loop {
            if self.agent_state != AgentState::Idle {
                return None;
            }
            let next = self.pending_inputs.pop_front()?;

            let cmd = match next {
                PendingInput::Text(text) => self.submit_message(&text),
                PendingInput::Content(content) => self.submit_content(content),
                PendingInput::Continue => self.submit_continue(),
            };

            if cmd.is_some() {
                return cmd;
            }
        }
    }

    pub(super) fn queue_input(&mut self, kind: QueuedMessageKind) {
        let raw_text = self.input.value();
        let trimmed = raw_text.trim();
        if trimmed.is_empty() {
            self.status_message = Some("No input to queue".to_string());
            return;
        }

        let expanded = self.resources.expand_input(trimmed);

        // Track input history
        self.history.push(trimmed.to_string());

        if let Ok(mut queue) = self.message_queue.lock() {
            match kind {
                QueuedMessageKind::Steering => queue.push_steering(expanded),
                QueuedMessageKind::FollowUp => queue.push_follow_up(expanded),
            }
        }

        // Clear input and reset to single-line mode
        self.input.reset();
        self.input_mode = InputMode::SingleLine;
        self.set_input_height(3);

        let label = match kind {
            QueuedMessageKind::Steering => "steering",
            QueuedMessageKind::FollowUp => "follow-up",
        };
        self.status_message = Some(format!("Queued {label} message"));
    }

    pub(super) fn restore_queued_messages_to_editor(&mut self, abort: bool) -> usize {
        let (steering, follow_up) = self
            .message_queue
            .lock()
            .map_or_else(|_| (Vec::new(), Vec::new()), |mut queue| queue.clear_all());
        let mut all = steering;
        all.extend(follow_up);
        if all.is_empty() {
            if abort {
                self.abort_agent();
            }
            return 0;
        }

        let queued_text = all.join("\n\n");
        let current_text = self.input.value();
        let combined = [queued_text, current_text]
            .into_iter()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.input.set_value(&combined);
        if combined.contains('\n') {
            self.input_mode = InputMode::MultiLine;
            self.set_input_height(6);
        }
        self.input.focus();

        if abort {
            self.abort_agent();
        }

        all.len()
    }

    fn abort_agent(&self) {
        if let Some(handle) = &self.abort_handle {
            handle.abort();
        }
    }

    #[allow(clippy::too_many_lines)]
    fn submit_continue(&mut self) -> Option<Cmd> {
        if let Err(message) = self.sync_runtime_selection_from_session_header() {
            self.status_message = Some(message);
            return None;
        }

        let event_tx = self.event_tx.clone();
        let agent = Arc::clone(&self.agent);
        let session = Arc::clone(&self.session);
        let save_enabled = self.save_enabled;
        let runtime_handle = self.runtime_handle.clone();
        let (abort_handle, abort_signal) = AbortHandle::new();
        self.abort_handle = Some(abort_handle);

        self.agent_state = AgentState::Processing;
        self.scroll_to_bottom();

        let task_cx = Cx::current().unwrap_or_else(Cx::for_request);
        runtime_handle.spawn(async move {
            #[cfg(test)]
            emit_submit_continue_deadline_probe(task_cx.budget().deadline);
            let mut agent_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&agent), &task_cx).await {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock agent: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            let previous_len = agent_guard.messages().len();

            let event_sender = event_tx.clone();
            let ui_stream_batcher = Arc::new(StdMutex::new(UiStreamDeltaBatcher::new(
                event_sender.clone(),
            )));
            let ui_stream_batcher_for_events = Arc::clone(&ui_stream_batcher);
            let result = agent_guard
                .run_continue_with_abort(Some(abort_signal), move |event| {
                    let mut batcher = match ui_stream_batcher_for_events.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    dispatch_agent_event_to_ui(&event, &mut batcher);
                })
                .await;
            flush_ui_stream_batcher_with_backpressure(&ui_stream_batcher).await;

            let new_messages: Vec<crate::model::Message> =
                agent_guard.messages()[previous_len..].to_vec();
            drop(agent_guard);

            let mut session_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&session), &task_cx).await
                {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock session: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            for message in new_messages {
                session_guard.append_model_message(message);
            }
            let mut save_error = None;

            if save_enabled {
                if let Err(err) = session_guard.save().await {
                    save_error = Some(format!("Failed to save session: {err}"));
                }
            }
            drop(session_guard);

            if let Some(err) = save_error {
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(err),
                )
                .await;
            }

            if let Err(err) = result {
                let formatted = crate::error_hints::format_error_with_hints(&err);
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(formatted),
                )
                .await;
            }
        });

        None
    }

    #[allow(clippy::too_many_lines)]
    fn submit_content(&mut self, content: Vec<ContentBlock>) -> Option<Cmd> {
        let display = content_blocks_to_text(&content);
        self.submit_content_with_display(content, &display)
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn submit_content_with_display(
        &mut self,
        content: Vec<ContentBlock>,
        display: &str,
    ) -> Option<Cmd> {
        if content.is_empty() {
            return None;
        }

        if let Err(message) = self.sync_runtime_selection_from_session_header() {
            self.status_message = Some(message);
            return None;
        }

        let display_owned = display.to_string();
        if !display_owned.trim().is_empty() {
            self.messages.push(ConversationMessage {
                role: MessageRole::User,
                content: display_owned.clone(),
                thinking: None,
                collapsed: false,
            });
        }

        // Clear input and reset to single-line mode
        self.input.reset();
        self.input_mode = InputMode::SingleLine;
        self.set_input_height(3);

        // Start processing
        self.agent_state = AgentState::Processing;

        // Auto-scroll to bottom when new message is added
        self.scroll_to_bottom();

        let content_for_agent = content;
        let event_tx = self.event_tx.clone();
        let agent = Arc::clone(&self.agent);
        let session = Arc::clone(&self.session);
        let save_enabled = self.save_enabled;
        let runtime_handle = self.runtime_handle.clone();
        let (abort_handle, abort_signal) = AbortHandle::new();
        self.abort_handle = Some(abort_handle);

        let task_cx = Cx::current().unwrap_or_else(Cx::for_request);
        runtime_handle.spawn(async move {
            let base_system_prompt = {
                let guard =
                    match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&agent), &task_cx)
                        .await
                    {
                        Ok(guard) => guard,
                        Err(err) => {
                            let _ = crate::interactive::enqueue_pi_event(
                                &event_tx,
                                &Cx::for_request(),
                                PiMsg::AgentError(format!("Failed to lock agent: {err}")),
                            )
                            .await;
                            return;
                        }
                    };
                let prompt = guard.system_prompt().map(str::to_string);
                drop(guard);
                prompt
            };
            let mut agent_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&agent), &task_cx).await {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock agent: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            agent_guard.set_system_prompt(base_system_prompt.clone());
            let previous_len = agent_guard.messages().len();

            let event_sender = event_tx.clone();
            let ui_stream_batcher = Arc::new(StdMutex::new(UiStreamDeltaBatcher::new(
                event_sender.clone(),
            )));
            let ui_stream_batcher_for_events = Arc::clone(&ui_stream_batcher);
            let user_message = ModelMessage::User(UserMessage {
                content: UserContent::Blocks(content_for_agent),
                timestamp: Utc::now().timestamp_millis(),
            });
            let mut prompts = Vec::with_capacity(1);
            prompts.push(user_message);

            let result = agent_guard
                .run_with_messages_with_abort(prompts, Some(abort_signal), move |event| {
                    let mut batcher = match ui_stream_batcher_for_events.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    dispatch_agent_event_to_ui(&event, &mut batcher);
                })
                .await;
            flush_ui_stream_batcher_with_backpressure(&ui_stream_batcher).await;

            agent_guard.set_system_prompt(base_system_prompt);

            let new_messages: Vec<crate::model::Message> =
                agent_guard.messages()[previous_len..].to_vec();
            drop(agent_guard);

            let mut session_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&session), &task_cx).await
                {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock session: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            for message in new_messages {
                session_guard.append_model_message(message);
            }
            let mut save_error = None;

            if save_enabled {
                if let Err(err) = session_guard.save().await {
                    save_error = Some(format!("Failed to save session: {err}"));
                }
            }
            drop(session_guard);

            if let Some(err) = save_error {
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(err),
                )
                .await;
            }

            if let Err(err) = result {
                let formatted = crate::error_hints::format_error_with_hints(&err);
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(formatted),
                )
                .await;
            }
        });

        None
    }

    /// Submit a message to the agent.
    #[allow(clippy::too_many_lines)]
    pub(super) fn submit_message(&mut self, message: &str) -> Option<Cmd> {
        let message = message.trim();
        if message.is_empty() {
            return None;
        }

        if let Some(pending) = self.pending_oauth.take() {
            return self.submit_oauth_code(message, pending);
        }

        if let Some((command, exclude_from_context)) = parse_bash_command(message) {
            return self.submit_bash_command(message, command, exclude_from_context);
        }

        // Check for slash commands
        if let Some((cmd, args)) = SlashCommand::parse(message) {
            return self.handle_slash_command(cmd, args);
        }

        if message.starts_with('/') && !message.starts_with("/skill:") {
            let command = message.split_whitespace().next().unwrap_or(message);
            let error = format!("Unknown command: {command}");
            self.status_message = Some(error.clone());
            self.messages.push(ConversationMessage {
                role: MessageRole::System,
                content: error,
                thinking: None,
                collapsed: false,
            });
            self.scroll_to_bottom();
            self.input.reset();
            self.input.focus();
            return None;
        }

        if let Err(message) = self.sync_runtime_selection_from_session_header() {
            if message.starts_with("Agent busy;") || message.starts_with("Session busy;") {
                tracing::debug!(
                    message,
                    "skipping runtime selection sync while submitting input"
                );
            } else {
                self.status_message = Some(message);
                return None;
            }
        }

        let message_owned = message.to_string();
        let (message_without_refs, file_refs) = self.extract_file_references(&message_owned);
        let message_for_agent = if file_refs.is_empty() {
            self.resources.expand_input(&message_owned)
        } else {
            self.resources.expand_input(message_without_refs.trim())
        };

        if !file_refs.is_empty() {
            let auto_resize = self
                .config
                .images
                .as_ref()
                .and_then(|images| images.auto_resize)
                .unwrap_or(true);

            let processed = match process_file_arguments(&file_refs, &self.cwd, auto_resize) {
                Ok(processed) => processed,
                Err(err) => {
                    self.status_message = Some(err.to_string());
                    return None;
                }
            };

            let mut text = processed.text;
            if !message_for_agent.trim().is_empty() {
                text.push_str(&message_for_agent);
            }

            let mut content = Vec::new();
            if !text.trim().is_empty() {
                content.push(ContentBlock::Text(TextContent::new(text)));
            }
            for image in processed.images {
                content.push(ContentBlock::Image(image));
            }

            self.history.push(message_owned.clone());

            let display = content_blocks_to_text(&content);
            return self.submit_content_with_display(content, &display);
        }
        let event_tx = self.event_tx.clone();
        let agent = Arc::clone(&self.agent);
        let session = Arc::clone(&self.session);
        let save_enabled = self.save_enabled;
        let (abort_handle, abort_signal) = AbortHandle::new();
        self.abort_handle = Some(abort_handle);

        // Add to history
        self.history.push(message_owned.clone());

        // Add user message to display
        self.messages.push(ConversationMessage {
            role: MessageRole::User,
            content: message_for_agent.clone(),
            thinking: None,
            collapsed: false,
        });
        // Clear input and reset to single-line mode
        self.input.reset();
        self.input_mode = InputMode::SingleLine;
        self.set_input_height(3);

        // Start processing
        self.agent_state = AgentState::Processing;

        // Auto-scroll to bottom when new message is added
        self.scroll_to_bottom();

        let runtime_handle = self.runtime_handle.clone();

        // Spawn async task to run the agent
        let task_cx = Cx::current().unwrap_or_else(Cx::for_request);
        runtime_handle.spawn(async move {
            let input_images = Vec::new();

            let mut agent_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&agent), &task_cx).await {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock agent: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            let previous_len = agent_guard.messages().len();

            let event_sender = event_tx.clone();
            let ui_stream_batcher = Arc::new(StdMutex::new(UiStreamDeltaBatcher::new(
                event_sender.clone(),
            )));
            let result = if input_images.is_empty() {
                let ui_stream_batcher_for_events = Arc::clone(&ui_stream_batcher);
                agent_guard
                    .run_with_abort(message_for_agent, Some(abort_signal), move |event| {
                        let mut batcher = match ui_stream_batcher_for_events.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        dispatch_agent_event_to_ui(&event, &mut batcher);
                    })
                    .await
            } else {
                let content_for_agent =
                    build_content_blocks_for_input(&message_for_agent, &input_images);
                let ui_stream_batcher_for_events = Arc::clone(&ui_stream_batcher);
                agent_guard
                    .run_with_content_with_abort(
                        content_for_agent,
                        Some(abort_signal),
                        move |event| {
                            let mut batcher = match ui_stream_batcher_for_events.lock() {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                            dispatch_agent_event_to_ui(&event, &mut batcher);
                        },
                    )
                    .await
            };
            flush_ui_stream_batcher_with_backpressure(&ui_stream_batcher).await;

            let new_messages: Vec<crate::model::Message> =
                agent_guard.messages()[previous_len..].to_vec();
            drop(agent_guard);

            let mut session_guard =
                match asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&session), &task_cx).await
                {
                    Ok(guard) => guard,
                    Err(err) => {
                        let _ = crate::interactive::enqueue_pi_event(
                            &event_tx,
                            &Cx::for_request(),
                            PiMsg::AgentError(format!("Failed to lock session: {err}")),
                        )
                        .await;
                        return;
                    }
                };
            for message in new_messages {
                session_guard.append_model_message(message);
            }
            let mut save_error = None;

            if save_enabled {
                if let Err(err) = session_guard.save().await {
                    save_error = Some(format!("Failed to save session: {err}"));
                }
            }
            drop(session_guard);

            if let Some(err) = save_error {
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(err),
                )
                .await;
            }

            if let Err(err) = result {
                let _ = crate::interactive::enqueue_pi_event(
                    &event_tx,
                    &Cx::for_request(),
                    PiMsg::AgentError(err.to_string()),
                )
                .await;
            }
        });

        None
    }
}

#[cfg(test)]
fn submit_continue_deadline_probe()
-> &'static std::sync::Mutex<Option<std::sync::mpsc::Sender<Option<asupersync::Time>>>> {
    static PROBE: std::sync::OnceLock<
        std::sync::Mutex<Option<std::sync::mpsc::Sender<Option<asupersync::Time>>>>,
    > = std::sync::OnceLock::new();
    PROBE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn emit_submit_continue_deadline_probe(deadline: Option<asupersync::Time>) {
    let probe = submit_continue_deadline_probe();
    let guard = probe.lock().expect("lock submit_continue deadline probe");
    if let Some(tx) = guard.as_ref() {
        let _ = tx.send(deadline);
    }
}

#[cfg(test)]
mod stream_delta_batcher_tests {
    use super::*;
    use crate::agent::{Agent, AgentConfig};
    use crate::config::Config;
    use crate::keybindings::KeyBindings;
    use crate::model::{AssistantMessage, StreamEvent, Usage};
    use crate::provider::{Context, InputType, Model, ModelCost, Provider, StreamOptions};
    use crate::resources::{ResourceCliOptions, ResourceLoader};
    use crate::session::Session;
    use crate::tools::ToolRegistry;
    use asupersync::runtime::RuntimeBuilder;
    use futures::stream;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicUsize;

    struct DummyProvider;

    #[async_trait::async_trait]
    impl Provider for DummyProvider {
        fn name(&self) -> &'static str {
            "dummy"
        }

        fn api(&self) -> &'static str {
            "dummy"
        }

        fn model_id(&self) -> &'static str {
            "dummy-model"
        }

        async fn stream(
            &self,
            _context: &Context<'_>,
            _options: &StreamOptions,
        ) -> crate::error::Result<
            Pin<Box<dyn futures::Stream<Item = crate::error::Result<StreamEvent>> + Send>>,
        > {
            Ok(Box::pin(stream::empty()))
        }
    }

    fn runtime() -> &'static asupersync::runtime::Runtime {
        static RT: OnceLock<asupersync::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| {
            RuntimeBuilder::multi_thread()
                .blocking_threads(1, 8)
                .build()
                .expect("build runtime")
        })
    }

    fn runtime_handle() -> asupersync::runtime::RuntimeHandle {
        runtime().handle()
    }

    fn text_tool_update(text: &str) -> PiMsg {
        PiMsg::ToolUpdate {
            name: "bash".to_string(),
            tool_id: "t1".to_string(),
            content: vec![ContentBlock::Text(TextContent::new(text))],
            details: Some(json!({
                "progress": {
                    "byteCount": text.len(),
                    "lineCount": text.lines().count(),
                }
            })),
        }
    }

    fn model_entry(provider: &str, id: &str) -> ModelEntry {
        ModelEntry {
            model: Model {
                id: id.to_string(),
                name: id.to_string(),
                api: "openai-completions".to_string(),
                provider: provider.to_string(),
                base_url: "https://example.invalid".to_string(),
                reasoning: true,
                input: vec![InputType::Text],
                cost: ModelCost {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 8_192,
                headers: HashMap::new(),
            },
            api_key: Some("test-key".to_string()),
            headers: HashMap::new(),
            auth_header: true,
            compat: None,
        }
    }

    fn build_test_app_with_provider(provider: Arc<dyn Provider>) -> (PiApp, mpsc::Receiver<PiMsg>) {
        let current = model_entry("continue-probe", "continue-probe-model");
        let agent = Agent::new(
            provider,
            ToolRegistry::new(&[], Path::new("."), None),
            AgentConfig::default(),
        );
        let session = Arc::new(asupersync::sync::Mutex::new(Session::in_memory()));
        let resources = ResourceLoader::empty(false);
        let resource_cli = ResourceCliOptions {
            no_skills: false,
            no_prompt_templates: false,
            no_themes: false,
            skill_paths: Vec::new(),
            prompt_paths: Vec::new(),
            theme_paths: Vec::new(),
        };
        let (event_tx, event_rx) = asupersync::channel::mpsc::channel(64);
        let config = Config {
            last_changelog_version: Some(crate::platform::VERSION.to_string()),
            ..Config::default()
        };
        (
            PiApp::new(
                agent,
                session,
                config,
                resources,
                resource_cli,
                Path::new(".").to_path_buf(),
                current.clone(),
                Vec::new(),
                vec![current],
                Vec::new(),
                event_tx,
                runtime_handle(),
                true,
                false,
                None,
                Some(KeyBindings::new()),
                Vec::new(),
                Usage::default(),
            ),
            event_rx,
        )
    }

    fn build_test_app() -> PiApp {
        let (app, _event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));
        app
    }

    #[derive(Default)]
    struct ContinueProbeState {
        calls: AtomicUsize,
        saw_custom_message: AtomicBool,
        saw_user_message: AtomicBool,
    }

    struct ContinueProbeProvider {
        state: Arc<ContinueProbeState>,
    }

    impl ContinueProbeProvider {
        fn assistant_message(&self, content: &str) -> AssistantMessage {
            AssistantMessage {
                content: vec![ContentBlock::Text(TextContent::new(content))],
                api: self.api().to_string(),
                provider: self.name().to_string(),
                model: self.model_id().to_string(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for ContinueProbeProvider {
        fn name(&self) -> &'static str {
            "continue-probe"
        }

        fn api(&self) -> &'static str {
            "continue-probe"
        }

        fn model_id(&self) -> &'static str {
            "continue-probe-model"
        }

        async fn stream(
            &self,
            context: &Context<'_>,
            _options: &StreamOptions,
        ) -> crate::error::Result<
            Pin<Box<dyn futures::Stream<Item = crate::error::Result<StreamEvent>> + Send>>,
        > {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            self.state.saw_custom_message.store(
                context.messages.iter().any(|message| {
                    matches!(
                        message,
                        ModelMessage::Custom(CustomMessage { custom_type, content, .. })
                            if custom_type == "note" && content == "continue-now"
                    )
                }),
                Ordering::SeqCst,
            );
            self.state.saw_user_message.store(
                context
                    .messages
                    .iter()
                    .any(|message| matches!(message, ModelMessage::User(_))),
                Ordering::SeqCst,
            );

            let partial = self.assistant_message("");
            let message = self.assistant_message("continued");
            Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::Start { partial }),
                Ok(StreamEvent::Done {
                    reason: StopReason::Stop,
                    message,
                }),
            ])))
        }
    }

    #[test]
    fn coalesces_adjacent_deltas_of_same_kind() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut batcher = UiStreamDeltaBatcher::new(tx);
        batcher.flush_interval = std::time::Duration::from_secs(60);
        batcher.last_flush = std::time::Instant::now();

        batcher.push_delta(StreamDeltaKind::Text, "Hel");
        batcher.push_delta(StreamDeltaKind::Text, "lo");
        assert!(rx.try_recv().is_err());

        batcher.flush(true);
        let msg = rx.try_recv().expect("expected coalesced text delta");
        assert!(matches!(msg, PiMsg::TextDelta(text) if text == "Hello"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn send_immediate_flushes_pending_before_tool_event() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut batcher = UiStreamDeltaBatcher::new(tx);
        batcher.flush_interval = std::time::Duration::from_secs(60);
        batcher.last_flush = std::time::Instant::now();

        batcher.push_delta(StreamDeltaKind::Text, "partial");
        batcher.send_immediate(PiMsg::ToolStart {
            name: "bash".to_string(),
            tool_id: "t1".to_string(),
        });

        let first = rx.try_recv().expect("expected flushed text delta first");
        let second = rx.try_recv().expect("expected immediate tool start second");
        assert!(matches!(first, PiMsg::TextDelta(text) if text == "partial"));
        assert!(
            matches!(second, PiMsg::ToolStart { name, tool_id } if name == "bash" && tool_id == "t1")
        );
    }

    #[test]
    fn normal_tool_updates_flush_immediately() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut batcher = UiStreamDeltaBatcher::new(tx);

        batcher.send_immediate(text_tool_update("first"));

        let msg = rx.try_recv().expect("expected immediate tool update");
        assert!(matches!(
            msg,
            PiMsg::ToolUpdate { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text(text)) if text.text == "first")
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn retains_unsent_chunk_when_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut batcher = UiStreamDeltaBatcher::new(tx);
        batcher.flush_interval = std::time::Duration::from_secs(60);
        batcher.last_flush = std::time::Instant::now();

        batcher.send_immediate(PiMsg::System("occupy".to_string()));
        batcher.push_delta(StreamDeltaKind::Text, "later");
        batcher.flush(true);
        assert_eq!(batcher.pending_bytes, "later".len());

        let _ = rx.try_recv().expect("expected occupied slot message");
        batcher.flush(true);

        let msg = rx.try_recv().expect("expected retained text delta");
        assert!(matches!(msg, PiMsg::TextDelta(text) if text == "later"));
        assert_eq!(batcher.pending_bytes, 0);
    }

    #[test]
    fn retains_immediate_events_when_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut batcher = UiStreamDeltaBatcher::new(tx);
        batcher.flush_interval = std::time::Duration::from_secs(60);
        batcher.last_flush = std::time::Instant::now();

        // Occupy the single slot.
        batcher.send_immediate(PiMsg::System("occupy".to_string()));

        // Queue a delta and a control event while the channel is full.
        batcher.push_delta(StreamDeltaKind::Text, "before-done");
        batcher.send_immediate(PiMsg::AgentDone {
            usage: None,
            stop_reason: StopReason::Stop,
            error_message: None,
        });

        // Nothing should be dropped; queue should still hold both messages.
        assert_eq!(batcher.pending_bytes, "before-done".len());
        assert_eq!(batcher.pending.len(), 2);

        // Free slot and flush repeatedly; ordering must be preserved.
        let _ = rx.try_recv().expect("expected occupied slot message");
        batcher.flush(true);
        let first = rx.try_recv().expect("expected retained text delta");
        assert!(matches!(first, PiMsg::TextDelta(text) if text == "before-done"));

        batcher.flush(true);
        let second = rx.try_recv().expect("expected retained agent_done event");
        assert!(matches!(second, PiMsg::AgentDone { .. }));
    }

    #[test]
    fn continue_pending_input_runs_agent_without_new_user_message() {
        let state = Arc::new(ContinueProbeState::default());
        let provider: Arc<dyn Provider> = Arc::new(ContinueProbeProvider {
            state: Arc::clone(&state),
        });
        let (mut app, mut event_rx) = build_test_app_with_provider(provider);

        runtime().block_on(async {
            let cx = Cx::for_request();
            let mut guard = asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&app.agent), &cx)
                .await
                .expect("lock agent");
            guard.add_message(ModelMessage::Custom(CustomMessage {
                content: "continue-now".to_string(),
                custom_type: "note".to_string(),
                display: true,
                details: None,
                timestamp: 0,
            }));
        });

        let _ = app.handle_pi_message(PiMsg::EnqueuePendingInput(PendingInput::Continue));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut saw_done = false;
        while std::time::Instant::now() < deadline {
            match event_rx.try_recv() {
                Ok(PiMsg::AgentDone { error_message, .. }) => {
                    saw_done = true;
                    if let Some(err) = error_message {
                        println!("AgentDone error: {}", err);
                    }
                }
                Ok(PiMsg::AgentError(err)) => {
                    println!("AgentError: {}", err);
                }
                Ok(_) => {}
                Err(_) => {}
            }

            if saw_done && state.calls.load(Ordering::SeqCst) == 1 {
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if state.calls.load(Ordering::SeqCst) == 0 {
            println!("Status message: {:?}", app.status_message);
        }

        assert!(saw_done, "submit_message path should finish an agent turn");
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        assert!(
            state.saw_custom_message.load(Ordering::SeqCst),
            "continue path should reuse the injected custom message as provider context"
        );
        assert!(
            !state.saw_user_message.load(Ordering::SeqCst),
            "continue path should not synthesize a user message"
        );
    }

    #[test]
    fn spawn_save_session_inherits_cancelled_context_when_session_lock_is_held() {
        let (app, mut event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));

        runtime().block_on(async {
            let hold_cx = Cx::for_request();
            let _held_guard =
                asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&app.session), &hold_cx)
                    .await
                    .expect("lock session");

            let ambient_cx = Cx::for_testing();
            ambient_cx.set_cancel_requested(true);
            let _current = Cx::set_current(Some(ambient_cx));

            app.spawn_save_session();

            let recv_cx = Cx::for_testing();
            let wait_for_error = async {
                loop {
                    match event_rx.recv(&recv_cx).await {
                        Ok(PiMsg::AgentError(message))
                            if message.contains("Failed to lock session") =>
                        {
                            break message;
                        }
                        Ok(_) => {}
                        Err(err) => break format!("event receive failed: {err}"),
                    }
                }
            };
            futures::pin_mut!(wait_for_error);
            let err = asupersync::time::timeout(
                asupersync::time::wall_now(),
                std::time::Duration::from_secs(1),
                wait_for_error,
            )
            .await
            .expect("cancelled save task should finish before timeout");

            assert!(
                err.contains("Failed to lock session"),
                "unexpected save-task error: {err}"
            );
        });
    }

    #[test]
    fn submit_continue_inherits_cancelled_context_when_agent_lock_is_attempted() {
        let (mut app, mut event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));

        runtime().block_on(async {
            let ambient_cx = Cx::for_testing();
            ambient_cx.set_cancel_requested(true);
            let _current = Cx::set_current(Some(ambient_cx));

            let _ = app.submit_continue();

            let recv_cx = Cx::for_testing();
            let wait_for_terminal = async {
                loop {
                    match event_rx.recv(&recv_cx).await {
                        Ok(PiMsg::AgentError(message)) => break format!("error:{message}"),
                        Ok(PiMsg::AgentDone { error_message, .. }) => {
                            break format!("done:{}", error_message.unwrap_or_default());
                        }
                        Ok(_) => {}
                        Err(err) => break format!("receive-error:{err}"),
                    }
                }
            };
            futures::pin_mut!(wait_for_terminal);
            let outcome = asupersync::time::timeout(
                asupersync::time::wall_now(),
                std::time::Duration::from_secs(1),
                wait_for_terminal,
            )
            .await
            .expect("cancelled continue task should reach provider before timeout");

            assert!(
                outcome.contains("Failed to lock agent"),
                "unexpected continue-task outcome: {outcome}"
            );
        });
    }

    #[test]
    fn submit_continue_inherits_deadline_into_spawned_task() {
        struct ProbeReset;
        impl Drop for ProbeReset {
            fn drop(&mut self) {
                let mut probe = submit_continue_deadline_probe()
                    .lock()
                    .expect("lock submit_continue deadline probe");
                *probe = None;
            }
        }

        let (mut app, _event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));

        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        {
            let mut probe = submit_continue_deadline_probe()
                .lock()
                .expect("lock submit_continue deadline probe");
            assert!(
                probe.is_none(),
                "submit_continue deadline probe already installed"
            );
            *probe = Some(probe_tx);
        }
        let _probe_reset = ProbeReset;

        runtime().block_on(async {
            let cx = Cx::for_request();
            let mut guard = asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&app.agent), &cx)
                .await
                .expect("lock agent");
            guard.add_message(ModelMessage::Custom(CustomMessage {
                content: "continue-now".to_string(),
                custom_type: "note".to_string(),
                display: true,
                details: None,
                timestamp: 0,
            }));
        });

        let expected_deadline = asupersync::time::wall_now() + std::time::Duration::from_secs(30);
        let ambient_cx = Cx::for_testing_with_budget(
            asupersync::Budget::INFINITE.with_deadline(expected_deadline),
        );
        let _current = Cx::set_current(Some(ambient_cx));

        let _ = app.handle_pi_message(PiMsg::EnqueuePendingInput(PendingInput::Continue));

        let recorded = loop {
            let res = probe_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("submit_continue deadline probe");
            if res == Some(expected_deadline) {
                break res;
            }
        };
        assert_eq!(recorded, Some(expected_deadline));
    }

    #[test]
    fn conversation_reset_syncs_runtime_model_and_thinking_from_session_header() {
        let (mut app, _event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));
        let mut next = model_entry("openai", "gpt-4o");
        next.model.reasoning = false;
        app.available_models.push(next.clone());

        runtime().block_on(async {
            let cx = Cx::for_request();
            let mut session_guard =
                asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&app.session), &cx)
                    .await
                    .expect("lock session");
            session_guard.header.provider = Some(next.model.provider.clone());
            session_guard.header.model_id = Some(next.model.id.clone());
            session_guard.header.thinking_level = Some("high".to_string());
        });

        let _ = app.handle_pi_message(PiMsg::ConversationReset {
            messages: Vec::new(),
            usage: Usage::default(),
            status: Some("Session resumed".to_string()),
        });

        assert_eq!(app.model, "openai/gpt-4o");
        assert_eq!(app.model_entry.model.provider, "openai");
        assert_eq!(app.model_entry.model.id, "gpt-4o");
        assert_eq!(app.status_message.as_deref(), Some("Session resumed"));

        let shared = app
            .model_entry_shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(shared.model.provider, "openai");
        assert_eq!(shared.model.id, "gpt-4o");
        drop(shared);

        let agent_guard = app.agent.try_lock().expect("lock agent");
        assert_eq!(agent_guard.provider().name(), "openai");
        assert_eq!(agent_guard.provider().model_id(), "gpt-4o");
        assert_eq!(
            agent_guard.stream_options().thinking_level,
            Some(crate::model::ThinkingLevel::Off)
        );
    }

    #[test]
    fn fast_tree_navigation_syncs_runtime_model_and_thinking_from_target_branch() {
        let (mut app, _event_rx) = build_test_app_with_provider(Arc::new(DummyProvider));
        let mut next = model_entry("openai", "gpt-4o");
        next.model.reasoning = false;
        app.available_models.push(next.clone());

        let (session_id, current_leaf_id, target_leaf_id) = runtime().block_on(async {
            let cx = Cx::for_request();
            let mut session_guard =
                asupersync::sync::OwnedMutexGuard::lock(Arc::clone(&app.session), &cx)
                    .await
                    .expect("lock session");
            let root_id = session_guard.append_message(crate::session::SessionMessage::User {
                content: crate::model::UserContent::Text("root".to_string()),
                timestamp: Some(0),
            });
            let current_leaf_id =
                session_guard.append_message(crate::session::SessionMessage::User {
                    content: crate::model::UserContent::Text("current".to_string()),
                    timestamp: Some(0),
                });
            assert!(session_guard.create_branch_from(&root_id));
            session_guard.append_model_change(next.model.provider.clone(), next.model.id.clone());
            session_guard.append_thinking_level_change("high".to_string());
            let target_leaf_id =
                session_guard.append_message(crate::session::SessionMessage::User {
                    content: crate::model::UserContent::Text("target".to_string()),
                    timestamp: Some(0),
                });
            assert!(session_guard.navigate_to(&current_leaf_id));
            (
                session_guard.header.id.clone(),
                Some(current_leaf_id),
                Some(target_leaf_id),
            )
        });

        let switched = app.start_tree_navigation(
            super::super::tree::PendingTreeNavigation {
                session_id,
                old_leaf_id: current_leaf_id,
                new_leaf_id: target_leaf_id,
                editor_text: None,
                entries_to_summarize: Vec::new(),
                summary_from_id: String::new(),
                api_key_present: false,
            },
            super::super::tree::TreeSummaryChoice::NoSummary,
            None,
        );

        assert!(switched, "fast tree navigation should succeed");
        assert_eq!(app.model, "openai/gpt-4o");
        assert_eq!(app.model_entry.model.provider, "openai");
        assert_eq!(app.model_entry.model.id, "gpt-4o");
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|msg| msg.starts_with("Switched to ")),
            "status should still report the branch switch"
        );

        let shared = app
            .model_entry_shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(shared.model.provider, "openai");
        assert_eq!(shared.model.id, "gpt-4o");
        drop(shared);

        let agent_guard = app.agent.try_lock().expect("lock agent");
        assert_eq!(agent_guard.provider().name(), "openai");
        assert_eq!(agent_guard.provider().model_id(), "gpt-4o");
        assert_eq!(
            agent_guard.stream_options().thinking_level,
            Some(crate::model::ThinkingLevel::Off)
        );
    }
}
