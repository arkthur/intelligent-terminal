# Agent Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-WTA TUI view that lists live and historical CLI agent sessions (Claude Code, Copilot CLI, Gemini CLI), shows their status, and lets the user focus a live pane or resume a past session.

**Architecture:** Forward-port PR #11's SessionId(Guid) refactor and `wt-agent-hooks` plugin, then add an `AgentSessionRegistry` (in-WTA model) fed by hook events + WT `connection_state` events, rendered by a new `Agents` view in the existing ratatui TUI.

**Tech Stack:** Rust 2021 (`wta`), C++/WinRT (Windows Terminal protocol), ratatui 0.30, JSON-RPC over COM (`IProtocolServer`), Bash + PowerShell hook scripts.

**Reference:** `docs/superpowers/specs/2026-05-02-agent-management-design.md`

---

## File Structure

### Modified files (M1 — SessionId refactor)
- `src/cascadia/TerminalProtocol/TerminalProtocol.idl` — rename `UInt32 PaneId` → `Guid SessionId` everywhere; preserve current additions (`Cwd`, `HasMarks`, `FocusPane`)
- `src/cascadia/WindowsTerminal/TerminalProtocolComServer.h`, `.cpp` — propagate signature change; bump `GetCapabilities` version `1.1`→`1.2`
- `src/cascadia/TerminalApp/TerminalPage.Protocol.h`, `.cpp` — replace `FindPaneById(uint32_t)` with `FindPaneBySessionId(winrt::guid)`
- `src/cascadia/TerminalApp/TerminalPage.h`, `.cpp` — header signatures + autofix listener uses pane Guid
- `src/cascadia/TerminalApp/Pane.h`, `.cpp` — add `FindPaneBySessionId` (PR-11 commit `e04eb50cd`)
- `src/tools/wtcli/main.cpp` — flags `--pane-id` → `--session-id` with deprecated alias
- `wta/src/app.rs` — `pane_id: String` → `session_id: String` in event payloads + tests

### Created files (M2 — Hooks plugin)
- `wta/agent-hooks-plugin/plugin.json`
- `wta/agent-hooks-plugin/hooks/hooks.json`
- `wta/agent-hooks-plugin/hooks/run-hook.cmd`
- `wta/agent-hooks-plugin/hooks/send-event.ps1`
- `wta/agent-hooks-plugin/hooks/pre-tool-use`
- `wta/agent-hooks-plugin/hooks/post-tool-use`
- `wta/agent-hooks-plugin/hooks/notification`
- `wta/agent-hooks-plugin/hooks/session-stop`
- `wta/agent-hooks-plugin/README.md`
- `wta/agent-hooks-plugin/doc/agent-hooks-plugin.md`

