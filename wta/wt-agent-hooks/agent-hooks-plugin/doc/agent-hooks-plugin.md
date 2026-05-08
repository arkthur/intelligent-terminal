# Agent Hooks Plugin

The `wt-agent-hooks` plugin forwards agent lifecycle events from **Copilot CLI**,
**Claude Code**, and **Gemini CLI** to Windows Terminal (WTA) via
`wtcli send-event`. This lets the WTA agent pane display real-time tool use,
prompts, and session events from any agent CLI session running in another pane.

The repo ships **two packages** under `wta/wt-agent-hooks/` that share the
same `send-event.ps1` script:

| Package                                              | Used by             | Manifest format              | Path-variable        |
| ---------------------------------------------------- | ------------------- | ---------------------------- | -------------------- |
| `wta/wt-agent-hooks/agent-hooks-plugin/`             | Copilot CLI, Claude | Claude plugin (`hooks.json`) | `${CLAUDE_PLUGIN_ROOT}` |
| `wta/wt-agent-hooks/gemini-extension/`               | Gemini CLI          | Gemini extension             | `${extensionPath}`   |

(Copilot CLI inherits Claude's plugin shape, so a single Claude-style package
satisfies both. Gemini's manifest is incompatible, so it ships separately.)

## How It Works

```
Agent CLI ─── hook fires ──▶ send-event.ps1 ──▶ wtcli send-event ──▶ WTA
            (stdin JSON)     (wraps payload)      (COM protocol)     (displays)
```

1. The agent CLI (Copilot / Claude / Gemini) triggers hooks at lifecycle points
   (tool use, prompt, session start/end).
2. `send-event.ps1` reads the hook JSON from stdin.
3. It wraps the payload as `{cli_source: <copilot|claude|gemini>, agent_session_id: <sid>, payload: <hook_data>}`
   and calls `wtcli send-event -e <event_type> <json>`.
4. WTA receives the event and displays it in the agent pane (if enabled).

The `cli_source` field is determined per invocation by `send-event.ps1`. There
are two installer styles in this repo:

- **`wta/wt-agent-hooks/agent-hooks-plugin/hooks/hooks.json`** — does **not**
  pass `-CliSource`, because the same package is installed by both Copilot
  and Claude. The script falls back to env-var detection at runtime
  (`COPILOT_SESSION_ID` ⇒ copilot, `CLAUDE_SESSION_ID` ⇒ claude, etc.).
- **`wta/wt-agent-hooks/gemini-extension/hooks/hooks.json`** — every command
  passes `-CliSource gemini` explicitly, because only Gemini installs that
  extension and env-var detection alone could mis-classify it.

See the comment block at the top of `send-event.ps1` for the full priority
order (param → `WTA_CLI_SOURCE` env → CLI-specific session-id env vars →
`CLAUDE_PLUGIN_ROOT` → default).

## Installation

### Copilot CLI

```powershell
# Install from local path (use forward slashes)
copilot plugin install "./wta/wt-agent-hooks/agent-hooks-plugin"
```

Installed to `~/.copilot/installed-plugins/_direct/agent-hooks-plugin/`.

### Claude Code

The same `wta/wt-agent-hooks/agent-hooks-plugin/` folder ships a
`.claude-plugin/plugin.json` manifest, so Claude can install it directly:

```powershell
# Depending on your Claude CLI version:
claude plugin install --local ./wta/wt-agent-hooks/agent-hooks-plugin
# or:
claude /plugin install ./wta/wt-agent-hooks/agent-hooks-plugin
```

If your Claude CLI doesn't expose a direct install command, copy the folder
into `~/.claude/plugins/wt-agent-hooks/` — Claude auto-discovers it on next
launch.

### Gemini CLI

Use the **separate** `wta/wt-agent-hooks/gemini-extension/` package
(different manifest format and event vocabulary):

```powershell
gemini extensions install C:\GitRepo\agentic-terminal\wta\wt-agent-hooks\gemini-extension
```

Installed to `~/.gemini/extensions/wt-agent-hooks/`. Verify with
`gemini extensions list`.

## Event Types

WTA normalises hook events from all three CLIs into a single set of topic
names. Event vocabularies differ per CLI:

| WTA event topic         | Copilot CLI hook       | Claude Code hook       | Gemini CLI hook |
| ----------------------- | ---------------------- | ---------------------- | --------------- |
| `agent.session.start`   | `SessionStart`         | `SessionStart`         | `SessionStart`  |
| `agent.session.end`     | `SessionEnd`           | `SessionEnd`           | `SessionEnd`    |
| `agent.prompt.submit`   | `UserPromptSubmit`     | `UserPromptSubmit`     | `BeforeAgent`   |
| `agent.tool.starting`   | `PreToolUse`           | `PreToolUse`           | `BeforeTool`    |
| `agent.tool.finished`   | `PostToolUse`          | `PostToolUse`          | `AfterTool`     |
| `agent.tool.failed`     | `PostToolUseFailure`   | `PostToolUseFailure`   | *(not emitted)* |
| `agent.notification`    | `Notification`         | `Notification`         | `Notification`  |
| `agent.error`           | `ErrorOccurred`        | `ErrorOccurred`        | *(not emitted)* |
| `agent.stop`            | `Stop`                 | `Stop`                 | `AfterAgent`    |
| `agent.subagent.stop`   | `SubagentStop`         | `SubagentStop`         | *(not emitted)* |

Notes:
- Copilot CLI and Claude Code share **the same `hooks.json`** in
  `wta/wt-agent-hooks/agent-hooks-plugin/hooks/hooks.json` — the event names
  are Claude-style (`PreToolUse`/`PostToolUse`/etc.) and Copilot CLI honours
  them too.
- Gemini's `hooks.json` lives in
  `wta/wt-agent-hooks/gemini-extension/hooks/hooks.json` and uses
  `BeforeTool`/`AfterTool`/`BeforeAgent`/`AfterAgent` per the
  [Gemini hooks reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/reference.md).
- Gemini does not have native equivalents for tool-failure, error, or
  subagent-stop, so those topics never fire from Gemini.

## Environment Variables

### Required (set automatically by Windows Terminal)

| Variable        | Description                                              |
|-----------------|----------------------------------------------------------|
| `WT_COM_CLSID`  | COM class ID for the WT Protocol server. Set by Windows Terminal in each pane. If not set, hooks exit silently (not running inside WT). |

### Required (must be on PATH)

| Binary   | Description                                                |
|----------|------------------------------------------------------------|
| `wtcli`  | Windows Terminal CLI client. Must be on PATH for hooks to send events. If not found, hooks exit silently. |

### Optional

| Variable              | Description                                           |
|-----------------------|-------------------------------------------------------|
| `WTA_LOG_AGENT_EVENT` | Set to `1` to enable agent event display in WTA. When unset, WTA silently ignores agent events. |

## Per-Repo Hooks (Copilot only)

The plugin sends events from **any directory** where Copilot CLI runs. For
per-repo hooks (only active in a specific project), use the `.github/hooks/`
approach instead. See `wta/wt-agent-hooks/agent-hooks/` for a working example
that combines file logging with `wtcli send-event`.

Key difference: per-repo hooks use `.github/hooks/hooks.json` at the **git
root** (not a subdirectory). Copilot CLI discovers hooks relative to the git
root of the working directory. Claude Code and Gemini CLI use their own
plugin/extension manifests instead — there is no per-repo hooks.json
equivalent for them.

## Troubleshooting

**Hooks not firing?**

| CLI     | Where to look                                                                  |
| ------- | ------------------------------------------------------------------------------ |
| Copilot | `~/.copilot/logs/process-*.log` — search for `"hook"` / `"Invalid"`. Verify load: `"Loaded N hook(s) from 2 plugin(s)"`. |
| Claude  | `~/.claude/logs/*.log` (or run `claude --debug`) — search for `hook` / `wt-agent-hooks`. |
| Gemini  | `~/.gemini/logs/*.log` and `gemini extensions list` — verify `wt-agent-hooks` is listed and active. |

For all three CLIs, also check `%LOCALAPPDATA%\IntelligentTerminal\logs\hook-trace.log`
— `send-event.ps1` writes one line per invocation regardless of CLI:

```
2026-05-08 10:15:29.123 | ENTER cli=claude event=agent.session.start envHint=claude wt=<GUID> pid=12345
2026-05-08 10:15:29.456 | OK    cli=claude event=agent.session.start exit=0 sessId=0303f5b0 wtcli=...
```

**Events not showing in WTA?**
- Ensure `WTA_LOG_AGENT_EVENT=1` is set in the WTA process environment
- Check `%TEMP%\wta-event-diag.log` for `agent_event` entries
- Verify `wtcli` is on PATH inside the pane where the CLI runs (or
  `WTCLI_PATH` is set as escape hatch)

**Wrong `cli_source` reported (e.g. Claude session shows up as `copilot`)?**
- For the `agent-hooks-plugin` (Copilot + Claude shared), `cli_source` is
  detected via env vars at runtime: `COPILOT_SESSION_ID` ⇒ `copilot`,
  `CLAUDE_SESSION_ID` ⇒ `claude`. If neither is set, the script falls back
  through `CLAUDE_PLUGIN_ROOT` (⇒ `claude`) and finally defaults to
  `copilot`. A misclassification usually means the CLI failed to set its
  session-id env var.
- For `gemini-extension`, every `hooks.json` command passes
  `-CliSource gemini`, so misclassification there means the hooks.json
  itself was edited or `WTA_CLI_SOURCE` was set in the environment to
  override it.
- Inspect both files:
  - `wta/wt-agent-hooks/agent-hooks-plugin/hooks/hooks.json` — should NOT
    contain `-CliSource` (env-var detection handles Copilot vs. Claude).
  - `wta/wt-agent-hooks/gemini-extension/hooks/hooks.json` — every entry
    should contain `-CliSource gemini`.

**"Invalid JSON" from wtcli?**
- The plugin uses `ProcessStartInfo` with CommandLineToArgvW-correct escaping
  to bypass PowerShell's native command argument mangling. If you modify
  `send-event.ps1`, avoid passing JSON directly as a PowerShell native command
  argument — use `ProcessStartInfo` with escaped quotes instead.

**Plugin hooks don't work in WTA agent pane (Copilot ACP mode)?**
- WTA launches Copilot via `copilot --acp --stdio` (Agent Control Protocol).
  ACP mode does **not** trigger CLI plugin hooks. The plugin only works for
  interactive Copilot CLI sessions running in regular terminal panes.
- Claude and Gemini hooks **do** fire when launched through WTA agent pane
  (they run in interactive mode there, not ACP), so this caveat is
  Copilot-specific.
