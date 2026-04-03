use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{bail, Context};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::{mpsc, Mutex};

use crate::app::DebugMessage;
use super::types::{WireRequest, WireResponse};
use super::WtChannel;

/// Named-pipe channel to the Windows Terminal protocol server.
///
/// ## Architecture
///
/// Before `start_reader()` is called, the pipe is used in a simple
/// sequential mode (write request → read response). This is used for
/// authentication during connection.
///
/// After `start_reader()`, the pipe is split into read/write halves:
/// - A background task owns the read half and continuously reads lines.
///   Events (`"type": "event"`) are forwarded to the event subscriber.
///   RPC responses are sent to an internal channel.
/// - `request_inner()` writes to the write half and awaits its response
///   from the internal channel.
///
/// This allows push events to be received in real time between RPC calls.
pub struct PipeChannel {
    /// Before split: holds the full pipe. After split: None.
    unsplit_pipe: Mutex<Option<NamedPipeClient>>,
    /// After split: holds the write half.
    write_half: Mutex<Option<WriteHalf<NamedPipeClient>>>,
    next_id: AtomicU64,
    available: AtomicBool,
    debug_log: Option<Mutex<std::fs::File>>,
    debug_tx: Option<mpsc::UnboundedSender<DebugMessage>>,
    /// Sender for forwarding push events to the TUI.
    event_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<serde_json::Value>>>,
    /// Whether the background reader has been started.
    reader_started: AtomicBool,
    /// RPC responses from the background reader.
    rpc_response_rx: Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
    rpc_response_tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl PipeChannel {
    /// Connect to the WT protocol server and authenticate.
    pub async fn connect() -> anyhow::Result<Self> {
        let pipe_name = std::env::var("WT_PIPE_NAME")
            .context("WT_PIPE_NAME not set. Must run inside a Windows Terminal pane with protocol access.")?;
        let token = std::env::var("WT_MCP_TOKEN").unwrap_or_default();
        Self::connect_with(&pipe_name, &token).await
    }

    /// Connect to a specific pipe with an explicit name and token.
    pub async fn connect_with(pipe_name: &str, token: &str) -> anyhow::Result<Self> {
        let pipe = ClientOptions::new()
            .open(pipe_name)
            .context(format!("Failed to connect to pipe: {}", pipe_name))?;

        let debug_log = if std::env::var("WTA_DEBUG_LOG").as_deref() == Ok("0") {
            None
        } else {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("wta-pipe-debug.log")
                .ok();
            file.map(Mutex::new)
        };

        let (rpc_response_tx, rpc_response_rx) = mpsc::unbounded_channel();

        let channel = Self {
            unsplit_pipe: Mutex::new(Some(pipe)),
            write_half: Mutex::new(None),
            next_id: AtomicU64::new(1),
            available: AtomicBool::new(false),
            debug_log,
            debug_tx: None,
            event_tx: std::sync::Mutex::new(None),
            reader_started: AtomicBool::new(false),
            rpc_response_rx: Mutex::new(rpc_response_rx),
            rpc_response_tx,
        };

        channel.log(&format!("Connecting to {} ...", pipe_name)).await;
        channel.log("Authenticating...").await;

        // Authenticate using sequential mode (reader not started yet).
        let result = channel
            .request_inner("authenticate", serde_json::json!({ "token": token }))
            .await
            .context("Authentication failed")?;

        let authenticated = result
            .get("authenticated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !authenticated {
            bail!("Authentication rejected by Windows Terminal");
        }

        channel.available.store(true, Ordering::Relaxed);
        channel.log("Authenticated successfully").await;
        Ok(channel)
    }

    /// Attach a debug message sender for the TUI debug panel.
    pub fn with_debug_sender(mut self, tx: mpsc::UnboundedSender<DebugMessage>) -> Self {
        self.debug_tx = Some(tx);
        self
    }

    /// Subscribe to push events from the WT protocol server.
    ///
    /// Returns a receiver that delivers all push events in real time
    /// (after `start_reader()` is called).
    pub fn subscribe_events(&self) -> mpsc::UnboundedReceiver<serde_json::Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.event_tx.lock().unwrap() = Some(tx);
        rx
    }

    /// Split the pipe and start the background reader loop.
    ///
    /// Must be called from a tokio LocalSet context (uses spawn_local).
    /// After this returns, the pipe is split and ready — push events are
    /// delivered in real time, and `request_inner()` uses the write half.
    pub async fn start_reader(self: &std::sync::Arc<Self>) {
        if self.reader_started.swap(true, Ordering::SeqCst) {
            return; // already started
        }

        // Split the pipe synchronously (before returning) so that
        // write_half is ready for request_inner() immediately.
        let pipe = {
            let mut guard = self.unsplit_pipe.lock().await;
            guard.take().expect("start_reader called but pipe already taken")
        };
        let (read_half, write_half) = io::split(pipe);
        *self.write_half.lock().await = Some(write_half);

        // Spawn only the reader loop.
        let channel = std::sync::Arc::clone(self);
        tokio::task::spawn_local(async move {
            channel.reader_loop(read_half).await;
        });
    }