(All cherry-picked from PR #11 then patched to forward `agent_session_id`.)

### Created files (M3 — Model + sources)
- `wta/src/agent_sessions.rs` — module containing `AgentSessionRegistry`, `AgentSession`, `AgentStatus`, `CliSource`, `SessionEvent`. (Note: cannot reuse the name `agent_registry.rs` — that file already exists and holds the CLI **profile catalog**.)

### Modified files (M3)
- `wta/src/app.rs` — own an `AgentSessionRegistry`; route classified events to it; cache WTA-self pane Guid
- `wta/src/main.rs` — `mod agent_sessions;`

### Created files (M4 — TUI)
- `wta/src/ui/agents_view.rs` — render the agents list

### Modified files (M4)
- `wta/src/app.rs` — add `View::Agents` variant, Tab/F2 dispatch, Enter handlers (focus / resume), Delete handler
- `wta/src/ui/mod.rs` — `pub mod agents_view;`

### Created files (M5 — Gemini extension support)
- `wta/agent-hooks-plugin/gemini-extension.json` — Gemini extension manifest
- `wta/agent-hooks-plugin/gemini-hooks/hooks.json` — Gemini-specific hooks config (uses `${extensionPath}` and Gemini event names: `SessionStart`, `SessionEnd`, `BeforeTool`, `AfterTool`, `Notification`)

### Modified files (M5)
- `wta/agent-hooks-plugin/README.md` — add Gemini install instructions
- `wta/agent-hooks-plugin/doc/agent-hooks-plugin.md` — document Gemini event mapping
- `wta/src/agent_registry.rs` — verify Gemini's `resume_flag` matches its actual CLI support

---

## Milestone 1 — SessionId refactor (squash cherry-pick from PR #11)

This milestone is a refactor port. Its tasks are not TDD-shaped because the work is "make the existing autofix tests pass after the type rename."

### Task 1.1: Squash cherry-pick PR #11 batch 1 onto current branch

**Files:** all files listed in M1 above, modified together in one commit.

- [ ] **Step 1: Create the integration branch**

```bash
git checkout dev/yuazha/session
git pull --ff-only
git checkout -b dev/yuazha/agent-management
```

- [ ] **Step 2: Stage the squashed PR-11 batch into the index without committing**

```bash
git cherry-pick -n adac19617^..8a0a0ab87
```

Expected: git reports conflicts in roughly these files (from PR-11 diff analysis):

```
src/cascadia/TerminalProtocol/TerminalProtocol.idl
src/cascadia/WindowsTerminal/TerminalProtocolComServer.h
src/cascadia/WindowsTerminal/TerminalProtocolComServer.cpp
src/cascadia/TerminalApp/TerminalPage.Protocol.h
src/cascadia/TerminalApp/TerminalPage.Protocol.cpp
src/cascadia/TerminalApp/TerminalPage.h
src/cascadia/TerminalApp/TerminalPage.cpp
src/cascadia/TerminalApp/Pane.h
src/cascadia/TerminalApp/Pane.cpp
src/tools/wtcli/main.cpp
wta/src/app.rs
```

- [ ] **Step 3: Resolve `TerminalProtocol.idl`**

Rule: keep current-branch fields (`Cwd`, `HasMarks`, `FocusPane`); apply PR-11 rename to all of them. Resulting structs/methods (the parts that change):

```idl
struct PaneInfo
{
    Guid SessionId;          // <- was UInt32 PaneId
    UInt32 TabId;
    UInt64 WindowId;
    String Title;
    String Profile;
    Boolean IsActive;
    Boolean IsAgentPane;
    UInt32 Pid;
    Int32 Rows;
    Int32 Columns;
    String Cwd;              // kept from current branch
};

struct PaneOutput
{
    Guid SessionId;          // <- was UInt32 PaneId
    String Content;
    Int32 LineCount;
    Boolean Truncated;
    Boolean HasMarks;        // kept from current branch
};

struct ProcessStatus    { Guid SessionId; /* rest unchanged */ };
struct SessionVariable  { Guid SessionId; /* rest unchanged */ };
struct TabCreationResult { UInt32 TabId; Guid SessionId; UInt64 WindowId; UInt32 Pid; };

interface IProtocolServer
{
    // ...queries/mutations: every UInt32 paneId parameter becomes Guid sessionId
    PaneOutput      ReadPaneOutput(Guid sessionId, String source, Int32 maxLines);
    ProcessStatus   GetProcessStatus(Guid sessionId);
    SessionVariable GetSessionVariable(Guid sessionId, String name);

    TabCreationResult SplitPane(Guid sessionId, String direction, Single size, String profile, String commandline, Boolean background);
    void ClosePane(Guid sessionId);
    void SendInput(Guid sessionId, String text);
    void FocusPane(Guid sessionId);            // kept from current branch, param renamed
    void SetSessionVariable(Guid sessionId, String name, String value);
}
```

Resolve and `git add src/cascadia/TerminalProtocol/TerminalProtocol.idl`.

- [ ] **Step 4: Resolve `TerminalProtocolComServer.h` and `.cpp`**

For every method whose IDL signature changed, change the C++ implementation to take `winrt::guid sessionId` instead of `uint32_t paneId`. The body change is uniformly:

```cpp
// before
auto pane = page.FindPaneById(paneId);
// after
auto pane = page.FindPaneBySessionId(sessionId);
```

In `GetCapabilities()` (or wherever the protocol version constant lives — search for `"1.1"` first, then `protocol_version`), bump the version string from `"1.1"` to `"1.2"`.

For `FocusPane(...)` (added on the current branch, not present in PR #11), apply the same rename: `winrt::guid sessionId`. Its body looks up the pane and calls `_FocusPane(pane)`.

`git add src/cascadia/WindowsTerminal/TerminalProtocolComServer.{h,cpp}`.

- [ ] **Step 5: Resolve `TerminalPage.Protocol.h` and `.cpp`**

Every signature in the `Protocol.h`/`Protocol.cpp` that took `uint32_t paneId` becomes `const winrt::guid& sessionId`. Every `_root->FindPaneById(paneId)` call becomes `_root->FindPaneBySessionId(sessionId)`. The autofix VT-sequence event (`ProtocolVtSequenceReceived`) emits the pane's Guid (`pane->GetTermControl().Connection().SessionId()`), not its `_id`.

`git add src/cascadia/TerminalApp/TerminalPage.Protocol.{h,cpp}`.

- [ ] **Step 6: Resolve `TerminalPage.h` and `.cpp`**

Header: forward-declare `winrt::guid` if needed, update method signatures. CPP: keep `_ensurePageEventsRegistered` and the `ProtocolVtSequenceReceived` listener wiring intact (current-branch additions); just thread Guid where `uint32_t` was used.

`git add src/cascadia/TerminalApp/TerminalPage.{h,cpp}`.

- [ ] **Step 7: Resolve `Pane.h` and `Pane.cpp`**

Add `FindPaneBySessionId` exactly as in PR-11 commit `e04eb50cd`:

```cpp
// Pane.h, near FindPaneByContentId declaration:
std::shared_ptr<Pane> FindPaneBySessionId(const winrt::guid& sessionId);

// Pane.cpp, after FindPaneByContentId definition:
std::shared_ptr<Pane> Pane::FindPaneBySessionId(const winrt::guid& sessionId)
{
    if (sessionId == winrt::guid{})
    {
        return nullptr;
    }
    return _FindPane([&](const auto& p) {
        if (!p->_IsLeaf() || !p->_content)
            return false;
        if (const auto termContent = p->_content.try_as<winrt::TerminalApp::TerminalPaneContent>())
        {
            if (const auto control = termContent.GetTermControl())
            {
                if (const auto conn = control.Connection())
                {
                    return conn.SessionId() == sessionId;
                }
            }
        }
        return false;
    });
}
```

`git add src/cascadia/TerminalApp/Pane.{h,cpp}`.

- [ ] **Step 8: Resolve `src/tools/wtcli/main.cpp`**

Every clap-equivalent flag (`--pane-id`) becomes `--session-id`. Keep `--pane-id` as a hidden deprecated alias that accepts a Guid string for one release. The JSON output of `list-panes` uses `"session_id"` key. `send-event` continues to take `-e <event_type>` and a payload string; the COM server attaches the caller pane's Guid automatically.

`git add src/tools/wtcli/main.cpp`.

- [ ] **Step 9: Resolve `wta/src/app.rs`**

Only one structural change: every `pane_id: String` field/argument becomes `session_id: String` (Guid in text form). Tests in the `tests` module that build params with `"pane_id": "3"` change to `"session_id": "00000000-0000-0000-0000-000000000003"` (any well-formed Guid is fine).

The `classify_wt_event` signature changes:
```rust
pub fn classify_wt_event(method: &str, session_id: &str, params: &serde_json::Value) -> WtNotification
```
Update each test:

```rust
#[test]
fn classify_connection_failed_is_critical() {
    let params = json!({"session_id": "00000000-0000-0000-0000-000000000003", "state": "failed"});
    let n = classify_wt_event("connection_state",
                              "00000000-0000-0000-0000-000000000003",
                              &params);
    assert_eq!(n.severity, WtEventSeverity::Critical);
    assert!(n.summary.contains("failed"));
}
```

Apply the same to all classify-tests (~7 tests). The `WtEvent` struct in `event.rs` may also need its `pane_id` field renamed — follow the compile errors.

`git add wta/src/app.rs wta/src/event.rs` (and any other Rust files the compiler points to).

- [ ] **Step 10: Run the C++ build and confirm it compiles**

```cmd
cmd.exe /c "tools\razzle.cmd && bcz no_clean"
```

Expected: build succeeds. If it doesn't, the error is almost certainly a missed call site — fix and re-run.

- [ ] **Step 11: Run the WTA Rust build and tests**

```bash
Get-Process wta -ErrorAction SilentlyContinue | Stop-Process -Force
cargo build --manifest-path wta/Cargo.toml
cargo test --manifest-path wta/Cargo.toml --lib classify_
```

Expected: `classify_*` tests pass with the renamed fields.

- [ ] **Step 12: Commit the squash**

```bash
git commit -m "refactor: switch protocol identity from PaneId(uint32) to SessionId(Guid)

Forward-ports PR #11 commits adac19617..8a0a0ab87 squashed onto
current HEAD. Manual conflict resolution because PR #11 base lags
the current branch by ~1 month (post #12 merge, FocusPane,
autofix, xaml titlebar).

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Milestone 2 — Hooks plugin + agent_session_id enrichment

### Task 2.1: Squash cherry-pick PR #11 batch 2 (hooks plugin)

**Files:** All files in `wta/agent-hooks-plugin/` (created).

- [ ] **Step 1: Cherry-pick the hooks plugin commits**

```bash
git cherry-pick -n 281495e1c^..7392e311d
```

Expected: minimal conflicts — these commits add new files under `wta/agent-hooks-plugin/`. There may be a small conflict in `wta/src/app.rs` if the agent-event handling arm overlaps with M1 changes; resolve by keeping both: M1's renamed `session_id` field plus PR-11's `agent_event` match arm.

- [ ] **Step 2: Verify the plugin tree exists**

```bash
ls wta/agent-hooks-plugin/hooks/
```

Expected output:
```
hooks.json   notification   post-tool-use   pre-tool-use   run-hook.cmd   send-event.ps1   session-stop
```

- [ ] **Step 3: Build and run the existing app.rs tests**

```bash
cargo test --manifest-path wta/Cargo.toml --lib agent_event
```

Expected: existing PR-11 tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(wta): add agent-hooks plugin (forward-port from PR #11)

Forward-ports PR #11 commits 281495e1c..7392e311d. The plugin
registers Copilot CLI hooks (PreToolUse, PostToolUse, Notification,
Stop) and forwards each event to WTA via wtcli send-event.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2.2: Enrich hook payloads with `agent_session_id`

**Files:**
- Modify: `wta/agent-hooks-plugin/hooks/send-event.ps1`
- Modify: `wta/agent-hooks-plugin/hooks/pre-tool-use`
- Modify: `wta/agent-hooks-plugin/hooks/post-tool-use`
- Modify: `wta/agent-hooks-plugin/hooks/notification`
- Modify: `wta/agent-hooks-plugin/hooks/session-stop`

Goal: every event sent to `wtcli send-event` includes a top-level
`agent_session_id` field, sourced from stdin JSON (Claude/Gemini) or env
(Copilot), with empty-string fallback when unavailable.

- [ ] **Step 1: Patch `send-event.ps1` to add `agent_session_id`**

Current contents (PR-11) wrap `parsed` under `payload`. Add an explicit
`agent_session_id` extraction:

```powershell
# send-event.ps1 — Forward Copilot CLI hook events to WTA via wtcli
param([string]$EventType = "agent.hook")

if (-not $env:WT_COM_CLSID) { exit 0 }
$wtcliPath = (Get-Command wtcli -ErrorAction SilentlyContinue).Source
if (-not $wtcliPath) { exit 0 }

$hookData = [Console]::In.ReadToEnd()
if (-not $hookData -or -not $hookData.Trim()) { exit 0 }

try {
    $parsed = $hookData | ConvertFrom-Json

    # Extract agent_session_id from known fields, env, or empty fallback.
    $agentSessionId = ""
    if ($parsed.PSObject.Properties.Name -contains "session_id") {
        $agentSessionId = [string]$parsed.session_id
    } elseif ($env:COPILOT_SESSION_ID) {
        $agentSessionId = $env:COPILOT_SESSION_ID
    } elseif ($env:CLAUDE_SESSION_ID) {
        $agentSessionId = $env:CLAUDE_SESSION_ID
    } elseif ($env:GEMINI_SESSION_ID) {
        $agentSessionId = $env:GEMINI_SESSION_ID
    }

    $payload = @{
        cli_source       = $env:WTA_CLI_SOURCE
        agent_session_id = $agentSessionId
        payload          = $parsed
    }
    if (-not $payload.cli_source) { $payload.cli_source = "copilot" }

    $json = $payload | ConvertTo-Json -Compress -Depth 5
    $escaped = $json.Replace('"', '\"')

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $wtcliPath
    $psi.Arguments = "send-event -e $EventType `"$escaped`""
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.WaitForExit(5000)
} catch {
    # Silently ignore errors — hooks must not block the agent.
}
```

`WTA_CLI_SOURCE` is the env var the bash hook scripts already set before invoking PowerShell.

- [ ] **Step 2: Patch each bash hook script to set `WTA_CLI_SOURCE`**

For each of `pre-tool-use`, `post-tool-use`, `notification`, `session-stop`, locate the line that detects the CLI (typically the `case` or `if` on `COPILOT_CLI` / `CLAUDE_PLUGIN_ROOT` / `GEMINI_CLI`) and ensure the result is `export`ed so PowerShell sees it:

```bash
# inside each hook script, after CLI_SOURCE is set:
export WTA_CLI_SOURCE="$CLI_SOURCE"
```

If the script invokes `send-event.ps1` via `pwsh -File ...`, the exported variable is inherited.

- [ ] **Step 3: Manual smoke-test: Claude session start**

```bash
# In a fresh WT pane:
echo '{"session_id":"abc-123","tool_name":"bash","tool_input":{"command":"ls"}}' \
    | bash wta/agent-hooks-plugin/hooks/pre-tool-use
```

Then in a second pane running `wtcli listen --json`, confirm a JSON record arrives with `"agent_session_id":"abc-123"` in the payload.

- [ ] **Step 4: Commit**

```bash
git add wta/agent-hooks-plugin/
git commit -m "feat(hooks): include agent_session_id in forwarded payloads

Each hook now extracts the agent's own session id from stdin JSON
(Claude/Gemini) or env (Copilot) and includes it as a top-level
agent_session_id field in the event sent to WTA. Empty string when
unavailable; receivers fall back to pane-Guid keyed entries.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Milestone 3 — AgentSessionRegistry model + sources wiring

This milestone is TDD throughout. Each event variant gets one failing test
and one minimal implementation step, in order.

### Task 3.1: Create `agent_sessions` module skeleton

**Files:**
- Create: `wta/src/agent_sessions.rs`
- Modify: `wta/src/main.rs` (add `mod agent_sessions;`)

- [ ] **Step 1: Create the module file with type definitions**

```rust
// wta/src/agent_sessions.rs
//
// Runtime registry for tracking live and historical CLI agent sessions.
// Independent from `agent_registry.rs`, which is the static catalog of
// CLI profiles.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

pub type AgentKey = String;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CliSource {
    Claude,
    Copilot,
    Gemini,
    Unknown(String),
}

impl CliSource {
    pub fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("").to_ascii_lowercase().as_str() {
            "claude"  => Self::Claude,
            "copilot" => Self::Copilot,
            "gemini"  => Self::Gemini,
            ""        => Self::Unknown(String::new()),
            other     => Self::Unknown(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Idle,
    Working,
    Attention,
    Error,
    Ended,
    Historical,
}

#[derive(Clone, Debug)]
pub struct AgentSession {
    pub key:               AgentKey,
    pub cli_source:        CliSource,
    pub pane_session_id:   Option<String>,    // Guid as text form
    pub window_id:         Option<u64>,
    pub tab_id:            Option<u32>,
    pub title:             String,
    pub cwd:               PathBuf,
    pub started_at:        SystemTime,
    pub last_activity_at:  SystemTime,
    pub status:            AgentStatus,
    pub last_error:        Option<String>,
    pub current_tool:      Option<String>,
    pub attention_reason:  Option<String>,
    pub log_path:          Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    SessionStarted   { key: AgentKey, cli_source: CliSource, pane_session_id: String, cwd: PathBuf, title: String },
    ToolStarting     { key: AgentKey, tool_name: String },
    ToolCompleted    { key: AgentKey },
    Notification     { key: AgentKey, message: String },
    SessionStopped   { key: AgentKey, reason: String },
    ConnectionFailed { pane_session_id: String, reason: String },
    PaneClosed       { pane_session_id: String },
}

#[derive(Default)]
pub struct AgentSessionRegistry {
    sessions:        HashMap<AgentKey, AgentSession>,
    active_by_pane:  HashMap<String, AgentKey>,   // pane Guid (text) -> AgentKey
    dirty:           bool,
}

impl AgentSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, _ev: SessionEvent) {
        // implemented in subsequent tasks
        unimplemented!()
    }

    pub fn iter_sorted(&self) -> Vec<&AgentSession> {
        let mut v: Vec<_> = self.sessions.values().collect();
        v.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
        v
    }

    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }
}
```

- [ ] **Step 2: Register the module**

In `wta/src/main.rs`, add (alphabetically near `mod agent_registry;`):

```rust
mod agent_sessions;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build --manifest-path wta/Cargo.toml
```

Expected: builds (the `unimplemented!()` is fine — only triggers if called).

- [ ] **Step 4: Commit**

```bash
git add wta/src/agent_sessions.rs wta/src/main.rs
git commit -m "feat(wta): scaffold AgentSessionRegistry types

Core types (CliSource, AgentStatus, AgentSession, SessionEvent,
AgentSessionRegistry) with stub apply(). Subsequent commits add
each event variant via TDD.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.2: Implement `SessionStarted`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing test**

Append to `wta/src/agent_sessions.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn k(s: &str) -> AgentKey { s.to_string() }
    fn pane(s: &str) -> String { s.to_string() }

    #[test]
    fn session_started_creates_idle_entry_bound_to_pane() {
        let mut reg = AgentSessionRegistry::new();
        reg.apply(SessionEvent::SessionStarted {
            key: k("sid-1"),
            cli_source: CliSource::Claude,
            pane_session_id: pane("00000000-0000-0000-0000-000000000001"),
            cwd: PathBuf::from("/work/proj"),
            title: "claude — proj".to_string(),
        });

        let s = reg.sessions.get("sid-1").expect("session created");
        assert_eq!(s.status, AgentStatus::Idle);
        assert_eq!(s.cli_source, CliSource::Claude);
        assert_eq!(s.pane_session_id.as_deref(), Some("00000000-0000-0000-0000-000000000001"));
        assert_eq!(reg.active_by_pane.get("00000000-0000-0000-0000-000000000001"), Some(&k("sid-1")));
        assert!(reg.take_dirty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib session_started_creates_idle_entry_bound_to_pane
```

Expected: FAIL — `apply` panics with `unimplemented!`.

- [ ] **Step 3: Implement `SessionStarted` in `apply`**

Replace the body of `AgentSessionRegistry::apply`:

```rust
pub fn apply(&mut self, ev: SessionEvent) {
    let now = SystemTime::now();
    match ev {
        SessionEvent::SessionStarted { key, cli_source, pane_session_id, cwd, title } => {
            let entry = self.sessions.entry(key.clone()).or_insert_with(|| AgentSession {
                key:               key.clone(),
                cli_source:        cli_source.clone(),
                pane_session_id:   None,
                window_id:         None,
                tab_id:            None,
                title:             title.clone(),
                cwd:               cwd.clone(),
                started_at:        now,
                last_activity_at:  now,
                status:            AgentStatus::Idle,
                last_error:        None,
                current_tool:      None,
                attention_reason:  None,
                log_path:          None,
            });
            entry.cli_source       = cli_source;
            entry.title            = title;
            entry.cwd              = cwd;
            entry.pane_session_id  = Some(pane_session_id.clone());
            entry.status           = AgentStatus::Idle;
            entry.last_error       = None;
            entry.attention_reason = None;
            entry.current_tool     = None;
            entry.last_activity_at = now;
            self.active_by_pane.insert(pane_session_id, key);
            self.dirty = true;
        }
        _ => unimplemented!()
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib session_started_creates_idle_entry_bound_to_pane
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): apply SessionStarted

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.3: Implement `ToolStarting` / `ToolCompleted`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing tests**

Add inside the `tests` module:

```rust
#[test]
fn tool_starting_transitions_idle_to_working() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::ToolStarting { key: k("s"), tool_name: "bash".into() });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Working);
    assert_eq!(s.current_tool.as_deref(), Some("bash"));
}

