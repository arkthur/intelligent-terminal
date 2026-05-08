# wt-agent-hooks Plugin

Forward CLI agent hook events to Windows Terminal for WTA display.

## Overview

This plugin bridges agent lifecycle events (Copilot CLI and Claude Code) to
Windows Terminal's WTA infrastructure, enabling real-time visibility into agent
tool use and notifications within the Terminal. For Gemini CLI, see the sibling
[`../gemini-extension/`](../gemini-extension/) directory — Gemini uses a
different manifest format and event names, so it ships as a separate package.

## Installation

### Copilot CLI

```bash
copilot plugin install ./wta/wt-agent-hooks/agent-hooks-plugin
```

Verifies at `~/.copilot/installed-plugins/_direct/agent-hooks-plugin/`.

### Claude Code

The plugin also ships a `.claude-plugin/plugin.json` manifest so it can be
installed into Claude Code. The recommended path is the local-marketplace
form — point Claude at the plugin directory:

```bash
# from the agentic-terminal repo root:
claude plugin install --local ./wta/wt-agent-hooks/agent-hooks-plugin
# or, depending on your Claude CLI version:
claude /plugin install ./wta/wt-agent-hooks/agent-hooks-plugin
```

If your Claude CLI does not expose a direct install command, copy the plugin
folder into Claude's user-local plugin directory (typically
`~/.claude/plugins/wt-agent-hooks/`) and Claude will auto-discover it on next
launch.

### Gemini CLI

Gemini does **not** use this plugin. See [`../gemini-extension/`](../gemini-extension/)
for the Gemini equivalent.

## Configuration

To enable event logging to the console:

```bash
export WTA_LOG_AGENT_EVENT=1
```

Then launch WTA with the environment variable set.

## Supported CLIs

- **Copilot CLI** — Fully supported (this package)
- **Claude Code** — Fully supported (this package, install via `.claude-plugin/`)
- **Gemini CLI** — Supported via [`../gemini-extension/`](../gemini-extension/)

## Requirements

- `wtcli` on PATH (automatic inside Windows Terminal package)

## Events

The plugin registers the following lifecycle events:

- **PreToolUse** — Fired before a tool is invoked
- **PostToolUse** — Fired after tool execution completes
- **Notification** — General notifications from the agent
- **Stop** — Session termination (agent stop event)
- **SubagentStop** — Sub-agent termination