    /// Background reader loop. Owns the read half and routes lines.
    async fn reader_loop(self: std::sync::Arc<Self>, mut reader: ReadHalf<NamedPipeClient>) {
        loop {
            let buf = match Self::read_line_from(&mut reader).await {
                Ok(buf) => buf,
                Err(_) => break, // pipe closed
            };

            if buf.is_empty() {
                continue;
            }

            let line_str = String::from_utf8_lossy(&buf);
            self.log(&format!("<<< {}", line_str)).await;
            self.emit_debug(crate::app::DebugDir::Received, line_str.to_string());

            // Classify: event vs RPC response.
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) {
                if v.get("type").and_then(|t| t.as_str()) == Some("event") {
                    self.log("[DIAG] reader_loop: forwarding event").await;
                    self.forward_event(v);
                    continue;
                }
            }

            // RPC response — send to request_inner().
            let _ = self.rpc_response_tx.send(buf);
        }
    }

    /// Forward a push event to the subscriber (if any).
    fn forward_event(&self, event: serde_json::Value) {
        let guard = self.event_tx.lock().unwrap();
        if let Some(ref tx) = *guard {
            match tx.send(event) {
                Ok(_) => { /* logged by caller */ }
                Err(_) => {
                    // subscriber dropped — will be logged by caller
                }
            }
        } else {
            // No subscriber — event_tx is None. This means subscribe_events() was never called
            // or wasn't called before events started arriving.
        }
    }

    fn emit_debug(&self, direction: crate::app::DebugDir, content: String) {
        if let Some(ref tx) = self.debug_tx {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let _ = tx.send(DebugMessage {
                timestamp: ts,
                direction,
                content,
            });
        }
    }

    async fn log(&self, msg: &str) {
        if let Some(ref log_file) = self.debug_log {
            use std::io::Write;
            let mut f = log_file.lock().await;
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = writeln!(f, "[{:.3}] {}", elapsed.as_secs_f64(), msg);
        }
    }

    /// Read a single line from the pipe (pre-split mode).
    /// Used by the `listen` subcommand.
    pub async fn read_line(&self) -> anyhow::Result<String> {
        let mut guard = self.unsplit_pipe.lock().await;
        let pipe = guard.as_mut().context("Pipe already split — use event subscription instead")?;
        let buf = Self::read_line_from(pipe).await?;
        let line = String::from_utf8(buf)?;
        self.log(&format!("<<< {}", line)).await;
        Ok(line)
    }

    /// Read a newline-terminated line from any AsyncRead.
    async fn read_line_from<R: AsyncReadExt + Unpin>(reader: &mut R) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(4096);
        loop {
            let byte = reader.read_u8().await?;
            if byte == b'\n' {
                break;
            }
            buf.push(byte);
        }
        Ok(buf)
    }

    /// Write raw bytes to the pipe (works in both pre-split and post-split mode).
    async fn write_bytes(&self, data: &[u8]) -> anyhow::Result<()> {
        if self.reader_started.load(Ordering::SeqCst) {
            // Post-split: use the write half.
            let mut guard = self.write_half.lock().await;
            let writer = guard.as_mut().context("Write half not available")?;
            writer.write_all(data).await?;
        } else {
            // Pre-split: use the full pipe.
            let mut guard = self.unsplit_pipe.lock().await;
            let pipe = guard.as_mut().context("Pipe not available")?;
            pipe.write_all(data).await?;
        }
        Ok(())
    }

    /// Core request implementation.
    ///
    /// Pre-split: writes then reads directly (sequential, used for auth).
    /// Post-split: writes to write half, awaits response from background reader.
    async fn request_inner(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();

        let wire_req = WireRequest {
            msg_type: "request",
            id,
            method,
            params,
        };

        let mut json = serde_json::to_string(&wire_req)?;
        self.log(&format!(">>> {}", json)).await;
        self.emit_debug(crate::app::DebugDir::Sent, json.clone());
        json.push('\n');

        if self.reader_started.load(Ordering::SeqCst) {
            // Post-split: write, then await response from background reader.
            self.write_bytes(json.as_bytes()).await?;

            let mut rx = self.rpc_response_rx.lock().await;
            let buf = rx.recv().await.context("Pipe reader closed")?;

            let resp: WireResponse = serde_json::from_slice(&buf)
                .with_context(|| format!(
                    "Failed to parse response from Windows Terminal: {}",
                    String::from_utf8_lossy(&buf)
                ))?;

            if let Some(err) = resp.error {
                bail!("WT protocol error [{}]: {}", err.code, err.message);
            }

            Ok(resp.result.unwrap_or(serde_json::Value::Null))
        } else {
            // Pre-split: write + read directly on the full pipe.
            let mut guard = self.unsplit_pipe.lock().await;
            let pipe = guard.as_mut().context("Pipe not available")?;
            pipe.write_all(json.as_bytes()).await?;

            loop {
                let buf = Self::read_line_from(pipe).await?;

                let line_str = String::from_utf8_lossy(&buf);
                self.log(&format!("<<< {}", line_str)).await;
                self.emit_debug(crate::app::DebugDir::Received, line_str.to_string());

                if buf.is_empty() {
                    continue;
                }

                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("event") {
                        self.forward_event(v);
                        continue;
                    }
                }

                let resp: WireResponse = serde_json::from_slice(&buf)
                    .with_context(|| format!(
                        "Failed to parse response from Windows Terminal: {}",
                        String::from_utf8_lossy(&buf)
                    ))?;

                if let Some(err) = resp.error {
                    bail!("WT protocol error [{}]: {}", err.code, err.message);
                }

                return Ok(resp.result.unwrap_or(serde_json::Value::Null));
            }
        }
    }
}

#[async_trait::async_trait]
impl WtChannel for PipeChannel {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.request_inner(method, params).await
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }
}