#[test]
fn tool_completed_returns_working_to_idle() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::ToolStarting   { key: k("s"), tool_name: "bash".into() });
    reg.apply(SessionEvent::ToolCompleted  { key: k("s") });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Idle);
    assert!(s.current_tool.is_none());
}

#[test]
fn tool_completed_does_not_demote_attention_or_error() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    // simulate Notification arriving before tool completes:
    reg.sessions.get_mut("s").unwrap().status = AgentStatus::Attention;
    reg.apply(SessionEvent::ToolCompleted { key: k("s") });
    assert_eq!(reg.sessions.get("s").unwrap().status, AgentStatus::Attention);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --manifest-path wta/Cargo.toml --lib tool_
```

Expected: FAIL with `unimplemented!`.

- [ ] **Step 3: Implement both arms**

Replace the catch-all `_ => unimplemented!()` arm in `apply` with:

```rust
SessionEvent::ToolStarting { key, tool_name } => {
    if let Some(entry) = self.sessions.get_mut(&key) {
        entry.status            = AgentStatus::Working;
        entry.current_tool      = Some(tool_name);
        entry.last_activity_at  = now;
        self.dirty = true;
    }
}

SessionEvent::ToolCompleted { key } => {
    if let Some(entry) = self.sessions.get_mut(&key) {
        if entry.status == AgentStatus::Working {
            entry.status        = AgentStatus::Idle;
        }
        entry.current_tool      = None;
        entry.last_activity_at  = now;
        self.dirty = true;
    }
}

