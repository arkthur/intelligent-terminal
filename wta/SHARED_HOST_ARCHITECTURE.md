# WTA Shared Host Architecture

## Goal

Make agent panes fast by avoiding a fresh ACP session per pane.

The new model is:

- one long-lived `wta host`
- many lightweight `wta attach` pane TUIs
- one shared ACP agent connection and one shared ACP session

This means Terminal startup pays the agent startup cost once, and later agent panes attach to the already-warm host.

---

## Current Model

Today each agent pane starts its own full WTA ACP TUI process.

That pane-local process does all of the following itself:

- connect to Windows Terminal
- discover pane identity
- spawn the ACP agent process
- initialize ACP
- create a new ACP session
- build prompt context
- run the prompt loop

This is expensive and repeats for every agent pane.

---

## Target Model

### Processes

- `wta host`
  - long-lived background process
  - owns the ACP agent process
  - owns the ACP connection
  - owns the shared `session_id`
  - owns shared transcript and recommendation state
  - owns recommendation execution

- `wta attach`
  - lightweight pane-local TUI
  - renders the shared state from the host
  - keeps only local UI state such as input buffer, cursor, and scroll
  - sends prompt submissions and selection actions to the host

### Ownership

The host is not just a dumb proxy.

It must own:

- ACP initialize and session creation
- the live ACP connection
- prompt submission
- streamed agent updates
- parsed recommendations
- execution state
- shared transcript

Without this, each pane would still need to create its own ACP client session, which defeats the goal.

---

## High-Level Flow

### Startup

1. Windows Terminal starts.
2. Terminal calls `ensure_wta_host_running()`.
3. `wta host` is started if not already running.
4. `wta host` starts the ACP agent and creates the shared ACP session.
5. Later, any agent pane runs `wta attach`.
6. `wta attach` connects to the already-running host and renders the shared session state.

### Prompt Submission

1. User types in a pane-local `wta attach` TUI.
2. The TUI sends the user request and its pane-local context to `wta host`.
3. `wta host` builds the final planner prompt using that pane context plus WT state.
4. `wta host` sends one `prompt()` call to the shared ACP session.
5. ACP updates stream back to the host.
6. The host updates shared state and broadcasts updates to attached pane TUIs.

### Recommendation Execution

1. A pane user selects a recommendation.
2. `wta attach` sends the selected action to the host.
3. `wta host` executes it through the WT integration layer.
4. Execution status is broadcast back to all attached pane TUIs.

---

## Communication Boundaries

There are three different links in the system:

- `wta host <-> ACP agent`
  - ACP over stdio

- `wta attach <-> wta host`
  - local IPC

- `wta host <-> Windows Terminal`
  - WT named pipe

### Why Local IPC Is Needed

Once `wta host` and `wta attach` are separate processes, in-process Rust channels are no longer enough.

The pane TUI must be able to:

- attach to the already-running host
- submit prompts
- receive streamed updates
- receive recommendation state
- trigger execution
- sync shared transcript state

That is why host/client local IPC is required.

### Recommended IPC Transport

Use a Windows named pipe with framed JSON messages.

Reasons:

- local-only communication
- fits Windows well
- works naturally with attach-to-existing-host behavior
- easy to combine with single-instance host startup

Do not use MCP for pane attachment. MCP is the wrong abstraction here.

---

## Shared vs Local State

### State Owned By `wta host`

- ACP connection
- ACP session id
- shared transcript
- recommendation list
- selected execution status
- tool call / plan / system status
- prompt in-flight state

### State Owned By Each `wta attach`

- input buffer
- cursor position
- local scroll position
- debug panel open or closed
- pane-local identity and cwd

---

## Pane Context Model

Each pane TUI still matters, because prompts may depend on which pane the user is currently working from.

Each `wta attach` should send pane-local context with prompt submissions:

- current pane id
- current tab id
- current window id
- cwd
- source pane identity if relevant

The host should use that per-request pane context when assembling the final planner prompt.

So the TUI is not the ACP client, but it is still the source of pane-local prompt context.

---

## Host Startup Policy

The host should be started by Terminal startup, not by the first agent pane.

### Recommended Scope

Start with one shared host for the whole Terminal app lifetime.

That means:

- Terminal startup launches or attaches to one host
- all later agent panes attach to that same host
- one shared conversation/session exists across panes

If needed later, this can be changed to per-window scope.

---

## Safe Multi-Start Behavior

Host startup must be idempotent and race-safe.

Use this sequence:

1. Try connecting to the host IPC pipe.
2. If the host is already running, stop.
3. If not, acquire a named mutex.
4. After acquiring the mutex, try the host pipe again.
5. If the host is still absent, spawn `wta host`.
6. Wait for a host ready signal.
7. Release the mutex.

This prevents duplicate hosts if:

- Terminal startup runs multiple ensure attempts
- multiple windows start at once
- an attach client races with startup

---

## Required Modes

### `wta host`

Responsibilities:

- launch ACP agent
- initialize ACP
- create and own one session
- hold shared transcript and recommendations
- accept attach clients
- accept prompt submissions and selection actions
- broadcast updates
- execute WT actions

### `wta attach`

Responsibilities:

- connect to host
- render the shared state
- capture pane-local context
- send prompt submissions
- send recommendation selections

---

## Suggested Host-Client Messages

### Client To Host

- `Attach`
- `GetSnapshot`
- `SubmitPrompt`
- `SelectRecommendation`
- `PaneContextUpdate`
- `Detach`

### Host To Client

- `Attached`
- `SharedStateSnapshot`
- `SessionConnected`
- `AgentChunk`
- `AgentMessageEnd`
- `RecommendationsUpdated`
- `ToolCallUpdated`
- `ExecutionStatus`
- `Error`

Use push/subscription semantics, not polling.

---

## Terminal Integration Changes

### Terminal Startup

Terminal should eagerly ensure the host is running once Terminal is up.

If we want one app-wide host, this should be done at the app-wide startup boundary.

### Agent Pane Launch

Agent panes should stop launching the current full WTA ACP session path.

Instead, an agent pane should launch:

- `wta attach`

not:

- a fresh `wta` ACP TUI that creates its own agent connection and ACP session

---

## Non-Goals

This design does not aim to:

- create one ACP session per pane
- use MCP as the pane attachment protocol
- keep recommendation execution in pane-local clients
- allow every pane to independently own ACP state

---

## Success Criteria

- first Terminal startup pays the ACP startup cost once
- opening a second or third agent pane is fast
- no fresh ACP `new_session` happens per pane
- all attached panes show the same conversation and recommendations
- recommendation execution stays consistent across panes
- prompt submissions still use pane-local context from the pane that sent them
- duplicate host startup is prevented

---

## Implementation Order

1. Add `wta host` mode and shared state model.
2. Add host/client IPC protocol.
3. Convert the pane TUI into `wta attach`.
4. Move ACP session ownership into the host.
5. Add safe `ensure_wta_host_running()` logic with mutex and ready handshake.
6. Start the host from Terminal startup.
7. Change agent pane launch to `wta attach`.

