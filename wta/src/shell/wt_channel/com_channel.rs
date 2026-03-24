use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{bail, Context};
use tokio::sync::mpsc;
use windows::core::{BSTR, GUID, HRESULT, IUnknown, Interface};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSIDFromString,
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
};

use crate::app::DebugMessage;
use super::types::{WireRequest, WireResponse};
use super::WtChannel;

// ITerminalProtocolServer interface IID — must match the C++ IDL.
// uuid(7B3F8A1E-5C2D-4E6F-9A8B-1D3E5F7A9B0C)
const IID_TERMINAL_PROTOCOL_SERVER: GUID = GUID::from_values(
    0x7B3F8A1E,
    0x5C2D,
    0x4E6F,
    [0x9A, 0x8B, 0x1D, 0x3E, 0x5F, 0x7A, 0x9B, 0x0C],
);

/// Raw COM vtable for ITerminalProtocolServer.
/// Layout: [IUnknown vtable (3 entries)] + [HandleRequest]
#[repr(C)]
struct ProtocolVtbl {
    // IUnknown
    query_interface: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const GUID,
        *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    // ITerminalProtocolServer
    handle_request: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        request_json: *const u16, // BSTR (passed by value = pointer to BSTR data)
        response_json: *mut *mut u16, // BSTR* (out param)
    ) -> HRESULT,
}

/// Thin wrapper around a raw COM interface pointer.
/// Manages the reference count via AddRef/Release.
struct ProtocolServerProxy {
    ptr: *mut core::ffi::c_void,
}

impl ProtocolServerProxy {
    /// Create from an IUnknown obtained via CoCreateInstance, then QueryInterface
    /// for ITerminalProtocolServer.
    unsafe fn from_unknown(unk: &IUnknown) -> anyhow::Result<Self> {
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = unk.query(
            &IID_TERMINAL_PROTOCOL_SERVER as *const GUID,
            &mut ptr as *mut *mut core::ffi::c_void,
        );
        hr.ok().context("QueryInterface for ITerminalProtocolServer failed")?;
        Ok(Self { ptr })
    }

    /// Call HandleRequest(BSTR, BSTR*) -> HRESULT on the COM server.
    unsafe fn handle_request(&self, request: &str) -> anyhow::Result<String> {
        let vtbl = &**(self.ptr as *const *const ProtocolVtbl);

        // Allocate BSTR for the request.
        let request_bstr = BSTR::from(request);
        let mut response_ptr: *mut u16 = std::ptr::null_mut();

        let hr = (vtbl.handle_request)(
            self.ptr,
            request_bstr.as_ptr(),
            &mut response_ptr,
        );
        hr.ok().context("HandleRequest COM call failed")?;

        // Take ownership of the returned BSTR.
        let response_bstr = BSTR::from_raw(response_ptr);
        Ok(response_bstr.to_string())
    }
}

impl Drop for ProtocolServerProxy {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                let vtbl = &**(self.ptr as *const *const ProtocolVtbl);
                (vtbl.release)(self.ptr);
            }
        }
    }
}

// SAFETY: COM proxies obtained via CLSCTX_LOCAL_SERVER with automation marshaling
// handle cross-thread/cross-apartment calls transparently.
unsafe impl Send for ProtocolServerProxy {}
unsafe impl Sync for ProtocolServerProxy {}

/// COM-based channel to the Windows Terminal protocol server.
///
/// Connects via `CoCreateInstance(CLSCTX_LOCAL_SERVER)` using the CLSID
/// from the `WT_COM_CLSID` environment variable.
pub struct ComChannel {
    server: ProtocolServerProxy,
    next_id: AtomicU64,
    available: AtomicBool,
    debug_tx: Option<mpsc::UnboundedSender<DebugMessage>>,
}

// SAFETY: ProtocolServerProxy is Send+Sync, atomics are Send+Sync.
unsafe impl Send for ComChannel {}
unsafe impl Sync for ComChannel {}

impl ComChannel {
    /// Connect to the WT protocol server via COM and authenticate.
    ///
    /// Reads `WT_COM_CLSID` from environment (required).
    /// `WT_MCP_TOKEN` is optional — defaults to empty string for dev bypass.
    pub async fn connect() -> anyhow::Result<Self> {
        let clsid_str = std::env::var("WT_COM_CLSID")
            .context("WT_COM_CLSID not set. Must run inside a Windows Terminal pane with COM protocol access.")?;
        let token = std::env::var("WT_MCP_TOKEN").unwrap_or_default();

        Self::connect_with(&clsid_str, &token).await
    }

    /// Connect to a specific COM server with an explicit CLSID and token.
    pub async fn connect_with(clsid_str: &str, token: &str) -> anyhow::Result<Self> {
        let token = token.to_string();
        let clsid_str = clsid_str.to_string();

        // COM calls must happen on a thread with COM initialized.
        let server = tokio::task::spawn_blocking(move || -> anyhow::Result<ProtocolServerProxy> {
            unsafe {
                // Initialize COM (MTA — the proxy/stub DLL handles marshaling to WT's STA).
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

                // Parse the CLSID string (e.g. "{D5B7C9E1-4F6A-4B8C-D9E0-F1A2B3C4D5E6}")
                let clsid_wide: Vec<u16> = clsid_str.encode_utf16().chain(std::iter::once(0)).collect();
                let clsid = CLSIDFromString(windows::core::PCWSTR(clsid_wide.as_ptr()))
                    .context(format!("Invalid CLSID: {}", clsid_str))?;

                // CoCreateInstance returns IUnknown; we QI for our interface.
                let unk: IUnknown = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)
                    .map_err(|e| anyhow::anyhow!(
                        "CoCreateInstance({}) failed: HRESULT 0x{:08X} ({})",
                        clsid_str, e.code().0 as u32, e.message()
                    ))?;

                ProtocolServerProxy::from_unknown(&unk)
            }
        }).await??;

        let channel = Self {
            server,
            next_id: AtomicU64::new(1),
            available: AtomicBool::new(false),
            debug_tx: None,
        };

        // Authenticate (empty token triggers dev bypass on WT side)
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

    /// Core request implementation.
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

        let json = serde_json::to_string(&wire_req)?;
        self.emit_debug(crate::app::DebugDir::Sent, json.clone());

        // COM calls cross the process boundary — use the proxy directly.
        // The automation marshaler handles cross-thread dispatch.
        let resp_str = unsafe {
            self.server.handle_request(&json)?
        };

        self.emit_debug(crate::app::DebugDir::Received, resp_str.clone());

        let resp: WireResponse = serde_json::from_str(&resp_str)
            .context("Failed to parse response from Windows Terminal")?;

        if let Some(err) = resp.error {
            bail!("WT protocol error [{}]: {}", err.code, err.message);
        }

        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait::async_trait]
impl WtChannel for ComChannel {
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