_ => unimplemented!()
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --manifest-path wta/Cargo.toml --lib tool_
```

Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): apply ToolStarting/ToolCompleted

ToolCompleted only demotes from Working — preserves Attention and
Error so concurrent Notification or ConnectionFailed events are
not clobbered by a delayed PostToolUse.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.4: Implement `Notification`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn notification_sets_attention_with_reason() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::Notification {
        key: k("s"),
        message: "approve: rm -rf foo".into(),
    });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Attention);
    assert_eq!(s.attention_reason.as_deref(), Some("approve: rm -rf foo"));
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib notification_sets_attention
```

Expected: FAIL.

- [ ] **Step 3: Implement**

Insert before the catch-all:

```rust
SessionEvent::Notification { key, message } => {
    if let Some(entry) = self.sessions.get_mut(&key) {
        entry.status            = AgentStatus::Attention;
        entry.attention_reason  = Some(message);
        entry.last_activity_at  = now;
        self.dirty = true;
    }
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib notification_sets_attention
```

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): apply Notification

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.5: Implement `SessionStopped` and `PaneClosed`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn session_stopped_marks_ended_and_unbinds_pane() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::SessionStopped { key: k("s"), reason: "user_exit".into() });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Ended);
    assert!(s.pane_session_id.is_none());
    assert!(reg.active_by_pane.is_empty());
}

#[test]
fn pane_closed_marks_active_session_ended() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::PaneClosed { pane_session_id: pane("p") });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Ended);
    assert!(s.pane_session_id.is_none());
    assert!(reg.active_by_pane.is_empty());
}

#[test]
fn pane_closed_for_unknown_pane_is_noop() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::PaneClosed { pane_session_id: pane("ghost") });
    assert!(reg.sessions.is_empty());
    assert!(reg.active_by_pane.is_empty());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --manifest-path wta/Cargo.toml --lib session_stopped pane_closed
```

- [ ] **Step 3: Implement**

```rust
SessionEvent::SessionStopped { key, reason: _ } => {
    if let Some(entry) = self.sessions.get_mut(&key) {
        entry.status        = AgentStatus::Ended;
        if let Some(pane) = entry.pane_session_id.take() {
            self.active_by_pane.remove(&pane);
        }
        entry.current_tool      = None;
        entry.attention_reason  = None;
        entry.last_activity_at  = now;
        self.dirty = true;
    }
}

SessionEvent::PaneClosed { pane_session_id } => {
    if let Some(key) = self.active_by_pane.remove(&pane_session_id) {
        if let Some(entry) = self.sessions.get_mut(&key) {
            entry.status            = AgentStatus::Ended;
            entry.pane_session_id   = None;
            entry.current_tool      = None;
            entry.attention_reason  = None;
            entry.last_activity_at  = now;
            self.dirty = true;
        }
    }
}
```

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test --manifest-path wta/Cargo.toml --lib pane_closed session_stopped
```

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): apply SessionStopped/PaneClosed

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.6: Implement `ConnectionFailed`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn connection_failed_sets_error_with_reason() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: k("s"), cli_source: CliSource::Claude,
        pane_session_id: pane("p"), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    reg.apply(SessionEvent::ConnectionFailed {
        pane_session_id: pane("p"),
        reason: "ECONNRESET".into(),
    });
    let s = reg.sessions.get("s").unwrap();
    assert_eq!(s.status, AgentStatus::Error);
    assert_eq!(s.last_error.as_deref(), Some("ECONNRESET"));
    assert!(s.pane_session_id.is_some(), "pane stays bound until PaneClosed");
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib connection_failed_sets_error
```

- [ ] **Step 3: Implement**

```rust
SessionEvent::ConnectionFailed { pane_session_id, reason } => {
    if let Some(key) = self.active_by_pane.get(&pane_session_id).cloned() {
        if let Some(entry) = self.sessions.get_mut(&key) {
            entry.status            = AgentStatus::Error;
            entry.last_error        = Some(reason);
            entry.last_activity_at  = now;
            self.dirty = true;
        }
    }
}
```

Remove the `_ => unimplemented!()` arm now that all variants are covered.

- [ ] **Step 4: Run all agent_sessions tests**

```bash
cargo test --manifest-path wta/Cargo.toml --lib agent_sessions::
```

Expected: 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): apply ConnectionFailed; remove unimplemented arm

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.7: Add placeholder-key fallback for missing `agent_session_id`

**Files:**
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn fallback_resolves_missing_id_to_pane_keyed_placeholder() {
    let reg = AgentSessionRegistry::new();
    let pane_id = "00000000-0000-0000-0000-0000000000aa";
    let key = reg.resolve_or_synthesize_key("", pane_id);
    assert_eq!(key, format!("pane:{}", pane_id));
}

#[test]
fn fallback_returns_existing_active_key_when_pane_already_known() {
    let mut reg = AgentSessionRegistry::new();
    reg.apply(SessionEvent::SessionStarted {
        key: "real".into(), cli_source: CliSource::Claude,
        pane_session_id: "p".into(), cwd: PathBuf::from("/x"),
        title: "t".into(),
    });
    let key = reg.resolve_or_synthesize_key("", "p");
    assert_eq!(key, "real");
}

#[test]
fn fallback_uses_provided_id_when_present() {
    let reg = AgentSessionRegistry::new();
    let key = reg.resolve_or_synthesize_key("explicit", "anything");
    assert_eq!(key, "explicit");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --manifest-path wta/Cargo.toml --lib fallback_
```

Expected: FAIL — method not defined.

- [ ] **Step 3: Implement**

In `impl AgentSessionRegistry`:

```rust
/// Resolve the key for an incoming hook event, falling back to a
/// pane-Guid-derived placeholder when no agent_session_id was provided.
pub fn resolve_or_synthesize_key(
    &self,
    agent_session_id: &str,
    pane_session_id: &str,
) -> AgentKey {
    if !agent_session_id.is_empty() {
        return agent_session_id.to_string();
    }
    if let Some(existing) = self.active_by_pane.get(pane_session_id) {
        return existing.clone();
    }
    format!("pane:{}", pane_session_id)
}

pub fn has_session(&self, key: &AgentKey) -> bool {
    self.sessions.contains_key(key)
}
```

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test --manifest-path wta/Cargo.toml --lib fallback_
```

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs
git commit -m "feat(agent_sessions): resolve_or_synthesize_key fallback

Used by source layer when an event lacks agent_session_id —
attaches to an existing pane-bound session if one exists,
otherwise creates a pane:<guid> placeholder.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.8: Add registry to `App` and route `agent_event` events

**Files:**
- Modify: `wta/src/app.rs`

- [ ] **Step 1: Add the registry field on `App`**

In the `App` struct definition (around line 349 of `app.rs`), add:

```rust
pub agent_sessions: crate::agent_sessions::AgentSessionRegistry,
```

In `App::new`, initialize it:

```rust
agent_sessions: crate::agent_sessions::AgentSessionRegistry::new(),
```

In `test_app()` (around line 2762), do the same.

- [ ] **Step 2: Add helper that routes agent.* events into the registry**

Above the existing `classify_wt_event` function, add the routing helper:

```rust
/// Route a parsed `agent_event` payload into the AgentSessionRegistry.
///
/// Returns `true` if the registry was updated and the UI should redraw.
pub fn route_agent_event_to_registry(
    reg: &mut crate::agent_sessions::AgentSessionRegistry,
    pane_session_id: &str,
    params: &serde_json::Value,
) -> bool {
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;

    // The COM broadcast wraps the hook payload as:
    //   { "event": "agent.tool.starting",
    //     "cli_source": "claude",
    //     "agent_session_id": "...",
    //     "payload": { ...original hook stdin... } }
    let event = params.get("event").and_then(|v| v.as_str()).unwrap_or("");
    if !event.starts_with("agent.") {
        return false;
    }

    let cli_source = CliSource::parse(params.get("cli_source").and_then(|v| v.as_str()));
    let asid       = params.get("agent_session_id").and_then(|v| v.as_str()).unwrap_or("");
    let key        = reg.resolve_or_synthesize_key(asid, pane_session_id);

    let payload = params.get("payload").cloned().unwrap_or(serde_json::Value::Null);
    let cwd = payload.get("cwd")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let cwd_label = cwd.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let title_for_synth = format!("{:?} — {}", cli_source, cwd_label);

    // Synthesize SessionStarted on first sighting since the hooks plugin
    // doesn't ship a session-start hook (PreToolUse always fires before any
    // user-visible activity).
    let session_known = reg.has_session(&key);
    let needs_synthetic_start = event != "agent.session.started" && !session_known;
    if needs_synthetic_start {
        reg.apply(SessionEvent::SessionStarted {
            key: key.clone(),
            cli_source: cli_source.clone(),
            pane_session_id: pane_session_id.to_string(),
            cwd: cwd.clone(),
            title: title_for_synth.clone(),
        });
    }

    let ev = match event {
        "agent.session.started" => SessionEvent::SessionStarted {
            key,
            cli_source,
            pane_session_id: pane_session_id.to_string(),
            cwd,
            title: title_for_synth,
        },
        "agent.tool.starting" => SessionEvent::ToolStarting {
            key,
            tool_name: payload.get("tool_name").or_else(|| payload.get("toolName"))
                .and_then(|v| v.as_str()).unwrap_or("").to_string(),
        },
        "agent.tool.completed" => SessionEvent::ToolCompleted { key },
        "agent.notification"   => SessionEvent::Notification {
            key,
            message: payload.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        },
        "agent.session.stopped" => SessionEvent::SessionStopped {
            key,
            reason: payload.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        },
        _ => return reg.take_dirty(),
    };

    reg.apply(ev);
    reg.take_dirty()
}
```

- [ ] **Step 3: Test the routing function**

Append to the existing test module in `app.rs`:

```rust
#[test]
fn route_agent_event_creates_session_on_tool_starting() {
    use crate::agent_sessions::AgentSessionRegistry;
    let mut reg = AgentSessionRegistry::new();
    let pane = "00000000-0000-0000-0000-000000000001";
    let params = serde_json::json!({
        "event": "agent.tool.starting",
        "cli_source": "claude",
        "agent_session_id": "abc",
        "payload": {"tool_name": "bash", "cwd": "/work"}
    });
    let dirty = route_agent_event_to_registry(&mut reg, pane, &params);
    assert!(dirty);
    assert!(reg.has_session(&"abc".to_string()));
}

