use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{bail, Context};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::{mpsc, Mutex};

use crate::app::DebugMessage;
use super::types::{WireRequest, WireResponse};
use super::WtChannel;

/// Named-pipe channel to the Windows Terminal protocol server.
///
/// Connects to `\\.\pipe\WindowsTerminal-<PID>` using env var `WT_PIPE_NAME`.
/// `WT_MCP_TOKEN` is optional — if missing, sends empty string (dev bypass).
/// Debug logging to `wta-pipe-debug.log` is enabled by default.
pub struct PipeChannel {
    pipe: Mutex<tokio::net::windows::named_pipe::NamedPipeClient>,
    next_id: AtomicU64,
    available: AtomicBool,
    debug_log: Option<Mutex<std::fs::File>>,
    debug_tx: Option<mpsc::UnboundedSender<DebugMessage>>,
}

impl PipeChannel {
    /// Connect to the WT protocol server and authenticate.
    ///
    /// Reads `WT_PIPE_NAME` from environment (required).
    /// `WT_MCP_TOKEN` is optional — defaults to empty string for dev bypass.
    /// Debug log is always written to `wta-pipe-debug.log` unless `WTA_DEBUG_LOG=0`.
    pub async fn connect() -> anyhow::Result<Self> {
        let pipe_name = std::env::var("WT_PIPE_NAME")
            .context("WT_PIPE_NAME not set. Must run inside a Windows Terminal pane with protocol access.")?;
        // Token is optional for dev — empty string triggers the dev bypass in WT.
        let token = std::env::var("WT_MCP_TOKEN").unwrap_or_default();

        Self::connect_with(&pipe_name, &token).await
    }

    /// Connect to a specific pipe with an explicit name and token.
    /// This avoids needing environment variables (e.g. after VT discovery).
    pub async fn connect_with(pipe_name: &str, token: &str) -> anyhow::Result<Self> {
        let pipe = ClientOptions::new()
            .open(pipe_name)
            .context(format!("Failed to connect to pipe: {}", pipe_name))?;

        // Debug log is ON by default. Set WTA_DEBUG_LOG=0 to disable.
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

        let channel = Self {
            pipe: Mutex::new(pipe),
            next_id: AtomicU64::new(1),
            available: AtomicBool::new(false),
            debug_log,
            debug_tx: None,
        };

        channel
            .log(&format!("Connecting to {} ...", pipe_name))
            .await;

        // Authenticate (empty token triggers dev bypass on WT side)
        channel.log("Authenticating...").await;
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

    /// Read a single line from the pipe. Returns the raw string (without newline).
    /// Used by the `listen` subcommand to receive push events.
    pub async fn read_line(&self) -> anyhow::Result<String> {
        let mut pipe = self.pipe.lock().await;
        let buf = Self::read_line_raw(&mut pipe).await?;
        let line = String::from_utf8(buf)?;
        self.log(&format!("<<< {}", line)).await;
        Ok(line)
    }

    /// Read a single newline-terminated line from the pipe (caller must hold lock).
    async fn read_line_raw(
        pipe: &mut tokio::net::windows::named_pipe::NamedPipeClient,
    ) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(4096);
        loop {
            let byte = pipe.read_u8().await?;
            if byte == b'\n' {
                break;
            }
            buf.push(byte);
        }
        Ok(buf)
    }

    /// Core request implementation with full logging.
    /// Skips interleaved event messages and only returns the matching response.
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

        let mut pipe = self.pipe.lock().await;

        // Write request
        pipe.write_all(json.as_bytes()).await?;

        // Read lines until we get a response (skip interleaved events).
        loop {
            let buf = Self::read_line_raw(&mut pipe).await?;

            let line_str = String::from_utf8_lossy(&buf);
            self.log(&format!("<<< {}", line_str)).await;
            self.emit_debug(crate::app::DebugDir::Received, line_str.to_string());

            // Skip empty lines
            if buf.is_empty() {
                continue;
            }

            // Try to parse as a generic JSON to check the type field.
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) {
                if v.get("type").and_then(|t| t.as_str()) == Some("event") {
                    // This is a push event, not our response. Skip it.
                    continue;
                }
            }

            let resp: WireResponse = serde_json::from_slice(&buf)
                .context("Failed to parse response from Windows Terminal")?;

            if let Some(err) = resp.error {
                bail!("WT protocol error [{}]: {}", err.code, err.message);
            }

            return Ok(resp.result.unwrap_or(serde_json::Value::Null));
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
