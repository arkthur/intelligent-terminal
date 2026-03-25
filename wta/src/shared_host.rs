use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use crate::app::{AppEvent, ChatMessage, ConnectionState, DebugDir, DebugMessage, PermOption};
use crate::coordinator::{parse_recommendation_set, RecommendationChoice, RecommendationSet};
use crate::protocol::acp::client::{run_acp_client, PromptSubmission};
use crate::shell::wt_channel::ConnectionInfo;
use crate::shell::ShellManager;
use crate::ui_trace;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneContext {
    pub pane_id: Option<String>,
    pub tab_id: Option<String>,
    pub window_id: Option<String>,
    pub cwd: Option<String>,
    pub source_pane_id: Option<String>,
}

impl PaneContext {
    pub fn effective_source_pane_id(&self) -> Option<&str> {
        self.source_pane_id.as_deref().or(self.pane_id.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionPrompt {
    pub description: String,
    pub options: Vec<PermOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedStateSnapshot {
    pub version: u64,
    pub state: ConnectionState,
    pub agent_name: String,
    pub session_id: String,
    pub wt_connected: bool,
    pub messages: Vec<ChatMessage>,
    pub recommendations: Option<RecommendationSet>,
    pub agent_streaming: bool,
    pub pending_agent_response: String,
    pub prompt_in_flight: bool,
    pub permission: Option<PermissionPrompt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostClientRequest {
    Attach {
        pane_context: PaneContext,
    },
    GetSnapshot,
    SubmitPrompt {
        text: String,
        pane_context: Option<PaneContext>,
    },
    SelectRecommendation {
        choice: usize,
    },
    RespondPermission {
        option_id: String,
    },
    PaneContextUpdate {
        pane_context: PaneContext,
    },
    Detach,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostServerMessage {
    Attached {
        client_id: u64,
        snapshot: SharedStateSnapshot,
    },
    SharedStateSnapshot {
        snapshot: SharedStateSnapshot,
    },
    Event {
        event: SharedUiEvent,
    },
    Error {
        message: String,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SharedUiEvent {
    ConnectionStage { stage: String },
    AgentConnected { name: String, session_id: String },
    AgentError { message: String },
    UserMessage { text: String },
    AgentMessageChunk { text: String },
    AgentMessageEnd,
    ToolCall { id: String, title: String, status: String },
    ToolCallUpdate { id: String, status: String },
    Plan { entries: Vec<crate::app::PlanEntry> },
    PermissionRequest { description: String, options: Vec<PermOption> },
    PermissionCleared,
    SystemMessage { message: String },
}

impl SharedUiEvent {
    fn from_app_event(event: &AppEvent) -> Option<Self> {
        match event {
            AppEvent::ConnectionStage(stage) => Some(Self::ConnectionStage {
                stage: stage.clone(),
            }),
            AppEvent::AgentConnected { name, session_id } => Some(Self::AgentConnected {
                name: name.clone(),
                session_id: session_id.clone(),
            }),
            AppEvent::AgentError(message) => Some(Self::AgentError {
                message: message.clone(),
            }),
            AppEvent::AgentMessageChunk(text) => Some(Self::AgentMessageChunk {
                text: text.clone(),
            }),
            AppEvent::AgentMessageEnd => Some(Self::AgentMessageEnd),
            AppEvent::ToolCall { id, title, status } => Some(Self::ToolCall {
                id: id.clone(),
                title: title.clone(),
                status: status.clone(),
            }),
            AppEvent::ToolCallUpdate { id, status } => Some(Self::ToolCallUpdate {
                id: id.clone(),
                status: status.clone(),
            }),
            AppEvent::Plan(entries) => Some(Self::Plan {
                entries: entries.clone(),
            }),
            AppEvent::PermissionRequest {
                description,
                options,
                ..
            } => Some(Self::PermissionRequest {
                description: description.clone(),
                options: options.clone(),
            }),
            AppEvent::SystemMessage(message) => Some(Self::SystemMessage {
                message: message.clone(),
            }),
            AppEvent::Key(_)
            | AppEvent::Resize(_, _)
            | AppEvent::DebugPipeMessage(_)
            | AppEvent::SharedStateSnapshot(_)
            | AppEvent::UserMessage(_)
            | AppEvent::SharedPermissionRequest { .. }
            | AppEvent::PermissionCleared => None,
        }
    }

    fn into_app_event(self) -> AppEvent {
        match self {
            Self::ConnectionStage { stage } => AppEvent::ConnectionStage(stage),
            Self::AgentConnected { name, session_id } => {
                AppEvent::AgentConnected { name, session_id }
            }
            Self::AgentError { message } => AppEvent::AgentError(message),
            Self::UserMessage { text } => AppEvent::UserMessage(text),
            Self::AgentMessageChunk { text } => AppEvent::AgentMessageChunk(text),
            Self::AgentMessageEnd => AppEvent::AgentMessageEnd,
            Self::ToolCall { id, title, status } => AppEvent::ToolCall { id, title, status },
            Self::ToolCallUpdate { id, status } => AppEvent::ToolCallUpdate { id, status },
            Self::Plan { entries } => AppEvent::Plan(entries),
            Self::PermissionRequest {
                description,
                options,
            } => AppEvent::SharedPermissionRequest {
                description,
                options,
            },
            Self::PermissionCleared => AppEvent::PermissionCleared,
            Self::SystemMessage { message } => AppEvent::SystemMessage(message),
        }
    }
}

pub fn pipe_name_for(pipe_info: Option<&ConnectionInfo>) -> String {
    let mut hasher = DefaultHasher::new();
    pipe_info
        .map(|info| info.pipe_name.as_str())
        .unwrap_or("local-only")
        .hash(&mut hasher);
    format!(r"\\.\pipe\wta-shared-host-{:016x}", hasher.finish())
}

pub async fn wait_for_host(pipe_name: &str, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match try_connect_client_once(pipe_name) {
            Ok(client) => {
                let (reader, mut writer) = tokio::io::split(client);
                let mut lines = BufReader::new(reader).lines();

                send_line(&mut writer, &HostClientRequest::Ping).await?;
                if let Ok(Ok(Some(line))) =
                    tokio::time::timeout(Duration::from_secs(1), lines.next_line()).await
                {
                    let message: HostServerMessage =
                        serde_json::from_str(&line).context("invalid host ping response")?;
                    if matches!(message, HostServerMessage::Pong) {
                        return Ok(());
                    }
                }
            }
            Err(_) => {}
        }

        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for shared host pipe {}", pipe_name);
        }

        sleep(Duration::from_millis(75)).await;
    }
}

pub async fn run_attach_client(
    host_pipe_name: String,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    mut prompt_rx: mpsc::UnboundedReceiver<String>,
    mut recommendation_rx: mpsc::UnboundedReceiver<RecommendationChoice>,
    mut permission_rx: mpsc::UnboundedReceiver<String>,
    pane_context: PaneContext,
    initial_prompt: Option<String>,
    debug_capture_enabled: Arc<AtomicBool>,
) {
    if let Err(err) = run_attach_client_inner(
        host_pipe_name,
        event_tx.clone(),
        &mut prompt_rx,
        &mut recommendation_rx,
        &mut permission_rx,
        pane_context,
        initial_prompt,
        debug_capture_enabled,
    )
    .await
    {
        let _ = event_tx.send(AppEvent::AgentError(format!(
            "shared host connection failed: {:#}",
            err
        )));
    }
}

pub async fn run_host_server(
    host_pipe_name: String,
    agent_cmd: String,
    shell_mgr: Arc<ShellManager>,
    wt_connected: bool,
) -> Result<()> {
    host_log(&format!(
        "starting shared host pipe={} wt_connected={}",
        host_pipe_name, wt_connected
    ));

    let (host_command_tx, host_command_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_accept_loop(
        host_pipe_name.clone(),
        host_command_tx.clone(),
    ));

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel();
    let (recommendation_tx, recommendation_rx) = mpsc::unbounded_channel();

    tokio::spawn(crate::coordinator::run_recommendation_executor(
        recommendation_rx,
        event_tx.clone(),
        shell_mgr.clone(),
        crate::coordinator::default_delegate_agent_runtimes(),
    ));

    tokio::task::spawn_local(run_acp_client(
        agent_cmd,
        event_tx.clone(),
        prompt_rx,
        shell_mgr,
        wt_connected,
    ));

    run_host_service(
        host_command_rx,
        event_rx,
        prompt_tx,
        recommendation_tx,
        wt_connected,
    )
    .await
}

async fn run_attach_client_inner(
    host_pipe_name: String,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    prompt_rx: &mut mpsc::UnboundedReceiver<String>,
    recommendation_rx: &mut mpsc::UnboundedReceiver<RecommendationChoice>,
    permission_rx: &mut mpsc::UnboundedReceiver<String>,
    pane_context: PaneContext,
    initial_prompt: Option<String>,
    debug_capture_enabled: Arc<AtomicBool>,
) -> Result<()> {
    let client = connect_client(&host_pipe_name)
        .await
        .with_context(|| format!("failed to connect to shared host {}", host_pipe_name))?;
    let (reader, mut writer) = tokio::io::split(client);
    let mut lines = BufReader::new(reader).lines();

    send_host_request(
        &event_tx,
        &debug_capture_enabled,
        &mut writer,
        &HostClientRequest::Attach {
            pane_context: pane_context.clone(),
        },
    )
    .await?;

    if let Some(text) = initial_prompt {
        send_host_request(
            &event_tx,
            &debug_capture_enabled,
            &mut writer,
            &HostClientRequest::SubmitPrompt {
                text,
                pane_context: Some(pane_context.clone()),
            },
        )
        .await?;
    }

    loop {
        tokio::select! {
            read = lines.next_line() => {
                match read? {
                    Some(line) => {
                        let line_len = line.len();
                        emit_debug_message(
                            &event_tx,
                            &debug_capture_enabled,
                            DebugDir::Received,
                            line.clone(),
                        );
                        let parse_started = std::time::Instant::now();
                        let message: HostServerMessage = serde_json::from_str(&line)
                            .context("failed to parse shared host message")?;
                        ui_trace::log_slow("attach_host_message_parse", parse_started.elapsed(), || {
                            format!(
                                "bytes={} message_type={}",
                                line_len,
                                host_server_message_name(&message)
                            )
                        });
                        match message {
                            HostServerMessage::Attached { snapshot, .. }
                            | HostServerMessage::SharedStateSnapshot { snapshot } => {
                                let _ = event_tx.send(AppEvent::SharedStateSnapshot(snapshot));
                            }
                            HostServerMessage::Event { event } => {
                                let _ = event_tx.send(event.into_app_event());
                            }
                            HostServerMessage::Error { message } => {
                                let _ = event_tx.send(AppEvent::SystemMessage(format!(
                                    "[host] {}",
                                    message
                                )));
                            }
                            HostServerMessage::Pong => {}
                        }
                    }
                    None => {
                        anyhow::bail!("shared host closed the connection");
                    }
                }
            }

            Some(prompt) = prompt_rx.recv() => {
                send_host_request(
                    &event_tx,
                    &debug_capture_enabled,
                    &mut writer,
                    &HostClientRequest::SubmitPrompt {
                        text: prompt,
                        pane_context: Some(pane_context.clone()),
                    },
                ).await?;
            }

            Some(choice) = recommendation_rx.recv() => {
                send_host_request(
                    &event_tx,
                    &debug_capture_enabled,
                    &mut writer,
                    &HostClientRequest::SelectRecommendation {
                        choice: choice.choice,
                    },
                ).await?;
            }

            Some(option_id) = permission_rx.recv() => {
                send_host_request(
                    &event_tx,
                    &debug_capture_enabled,
                    &mut writer,
                    &HostClientRequest::RespondPermission { option_id },
                ).await?;
            }

            else => {
                send_host_request(
                    &event_tx,
                    &debug_capture_enabled,
                    &mut writer,
                    &HostClientRequest::Detach,
                ).await.ok();
                break;
            }
        }
    }

    Ok(())
}

async fn send_host_request<W: AsyncWrite + Unpin>(
    event_tx: &mpsc::UnboundedSender<AppEvent>,
    debug_capture_enabled: &Arc<AtomicBool>,
    writer: &mut W,
    request: &HostClientRequest,
) -> Result<()> {
    let json = serde_json::to_string(request)?;
    emit_debug_message(
        event_tx,
        debug_capture_enabled,
        DebugDir::Sent,
        json.clone(),
    );
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn run_accept_loop(pipe_name: String, host_command_tx: mpsc::UnboundedSender<HostCommand>) {
    let mut server = match ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
    {
        Ok(server) => server,
        Err(err) => {
            host_log(&format!(
                "failed to create shared host pipe {}: {}",
                pipe_name, err
            ));
            return;
        }
    };

    let mut next_client_id = 1u64;
    loop {
        if let Err(err) = server.connect().await {
            host_log(&format!("shared host accept failed: {}", err));
            return;
        }

        let connected = server;
        server = match ServerOptions::new().create(&pipe_name) {
            Ok(server) => server,
            Err(err) => {
                host_log(&format!(
                    "failed to replenish shared host pipe {}: {}",
                    pipe_name, err
                ));
                return;
            }
        };

        let client_id = next_client_id;
        next_client_id += 1;
        let tx = host_command_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = run_client_connection(connected, client_id, tx).await {
                host_log(&format!(
                    "client {} disconnected with error: {:#}",
                    client_id, err
                ));
            }
        });
    }
}

async fn run_client_connection(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    client_id: u64,
    host_command_tx: mpsc::UnboundedSender<HostCommand>,
) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = BufReader::new(reader).lines();
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let mut attached = false;

    loop {
        tokio::select! {
            read = lines.next_line() => {
                match read? {
                    Some(line) => {
                        let request: HostClientRequest = serde_json::from_str(&line)
                            .context("failed to parse client host request")?;

                        match request {
                            HostClientRequest::Attach { pane_context } => {
                                attached = true;
                                let _ = host_command_tx.send(HostCommand::AttachClient {
                                    client_id,
                                    pane_context,
                                    updates: updates_tx.clone(),
                                });
                            }
                            HostClientRequest::Detach => {
                                let _ = host_command_tx.send(HostCommand::DetachClient { client_id });
                                break;
                            }
                            HostClientRequest::Ping => {
                                send_line(&mut writer, &HostServerMessage::Pong).await?;
                            }
                            other => {
                                if attached {
                                    let _ = host_command_tx.send(HostCommand::ClientRequest {
                                        client_id,
                                        request: other,
                                    });
                                } else {
                                    send_line(
                                        &mut writer,
                                        &HostServerMessage::Error {
                                            message: "attach must be sent before other host requests".to_string(),
                                        },
                                    )
                                    .await?;
                                }
                            }
                        }
                    }
                    None => {
                        break;
                    }
                }
            }

            Some(message) = updates_rx.recv() => {
                send_line(&mut writer, &message).await?;
            }

            else => break,
        }
    }

    let _ = host_command_tx.send(HostCommand::DetachClient { client_id });
    Ok(())
}

async fn run_host_service(
    mut host_command_rx: mpsc::UnboundedReceiver<HostCommand>,
    mut event_rx: mpsc::UnboundedReceiver<AppEvent>,
    prompt_tx: mpsc::UnboundedSender<PromptSubmission>,
    recommendation_tx: mpsc::UnboundedSender<RecommendationChoice>,
    wt_connected: bool,
) -> Result<()> {
    let mut clients: HashMap<u64, AttachedClient> = HashMap::new();
    let mut state = HostSessionState::new(wt_connected);

    loop {
        tokio::select! {
            Some(command) = host_command_rx.recv() => {
                handle_host_command(
                    command,
                    &mut clients,
                    &mut state,
                    &prompt_tx,
                    &recommendation_tx,
                );
            }

            Some(event) = event_rx.recv() => {
                let shared_event = SharedUiEvent::from_app_event(&event);
                state.apply_agent_event(event);
                if let Some(event) = shared_event {
                    broadcast_event(&mut clients, &event);
                } else {
                    broadcast_snapshot(&mut clients, &state.snapshot());
                }
            }

            else => break,
        }
    }

    Ok(())
}

fn handle_host_command(
    command: HostCommand,
    clients: &mut HashMap<u64, AttachedClient>,
    state: &mut HostSessionState,
    prompt_tx: &mpsc::UnboundedSender<PromptSubmission>,
    recommendation_tx: &mpsc::UnboundedSender<RecommendationChoice>,
) {
    match command {
        HostCommand::AttachClient {
            client_id,
            pane_context,
            updates,
        } => {
            clients.insert(
                client_id,
                AttachedClient {
                    pane_context,
                    updates,
                },
            );
            send_to_client(
                clients,
                client_id,
                HostServerMessage::Attached {
                    client_id,
                    snapshot: state.snapshot(),
                },
            );
        }
        HostCommand::DetachClient { client_id } => {
            clients.remove(&client_id);
        }
        HostCommand::ClientRequest { client_id, request } => match request {
            HostClientRequest::GetSnapshot => {
                send_to_client(
                    clients,
                    client_id,
                    HostServerMessage::SharedStateSnapshot {
                        snapshot: state.snapshot(),
                    },
                );
            }
            HostClientRequest::PaneContextUpdate { pane_context } => {
                if let Some(client) = clients.get_mut(&client_id) {
                    client.pane_context = pane_context;
                }
            }
            HostClientRequest::SubmitPrompt { text, pane_context } => {
                if text.trim().is_empty() {
                    return;
                }

                let effective_context = if let Some(context) = pane_context {
                    if let Some(client) = clients.get_mut(&client_id) {
                        client.pane_context = context.clone();
                    }
                    Some(context)
                } else {
                    clients
                        .get(&client_id)
                        .map(|client| client.pane_context.clone())
                };

                state.record_prompt_submission(text.clone());
                broadcast_event(clients, &SharedUiEvent::UserMessage { text: text.clone() });
                if prompt_tx
                    .send(PromptSubmission {
                        text,
                        pane_context: effective_context,
                    })
                    .is_err()
                {
                    state.push_error("agent prompt loop is unavailable".to_string());
                    broadcast_event(
                        clients,
                        &SharedUiEvent::AgentError {
                            message: "agent prompt loop is unavailable".to_string(),
                        },
                    );
                }
            }
            HostClientRequest::SelectRecommendation { choice } => {
                let maybe_choice = state
                    .recommendations
                    .as_ref()
                    .and_then(|set| set.choices.iter().find(|item| item.choice == choice))
                    .cloned();

                if let Some(selected) = maybe_choice {
                    if recommendation_tx.send(selected).is_err() {
                        state.push_system_message(
                            "recommendation executor is unavailable".to_string(),
                        );
                        broadcast_snapshot(clients, &state.snapshot());
                    }
                } else {
                    send_to_client(
                        clients,
                        client_id,
                        HostServerMessage::Error {
                            message: format!("recommendation {} is no longer available", choice),
                        },
                    );
                }
            }
            HostClientRequest::RespondPermission { option_id } => {
                if let Some(responder) = state.permission_responder.take() {
                    let _ = responder.send(option_id);
                    state.permission = None;
                    state.bump();
                    broadcast_event(clients, &SharedUiEvent::PermissionCleared);
                } else {
                    send_to_client(
                        clients,
                        client_id,
                        HostServerMessage::Error {
                            message: "no pending permission request".to_string(),
                        },
                    );
                }
            }
            HostClientRequest::Ping => {
                send_to_client(clients, client_id, HostServerMessage::Pong);
            }
            HostClientRequest::Attach { .. } | HostClientRequest::Detach => {}
        },
    }
}

async fn connect_client(
    pipe_name: &str,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    loop {
        match try_connect_client_once(pipe_name) {
            Ok(client) => return Ok(client),
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                sleep(Duration::from_millis(50)).await;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                sleep(Duration::from_millis(50)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn try_connect_client_once(
    pipe_name: &str,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    ClientOptions::new().open(pipe_name)
}

async fn send_line<W: AsyncWrite + Unpin, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let json = serde_json::to_string(value)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn emit_debug_message(
    event_tx: &mpsc::UnboundedSender<AppEvent>,
    debug_capture_enabled: &Arc<AtomicBool>,
    direction: DebugDir,
    content: String,
) {
    if !debug_capture_enabled.load(Ordering::Relaxed) {
        return;
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let _ = event_tx.send(AppEvent::DebugPipeMessage(DebugMessage {
        timestamp,
        direction,
        content,
    }));
}

fn send_to_client(
    clients: &mut HashMap<u64, AttachedClient>,
    client_id: u64,
    message: HostServerMessage,
) {
    let failed = clients
        .get(&client_id)
        .map(|client| client.updates.send(message).is_err())
        .unwrap_or(false);

    if failed {
        clients.remove(&client_id);
    }
}

fn broadcast_snapshot(clients: &mut HashMap<u64, AttachedClient>, snapshot: &SharedStateSnapshot) {
    let mut dead = Vec::new();
    for (client_id, client) in clients.iter() {
        if client
            .updates
            .send(HostServerMessage::SharedStateSnapshot {
                snapshot: snapshot.clone(),
            })
            .is_err()
        {
            dead.push(*client_id);
        }
    }

    for client_id in dead {
        clients.remove(&client_id);
    }
}

fn broadcast_event(clients: &mut HashMap<u64, AttachedClient>, event: &SharedUiEvent) {
    let mut dead = Vec::new();
    for (client_id, client) in clients.iter() {
        if client
            .updates
            .send(HostServerMessage::Event {
                event: event.clone(),
            })
            .is_err()
        {
            dead.push(*client_id);
        }
    }

    for client_id in dead {
        clients.remove(&client_id);
    }
}

fn host_log(message: &str) {
    use std::io::Write;

    if std::env::var("WTA_DEBUG_LOG").as_deref() != Ok("1") {
        return;
    }

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("wta-host-debug.log")
    {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let _ = writeln!(file, "[{:.3}] {}", timestamp, message);
        let _ = file.flush();
    }
}

fn host_server_message_name(message: &HostServerMessage) -> &'static str {
    match message {
        HostServerMessage::Attached { .. } => "attached",
        HostServerMessage::SharedStateSnapshot { .. } => "shared_state_snapshot",
        HostServerMessage::Event { .. } => "event",
        HostServerMessage::Error { .. } => "error",
        HostServerMessage::Pong => "pong",
    }
}

struct AttachedClient {
    pane_context: PaneContext,
    updates: mpsc::UnboundedSender<HostServerMessage>,
}

enum HostCommand {
    AttachClient {
        client_id: u64,
        pane_context: PaneContext,
        updates: mpsc::UnboundedSender<HostServerMessage>,
    },
    ClientRequest {
        client_id: u64,
        request: HostClientRequest,
    },
    DetachClient {
        client_id: u64,
    },
}

struct HostSessionState {
    version: u64,
    state: ConnectionState,
    agent_name: String,
    session_id: String,
    wt_connected: bool,
    messages: Vec<ChatMessage>,
    recommendations: Option<RecommendationSet>,
    agent_streaming: bool,
    pending_agent_response: String,
    prompt_in_flight: bool,
    permission: Option<PermissionPrompt>,
    permission_responder: Option<tokio::sync::oneshot::Sender<String>>,
    tool_calls: HashMap<String, (String, String)>,
}

impl HostSessionState {
    fn new(wt_connected: bool) -> Self {
        Self {
            version: 1,
            state: ConnectionState::Connecting("Starting agent...".to_string()),
            agent_name: String::new(),
            session_id: String::new(),
            wt_connected,
            messages: Vec::new(),
            recommendations: None,
            agent_streaming: false,
            pending_agent_response: String::new(),
            prompt_in_flight: false,
            permission: None,
            permission_responder: None,
            tool_calls: HashMap::new(),
        }
    }

    fn snapshot(&self) -> SharedStateSnapshot {
        SharedStateSnapshot {
            version: self.version,
            state: self.state.clone(),
            agent_name: self.agent_name.clone(),
            session_id: self.session_id.clone(),
            wt_connected: self.wt_connected,
            messages: self.messages.clone(),
            recommendations: self.recommendations.clone(),
            agent_streaming: self.agent_streaming,
            pending_agent_response: self.pending_agent_response.clone(),
            prompt_in_flight: self.prompt_in_flight,
            permission: self.permission.clone(),
        }
    }

    fn bump(&mut self) {
        self.version += 1;
    }

    fn record_prompt_submission(&mut self, text: String) {
        self.prompt_in_flight = true;
        self.agent_streaming = false;
        self.pending_agent_response.clear();
        self.recommendations = None;
        self.messages.push(ChatMessage::User(text));
        self.bump();
    }

    fn push_error(&mut self, message: String) {
        self.state = ConnectionState::Failed(message.clone());
        self.prompt_in_flight = false;
        self.agent_streaming = false;
        self.pending_agent_response.clear();
        self.messages.push(ChatMessage::Error(message));
        self.permission = None;
        self.permission_responder = None;
        self.bump();
    }

    fn push_system_message(&mut self, message: String) {
        self.messages.push(ChatMessage::System(message));
        self.bump();
    }

    fn apply_agent_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ConnectionStage(stage) => {
                self.state = ConnectionState::Connecting(stage);
                self.bump();
            }
            AppEvent::AgentConnected { name, session_id } => {
                self.agent_name = name;
                self.session_id = session_id;
                self.state = ConnectionState::Connected;
                self.bump();
            }
            AppEvent::AgentError(message) => {
                self.push_error(message);
            }
            AppEvent::AgentMessageChunk(text) => {
                self.agent_streaming = true;
                self.prompt_in_flight = true;
                self.pending_agent_response.push_str(&text);
                self.bump();
            }
            AppEvent::AgentMessageEnd => {
                self.agent_streaming = false;
                self.prompt_in_flight = false;
                self.finalize_agent_response();
                self.bump();
            }
            AppEvent::ToolCall { id, title, status } => {
                self.tool_calls
                    .insert(id.clone(), (title.clone(), status.clone()));
                self.messages
                    .push(ChatMessage::ToolCall { id, title, status });
                self.bump();
            }
            AppEvent::ToolCallUpdate { id, status } => {
                if let Some(entry) = self.tool_calls.get_mut(&id) {
                    entry.1 = status.clone();
                }
                for message in &mut self.messages {
                    if let ChatMessage::ToolCall {
                        id: mid,
                        status: current,
                        ..
                    } = message
                    {
                        if mid == &id {
                            *current = status.clone();
                        }
                    }
                }
                self.bump();
            }
            AppEvent::Plan(entries) => {
                self.messages.push(ChatMessage::Plan(entries));
                self.bump();
            }
            AppEvent::PermissionRequest {
                description,
                options,
                responder,
            } => {
                self.permission = Some(PermissionPrompt {
                    description,
                    options,
                });
                self.permission_responder = Some(responder);
                self.bump();
            }
            AppEvent::SystemMessage(message) => {
                self.push_system_message(message);
            }
            AppEvent::UserMessage(_)
            | AppEvent::SharedPermissionRequest { .. }
            | AppEvent::PermissionCleared
            |
            AppEvent::Key(_)
            | AppEvent::Resize(_, _)
            | AppEvent::DebugPipeMessage(_)
            | AppEvent::SharedStateSnapshot(_) => {}
        }
    }

    fn finalize_agent_response(&mut self) {
        if self.pending_agent_response.trim().is_empty() {
            self.pending_agent_response.clear();
            return;
        }

        let text = std::mem::take(&mut self.pending_agent_response);
        match parse_recommendation_set(&text) {
            Ok(recommendations) => {
                self.recommendations = Some(recommendations);
            }
            Err(_) => {
                self.recommendations = None;
                self.messages.push(ChatMessage::Agent(text));
            }
        }
    }
}