#[test]
fn route_agent_event_falls_back_to_pane_keyed_placeholder() {
    use crate::agent_sessions::AgentSessionRegistry;
    let mut reg = AgentSessionRegistry::new();
    let pane = "00000000-0000-0000-0000-000000000001";
    let params = serde_json::json!({
        "event": "agent.tool.starting",
        "cli_source": "copilot",
        "payload": {"tool_name": "bash"}
    });
    route_agent_event_to_registry(&mut reg, pane, &params);
    assert!(reg.has_session(&format!("pane:{}", pane)));
}

#[test]
fn route_agent_event_ignores_non_agent_events() {
    use crate::agent_sessions::AgentSessionRegistry;
    let mut reg = AgentSessionRegistry::new();
    let params = serde_json::json!({"event": "something.else"});
    let dirty = route_agent_event_to_registry(&mut reg, "p", &params);
    assert!(!dirty);
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --manifest-path wta/Cargo.toml --lib route_agent_event
```

Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add wta/src/agent_sessions.rs wta/src/app.rs
git commit -m "feat(wta): route agent_event payloads into AgentSessionRegistry

Synthesizes SessionStarted on first sighting since the hooks
plugin doesn't ship a session-start hook (PreToolUse always
fires before any user-visible activity).

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3.9: Wire the route into the WT-event dispatch path

**Files:**
- Modify: `wta/src/app.rs`
- Modify: `wta/src/main.rs`

- [ ] **Step 1: Add the `self_pane_session_id` field**

On the `App` struct:

```rust
pub self_pane_session_id: Option<String>,
```

Initialize to `None` in `new()` and `test_app()`. Add a setter:

```rust
pub fn with_self_pane_session_id(mut self, id: Option<String>) -> Self {
    self.self_pane_session_id = id;
    self
}
```

In `wta/src/main.rs`, where `App::new` is called and the WTA pane Guid is currently discovered (search for the place that calls `wtcli list-panes` and matches by PID — it produces a `String` Guid), thread that value through `with_self_pane_session_id`.

- [ ] **Step 2: Find the existing WT event dispatcher**

In `app.rs`, locate the `AppEvent::WtEvent { method, session_id, params }` arm of `handle_event` (around line 1040 — where `classify_wt_event` is called). The current code calls `classify_wt_event` and then `maybe_trigger_autofix`.

- [ ] **Step 3: Add routing for `agent_event` and `connection_state` before classify**

Replace the start of that arm with:

```rust
AppEvent::WtEvent { method, session_id, params } => {
    // Filter: ignore events from the WTA pane itself.
    if Some(session_id.as_str()) == self.self_pane_session_id.as_deref() {
        return;
    }

    // Route agent.* hooks into the session registry.
    if method == "agent_event" {
        let dirty = route_agent_event_to_registry(
            &mut self.agent_sessions,
            &session_id,
            &params,
        );
        if dirty {
            self.dirty = true;
        }
        return;
    }

    // Route connection_state into the registry as well as classify_wt_event.
    if method == "connection_state" {
        use crate::agent_sessions::SessionEvent;
        let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("");
        match state {
            "failed" => {
                let reason = params.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("connection failed").to_string();
                self.agent_sessions.apply(SessionEvent::ConnectionFailed {
                    pane_session_id: session_id.clone(),
                    reason,
                });
            }
            "closed" => {
                self.agent_sessions.apply(SessionEvent::PaneClosed {
                    pane_session_id: session_id.clone(),
                });
            }
            _ => {}
        }
        if self.agent_sessions.take_dirty() {
            self.dirty = true;
        }
        // fall through to classify_wt_event for autofix
    }

    let notification = classify_wt_event(&method, &session_id, &params);
    // ...rest of existing handler unchanged...
}
```

If `App` does not have a public `dirty` flag, find the existing redraw signal (e.g. `self.needs_redraw = true` or a channel send) and use whatever the existing chat-side code uses when it wants the next frame to redraw.

- [ ] **Step 4: Run all wta tests**

```bash
cargo test --manifest-path wta/Cargo.toml --lib
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add wta/src/app.rs wta/src/main.rs
git commit -m "feat(wta): wire agent_event + connection_state into AgentSessionRegistry

Filters out events originating from the WTA pane itself using the
self_pane_session_id discovered at startup.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Milestone 4 — Agents view (TUI)

### Task 4.1: Add `View::Agents` enum variant and Tab/F2 dispatch

**Files:**
- Modify: `wta/src/app.rs`

- [ ] **Step 1: Locate or add the `View` enum**

Search `wta/src/app.rs` for the existing view-mode state. If a `View` enum already exists (e.g. for chat / debug panel), add an `Agents` variant; if not, add this:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Chat,
    Agents,
}
```

On `App`:

```rust
pub current_view: View,
```

Initialize to `View::Chat` in `App::new` and `test_app`.

- [ ] **Step 2: Write a test for the toggle**

```rust
#[test]
fn f2_toggles_between_chat_and_agents_view() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    assert_eq!(app.current_view, View::Chat);

    app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert_eq!(app.current_view, View::Agents);

    app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert_eq!(app.current_view, View::Chat);
}
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib f2_toggles
```

Expected: FAIL — current_view doesn't change.

- [ ] **Step 4: Implement the toggle in `handle_key`**

Inside `App::handle_key`, near the top before any other key matching:

```rust
match (key.code, key.modifiers) {
    (KeyCode::F(2), KeyModifiers::NONE) | (KeyCode::Tab, KeyModifiers::CONTROL) => {
        self.current_view = match self.current_view {
            View::Chat   => View::Agents,
            View::Agents => View::Chat,
        };
        self.dirty = true;
        return;
    }
    _ => {}
}
```

(Bare `Tab` is reserved for completion in chat input; using `Ctrl+Tab` plus `F2` is safer.)

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib f2_toggles
```

- [ ] **Step 6: Commit**

```bash
git add wta/src/app.rs
git commit -m "feat(wta): add View::Agents and F2/Ctrl+Tab toggle

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.2: Render the agents list

**Files:**
- Create: `wta/src/ui/agents_view.rs`
- Modify: `wta/src/ui/mod.rs`
- Modify: `wta/src/app.rs` (`draw_frame` / render dispatch)

- [ ] **Step 1: Create the view module**

```rust
// wta/src/ui/agents_view.rs
use ratatui::{
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};
use std::time::SystemTime;

use crate::agent_sessions::{AgentSession, AgentSessionRegistry, AgentStatus};

pub fn render(
    f:    &mut Frame,
    area: Rect,
    reg:  &AgentSessionRegistry,
    list_state: &mut ListState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Agents  (F2 / Ctrl+Tab to switch · ↑↓ select · Enter activate · Del remove) ");

    let rows: Vec<ListItem> = reg.iter_sorted().into_iter().map(row_for).collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, list_state);
}

fn row_for(s: &AgentSession) -> ListItem<'static> {
    let title  = format!("{} — {}", cli_label(s), cwd_basename(s));
    let status = status_label(s);
    let age    = relative_age(s.last_activity_at);

    let dim = matches!(s.status, AgentStatus::Ended | AgentStatus::Historical);
    let title_style  = if dim { Style::default().dim() } else { Style::default() };
    let status_style = match s.status {
        AgentStatus::Working   => Style::default().yellow(),
        AgentStatus::Attention => Style::default().magenta(),
        AgentStatus::Error     => Style::default().red(),
        _ => Style::default(),
    };

    let line = Line::from(vec![
        Span::styled(format!("{:<32}", trunc(&title, 32)), title_style),
        Span::raw("  "),
        Span::styled(format!("{:<10}", status), status_style),
        Span::raw("  "),
        Span::styled(format!("{:>4}", age), Style::default().dim()),
    ]);
    ListItem::new(line)
}

fn cli_label(s: &AgentSession) -> &'static str {
    use crate::agent_sessions::CliSource::*;
    match s.cli_source {
        Claude  => "claude",
        Copilot => "copilot",
        Gemini  => "gemini",
        _       => "agent",
    }
}

fn cwd_basename(s: &AgentSession) -> String {
    s.cwd.file_name().and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

fn status_label(s: &AgentSession) -> &'static str {
    match s.status {
        AgentStatus::Idle       => "IDLE",
        AgentStatus::Working    => "WORKING",
        AgentStatus::Attention  => "ATTENTION",
        AgentStatus::Error      => "ERROR",
        AgentStatus::Ended      => "",
        AgentStatus::Historical => "",
    }
}

fn relative_age(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    if secs < 60        { format!("{}s",  secs) }
    else if secs < 3600 { format!("{}m",  secs / 60) }
    else if secs < 86400{ format!("{}h",  secs / 3600) }
    else                { format!("{}d",  secs / 86400) }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}
```

- [ ] **Step 2: Register the module in `ui/mod.rs`**

```rust
pub mod agents_view;
```

- [ ] **Step 3: Add a `ListState` to `App` and dispatch render based on view**

On `App`:

```rust
pub agents_list_state: ratatui::widgets::ListState,
```

Initialize:

```rust
agents_list_state: {
    let mut s = ratatui::widgets::ListState::default();
    s.select(Some(0));
    s
},
```

In `draw_frame` (around line 609 of `app.rs`), at the point where the main content is drawn, branch on `self.current_view`:

```rust
match self.current_view {
    View::Chat   => { /* existing chat rendering */ }
    View::Agents => {
        crate::ui::agents_view::render(
            f,
            content_area,
            &self.agent_sessions,
            &mut self.agents_list_state,
        );
    }
}
```

- [ ] **Step 4: Build and check**

```bash
cargo build --manifest-path wta/Cargo.toml
```

Expected: builds.

- [ ] **Step 5: Commit**

```bash
git add wta/src/ui/agents_view.rs wta/src/ui/mod.rs wta/src/app.rs
git commit -m "feat(wta): render Agents view list

Three columns (Title · Status · Time), single flat list sorted by
last_activity_at desc. Status column blank for Ended/Historical
sessions; row dimmed.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.3: Cursor navigation (↑/↓) in Agents view

**Files:**
- Modify: `wta/src/app.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn arrow_keys_move_cursor_in_agents_view() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "a".into(), cli_source: CliSource::Claude,
        pane_session_id: "p1".into(), cwd: PathBuf::from("/x"), title: "t".into(),
    });
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "b".into(), cli_source: CliSource::Copilot,
        pane_session_id: "p2".into(), cwd: PathBuf::from("/y"), title: "u".into(),
    });
    app.current_view = View::Agents;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.agents_list_state.selected(), Some(1));

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.agents_list_state.selected(), Some(0));
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib arrow_keys_move_cursor_in_agents_view
```

- [ ] **Step 3: Implement**

In `handle_key`, after the F2 toggle but before chat handling, when `current_view == View::Agents`:

```rust
if self.current_view == View::Agents {
    let count = self.agent_sessions.iter_sorted().len();
    match key.code {
        KeyCode::Down => {
            let cur = self.agents_list_state.selected().unwrap_or(0);
            let next = if count == 0 { 0 } else { (cur + 1).min(count - 1) };
            self.agents_list_state.select(Some(next));
            self.dirty = true;
            return;
        }
        KeyCode::Up => {
            let cur = self.agents_list_state.selected().unwrap_or(0);
            self.agents_list_state.select(Some(cur.saturating_sub(1)));
            self.dirty = true;
            return;
        }
        _ => {}
    }
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib arrow_keys_move_cursor_in_agents_view
```

- [ ] **Step 5: Commit**

```bash
git add wta/src/app.rs
git commit -m "feat(wta): up/down navigation in Agents view

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.4: Enter on a live row → focus pane via wtcli

**Files:**
- Modify: `wta/src/app.rs`

- [ ] **Step 1: Add the dispatch tracking types**

Near the top of `app.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchedCommandKind {
    FocusPane,
    SplitPaneResume,
}

#[derive(Clone, Debug)]
pub struct DispatchedCommand {
    pub kind:       DispatchedCommandKind,
    pub session_id: Option<String>,
    pub argv:       Vec<String>,
}
```

On `App` (test-only):

```rust
#[cfg(test)]
pub last_dispatched_command: Option<DispatchedCommand>,
```

Initialize to `None` in `App::new` and `test_app`.

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn enter_on_live_row_dispatches_focus_command() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "a".into(), cli_source: CliSource::Claude,
        pane_session_id: "00000000-0000-0000-0000-0000000000aa".into(),
        cwd: PathBuf::from("/x"), title: "t".into(),
    });
    app.current_view = View::Agents;
    app.agents_list_state.select(Some(0));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let cmd = app.last_dispatched_command_for_test()
        .expect("a command was dispatched");
    assert_eq!(cmd.kind, DispatchedCommandKind::FocusPane);
    assert_eq!(cmd.session_id.as_deref(), Some("00000000-0000-0000-0000-0000000000aa"));
}
```

- [ ] **Step 3: Run to verify it fails**

Expected: FAIL.

- [ ] **Step 4: Implement command dispatch**

In `handle_key`, in the Agents-view branch (after Up/Down):

```rust
if self.current_view == View::Agents && key.code == KeyCode::Enter {
    if let Some(idx) = self.agents_list_state.selected() {
        // Clone the selected session so we don't keep an immutable borrow.
        let selected = self.agent_sessions
            .iter_sorted()
            .get(idx)
            .map(|s| (*s).clone());
        if let Some(s) = selected {
            self.activate_session(&s);
        }
    }
    return;
}
```

Then add the helpers (use whatever wtcli-spawn helper already exists in `cli_channel.rs`; if there isn't a fire-and-forget one, add `spawn_wtcli_async(argv)` there that does `tokio::process::Command::new(wtcli_path).args(argv).spawn()` and logs failures):

```rust
fn activate_session(&mut self, s: &crate::agent_sessions::AgentSession) {
    use crate::agent_sessions::AgentStatus::*;
    match s.status {
        Idle | Working | Attention | Error => {
            if let Some(pane) = &s.pane_session_id {
                self.dispatch_focus_pane(pane.clone());
            }
        }
        Ended | Historical => {
            self.dispatch_resume(s);
        }
    }
}

fn dispatch_focus_pane(&mut self, pane_session_id: String) {
    let argv = vec![
        "focus-pane".to_string(),
        "--session-id".to_string(),
        pane_session_id.clone(),
    ];
    crate::shell::wt_channel::cli_channel::spawn_wtcli_async(&argv);
    #[cfg(test)]
    {
        self.last_dispatched_command = Some(DispatchedCommand {
            kind: DispatchedCommandKind::FocusPane,
            session_id: Some(pane_session_id),
            argv,
        });
    }
}

#[cfg(test)]
pub fn last_dispatched_command_for_test(&self) -> Option<DispatchedCommand> {
    self.last_dispatched_command.clone()
}
```

`AgentSession` already derives `Clone`, so the borrow workaround above is fine.

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib enter_on_live_row
```

- [ ] **Step 6: Commit**

```bash
git add wta/src/app.rs wta/src/shell/wt_channel/cli_channel.rs
git commit -m "feat(wta): Enter on live agent focuses its pane via wtcli

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.5: Enter on history row → resume via SplitPane

**Files:**
- Modify: `wta/src/app.rs`
- Modify: `wta/src/agent_registry.rs` (add resume-flag knowledge to AgentProfile)

- [ ] **Step 1: Add `resume_flag` field to `AgentProfile`**

In `wta/src/agent_registry.rs`, locate the `AgentProfile` struct definition and the `KNOWN_AGENTS` array. Add the field:

```rust
pub struct AgentProfile {
    // ... existing fields ...
    /// Flag the CLI uses to resume a session, e.g. `"--resume"` for Claude.
    /// Empty when resume is unsupported.
    pub resume_flag: &'static str,
}
```

Set on each entry of `KNOWN_AGENTS` (keep existing field initializers; insert the new line):

```rust
// claude entry:
resume_flag: "--resume",
// copilot entry:
resume_flag: "--resume",     // verify against `copilot --help` at impl time
// gemini entry:
resume_flag: "--resume",
```

If a CLI does not support resume, set `resume_flag: ""` and `dispatch_resume` will fall through to a no-op (with a UI toast: "resume not supported for <cli>").

If `lookup_profile_by_id(name: &str) -> &'static AgentProfile` does not exist already (current `agent_registry.rs` has `lookup_profile`), add it as a thin alias:

```rust
pub fn lookup_profile_by_id(id: &str) -> Option<&'static AgentProfile> {
    KNOWN_AGENTS.iter().find(|p| p.id.eq_ignore_ascii_case(id))
}
```

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn enter_on_history_row_dispatches_split_pane_with_resume() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "abc-123".into(), cli_source: CliSource::Claude,
        pane_session_id: "p".into(), cwd: PathBuf::from("/work/proj"), title: "t".into(),
    });
    app.agent_sessions.apply(SessionEvent::SessionStopped {
        key: "abc-123".into(), reason: "user_exit".into(),
    });

    app.current_view = View::Agents;
    app.agents_list_state.select(Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let cmd = app.last_dispatched_command_for_test()
        .expect("a command was dispatched");
    assert_eq!(cmd.kind, DispatchedCommandKind::SplitPaneResume);
    let argv = cmd.argv.join(" ");
    assert!(argv.contains("split-pane"), "argv: {}", argv);
    assert!(argv.contains("claude --resume abc-123"), "argv: {}", argv);
}
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test --manifest-path wta/Cargo.toml --lib enter_on_history_row
```

- [ ] **Step 4: Implement `dispatch_resume`**

```rust
fn dispatch_resume(&mut self, s: &crate::agent_sessions::AgentSession) {
    let cli_id = match s.cli_source {
        crate::agent_sessions::CliSource::Claude  => "claude",
        crate::agent_sessions::CliSource::Copilot => "copilot",
        crate::agent_sessions::CliSource::Gemini  => "gemini",
        crate::agent_sessions::CliSource::Unknown(_) => return,
    };
    let profile = match crate::agent_registry::lookup_profile_by_id(cli_id) {
        Some(p) => p,
        None => return,
    };
    if profile.resume_flag.is_empty() {
        // v1: silently no-op. Future: push a toast.
        return;
    }
    let commandline = format!("{} {} {}", cli_id, profile.resume_flag, s.key);
    let cwd = s.cwd.to_string_lossy().to_string();

    let argv = vec![
        "split-pane".to_string(),
        "--commandline".to_string(),
        commandline,
        "--starting-directory".to_string(),
        cwd,
    ];
    crate::shell::wt_channel::cli_channel::spawn_wtcli_async(&argv);
    #[cfg(test)]
    {
        self.last_dispatched_command = Some(DispatchedCommand {
            kind: DispatchedCommandKind::SplitPaneResume,
            session_id: None,
            argv,
        });
    }
}
```

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test --manifest-path wta/Cargo.toml --lib enter_on_history_row
```

- [ ] **Step 6: Commit**

```bash
git add wta/src/app.rs wta/src/agent_registry.rs
git commit -m "feat(wta): Enter on history row resumes session in new pane

Looks up the CLI's resume flag from agent_registry::AgentProfile
and spawns wtcli split-pane with the resume commandline.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.6: Delete on history row removes from registry

**Files:**
- Modify: `wta/src/app.rs`
- Modify: `wta/src/agent_sessions.rs`

- [ ] **Step 1: Add `remove(&mut self, key)` on registry**

In `agent_sessions.rs`:

```rust
pub fn remove(&mut self, key: &AgentKey) {
    if let Some(s) = self.sessions.remove(key) {
        if let Some(pane) = s.pane_session_id {
            self.active_by_pane.remove(&pane);
        }
        self.dirty = true;
    }
}
```

- [ ] **Step 2: Write the failing tests**

```rust
#[test]
fn delete_on_history_row_removes_session_from_registry() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "k".into(), cli_source: CliSource::Claude,
        pane_session_id: "p".into(), cwd: PathBuf::from("/x"), title: "t".into(),
    });
    app.agent_sessions.apply(SessionEvent::SessionStopped {
        key: "k".into(), reason: "".into(),
    });
    app.current_view = View::Agents;
    app.agents_list_state.select(Some(0));

    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert!(!app.agent_sessions.has_session(&"k".to_string()));
}

#[test]
fn delete_on_live_row_is_noop() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = test_app();
    use crate::agent_sessions::{CliSource, SessionEvent};
    use std::path::PathBuf;
    app.agent_sessions.apply(SessionEvent::SessionStarted {
        key: "k".into(), cli_source: CliSource::Claude,
        pane_session_id: "p".into(), cwd: PathBuf::from("/x"), title: "t".into(),
    });
    app.current_view = View::Agents;
    app.agents_list_state.select(Some(0));

    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert!(app.agent_sessions.has_session(&"k".to_string()));
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test --manifest-path wta/Cargo.toml --lib delete_on_
```

- [ ] **Step 4: Implement**

In `handle_key`, in the Agents-view branch (after Enter):

```rust
if self.current_view == View::Agents && key.code == KeyCode::Delete {
    if let Some(idx) = self.agents_list_state.selected() {
        let target = self.agent_sessions
            .iter_sorted()
            .get(idx)
            .map(|s| (s.key.clone(), s.status.clone()));
        if let Some((key, status)) = target {
            use crate::agent_sessions::AgentStatus::*;
            if matches!(status, Ended | Historical) {
                self.agent_sessions.remove(&key);
                self.dirty = true;
            }
        }
    }
    return;
}
```

- [ ] **Step 5: Run to verify they pass**

```bash
cargo test --manifest-path wta/Cargo.toml --lib delete_on_
```

- [ ] **Step 6: Commit**

```bash
git add wta/src/app.rs wta/src/agent_sessions.rs
git commit -m "feat(wta): Delete on history row removes session from registry

Live sessions are unaffected — the user must end the agent first.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4.7: End-to-end manual smoke test

**Files:** none (manual verification).

- [ ] **Step 1: Build everything**

```bash
Get-Process wta -ErrorAction SilentlyContinue | Stop-Process -Force
cargo build --manifest-path wta/Cargo.toml
cmd.exe /c "tools\razzle.cmd && bcz no_clean"
```

- [ ] **Step 2: Deploy and launch**

In Visual Studio, F5 with `CascadiaPackage` as startup project. Wait for Windows Terminal to launch with the WTA agent pane.

- [ ] **Step 3: Open three panes with three CLIs**

In the WT window:
- Pane 1: `claude` (with the wt-agent-hooks plugin installed via `claude plugin install ...`).
- Pane 2: `copilot --interactive`.
- Pane 3: `gemini` (after completing M5 — the Gemini extension install).

In each, run a simple tool-using prompt (e.g. "list the files in this directory").

- [ ] **Step 4: Verify the Agents view**

In the WTA pane, press `F2`. Expected:
- Three rows visible.
- The active one shows `WORKING` then `IDLE` after the prompt completes.
- Selecting a row and pressing `Enter` focuses that pane in WT.

- [ ] **Step 5: Verify history**

`exit` one of the agents. Return to WTA agent view; the row's status column is empty and dimmed. Press `Enter` on it; a new pane opens running `<cli> --resume <sid>`.

- [ ] **Step 6: Document any rough edges**

Create `docs/superpowers/specs/2026-05-02-agent-management-known-issues.md` with anything observed (e.g. Copilot resume flag wrong, ATTENTION not detected, etc.) — but do not block this milestone on those. Commit the doc.

```bash
git add docs/superpowers/specs/2026-05-02-agent-management-known-issues.md
git commit -m "docs: agent management v1 known issues from smoke test

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Milestone 5 — Gemini CLI extension support

**Goal:** Make the existing `wta/agent-hooks-plugin/` directory ALSO installable as a Gemini extension via `gemini extensions install <path>`. Reuse the same `send-event.ps1` and bash hook scripts; only add manifest + Gemini-specific hooks.json with Gemini's event names (`BeforeTool`/`AfterTool` instead of `PreToolUse`/`PostToolUse`).

**Background:**
- Gemini CLI hooks docs: <https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/index.md>
- Gemini extension format requires `gemini-extension.json` at root + `hooks/hooks.json` using `${extensionPath}` (NOT `${CLAUDE_PLUGIN_ROOT}`).
- Event-name mapping (Claude/Copilot → Gemini):
  - `SessionStart` → `SessionStart` (same)
  - `SessionEnd` → `SessionEnd` (same)
  - `PreToolUse` → `BeforeTool`
  - `PostToolUse` → `AfterTool`
  - `Notification` → `Notification` (same)
  - `Stop` → `AfterAgent` (closest semantic match; optional)
  - `UserPromptSubmit` → `BeforeAgent`
- Gemini hooks contract is the same (stdin JSON with `session_id`, stdout JSON, exit codes 0/2). `send-event.ps1` already extracts `session_id` from stdin and `cli_source` from `WTA_CLI_SOURCE` env var — no modifications needed.

### Task 5.1: Add Gemini extension manifest

**Files:**
- Create: `wta/agent-hooks-plugin/gemini-extension.json`

- [ ] **Step 1: Create `gemini-extension.json`**

```json
{
  "name": "wt-agent-hooks",
  "version": "0.1.0",
  "description": "Forward Gemini CLI hook events to Windows Terminal for WTA display"
}
```

This is the minimal manifest. Gemini auto-discovers `hooks/hooks.json` at the extension root (so we cannot reuse the existing `hooks/hooks.json` that has Copilot/Claude event names — see Task 5.2 for the fix).

### Task 5.2: Restructure hooks layout to support both ecosystems

**Problem:** Both Copilot (looks at `hooks/hooks.json`, uses `${CLAUDE_PLUGIN_ROOT}`) and Gemini (also looks at `hooks/hooks.json`, uses `${extensionPath}`) expect their hooks config at the same path but with different event names and path variables. We need to keep them separate.

**Solution:** Move Gemini hooks to a sibling directory `gemini-hooks/` and reference it explicitly via the `contextFileName` / hooks dir override in `gemini-extension.json` if supported, OR: make `gemini-extension.json` declare hooks inline.

**Verification needed:** Check whether `gemini-extension.json` supports `"hooksFileName"` / `"hooksDir"` keys. If not, the only safe approach is **inline hooks in `gemini-extension.json`**.

- [ ] **Step 1: Verify gemini-extension.json schema**

```bash
# Look for keys related to hooks dir/file in the gemini-cli reference
curl -s https://raw.githubusercontent.com/google-gemini/gemini-cli/main/docs/extensions/reference.md | Select-String -Pattern "hooks"
```

- [ ] **Step 2 (if hooks must be at `hooks/hooks.json`): inline approach**

If Gemini insists on `hooks/hooks.json`, a clean alternative is:

- Keep `hooks/hooks.json` for Copilot/Claude (`${CLAUDE_PLUGIN_ROOT}`).
- Put Gemini hooks **inline** inside `gemini-extension.json` under a top-level `"hooks"` key (Gemini extensions support inline hook declarations per the docs example for issue #14449).

```json
{
  "name": "wt-agent-hooks",
  "version": "0.1.0",
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -ExecutionPolicy Bypass -Command \"$env:WTA_CLI_SOURCE='gemini'; & '${extensionPath}/hooks/send-event.ps1' agent.session.start\""
          }
        ]
      }
    ],
    "SessionEnd": [...],
    "BeforeTool": [
      { "matcher": ".*", "hooks": [{ "type": "command", "command": "powershell -ExecutionPolicy Bypass -Command \"$env:WTA_CLI_SOURCE='gemini'; & '${extensionPath}/hooks/send-event.ps1' agent.tool.starting\"" }] }
    ],
    "AfterTool": [
      { "matcher": ".*", "hooks": [{ "type": "command", "command": "powershell -ExecutionPolicy Bypass -Command \"$env:WTA_CLI_SOURCE='gemini'; & '${extensionPath}/hooks/send-event.ps1' agent.tool.finished\"" }] }
    ],
    "Notification": [...]
  }
}
```

- [ ] **Step 2 (alternative if Gemini DOES support `hooksFileName`):** create `gemini-hooks/hooks.json` and reference it from the manifest.

```json
{
  "name": "wt-agent-hooks",
  "version": "0.1.0",
  "hooksFileName": "gemini-hooks/hooks.json"
}
```

Then `gemini-hooks/hooks.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -ExecutionPolicy Bypass -Command \"$env:WTA_CLI_SOURCE='gemini'; & '${extensionPath}/hooks/send-event.ps1' agent.session.start\""
          }
        ]
      }
    ],
    "BeforeTool": [...],
    "AfterTool": [...],
    "SessionEnd": [...],
    "Notification": [...]
  }
}
```

**Decision rule:** prefer the inline approach (Step 2 first option) — it's documented in gemini-cli issue #14449 as the supported way to bundle hooks in extensions, and avoids the path-collision risk with `hooks/hooks.json`.

**Why explicitly set `WTA_CLI_SOURCE='gemini'` per command:** unlike Copilot's bash entry scripts which export this var, Gemini invokes the `command` directly without a wrapper. Setting it inline in the PowerShell command line ensures `send-event.ps1` tags events as `cli_source: "gemini"`.

### Task 5.3: Verify Gemini's resume flag

**Files:** `wta/src/agent_registry.rs`

- [ ] **Step 1: Check whether Gemini CLI supports `--resume <session_id>`**

```bash
gemini --help 2>&1 | Select-String -Pattern "resume|session"
```

- [ ] **Step 2: Update `agent_registry.rs` accordingly**

If Gemini supports `--resume <id>`:
```rust
gemini => AgentProfile {
    cli_id: "gemini",
    display_name: "Gemini",
    resume_flag: "--resume",
    // ...
};
```

If Gemini does NOT support resume by session id, set `resume_flag: ""` so `dispatch_resume` early-returns and history rows for Gemini are non-resumable. Document this behavior in `gemini-extension.json` README.

- [ ] **Step 3: Run tests**

```bash
cargo test --manifest-path wta/Cargo.toml --bin wta agent_registry
```

### Task 5.4: Update README + docs

**Files:**
- Modify: `wta/agent-hooks-plugin/README.md`
- Modify: `wta/agent-hooks-plugin/doc/agent-hooks-plugin.md`

- [ ] **Step 1: Add Gemini install section to README.md**

```markdown
### Gemini CLI

```bash
# Install as a Gemini extension (requires git in PATH; gemini-cli copies the dir)
gemini extensions install C:/GitRepo/agentic-terminal/wta/agent-hooks-plugin
```

After install, restart the Gemini session for hooks to load. Verify:

```bash
/extensions list  # inside gemini interactive mode
```
```

- [ ] **Step 2: Add Gemini event mapping table to doc/agent-hooks-plugin.md**

| Gemini Event   | WTA Event Type        |
|----------------|-----------------------|
| `SessionStart` | `agent.session.start` |
| `SessionEnd`   | `agent.session.end`   |
| `BeforeTool`   | `agent.tool.starting` |
| `AfterTool`    | `agent.tool.finished` |
| `Notification` | `agent.notification`  |

### Task 5.5: Manual smoke test — Gemini

- [ ] **Step 1: Install gemini CLI** (if not already)

```powershell
winget install --id Google.Gemini.CLI
# or follow https://github.com/google-gemini/gemini-cli/#install
```

- [ ] **Step 2: Install the extension**

```powershell
gemini extensions install C:/GitRepo/agentic-terminal/wta/agent-hooks-plugin
```

- [ ] **Step 3: Launch WT, F5 in VS, open three panes**

- Pane 1: `gemini`
- Pane 2: `copilot --interactive`
- Pane 3: `claude` (optional)

In each, run `list files in this directory`.

- [ ] **Step 4: Verify the Agents view**

Press `F2` in WTA pane. Expected: Gemini row appears with `cli=gemini`, status flips `WORKING` → `IDLE`.

- [ ] **Step 5: Commit M5**

```bash
git add wta/agent-hooks-plugin/gemini-extension.json wta/agent-hooks-plugin/README.md wta/agent-hooks-plugin/doc/agent-hooks-plugin.md wta/src/agent_registry.rs
git commit -m "feat(hooks): add Gemini CLI extension support

Adds gemini-extension.json with inline hooks declarations using Gemini's
event names (BeforeTool/AfterTool/SessionStart/SessionEnd/Notification)
and \${extensionPath} variable. Reuses the existing send-event.ps1 by
exporting WTA_CLI_SOURCE='gemini' inline per command.

Install: gemini extensions install <path-to-agent-hooks-plugin>

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Final verification

- [ ] **Step 1: Run full test suite**

```bash
cargo test --manifest-path wta/Cargo.toml
cmd.exe /c "tools\razzle.cmd && runut.cmd"
```

Expected: all tests pass.

- [ ] **Step 2: Push the branch**

```bash
git push -u origin dev/yuazha/agent-management
```

- [ ] **Step 3: Open PR with the spec and plan linked in the description**

Body should reference:
- `docs/superpowers/specs/2026-05-02-agent-management-design.md`
- `docs/superpowers/plans/2026-05-03-agent-management.md`
- PR #11 (the source of M1+M2)
