# wt-agent-hooks (Gemini Extension)

Forward Gemini CLI hook events to Windows Terminal for WTA display.

## Installation

```bash
gemini extensions install C:\GitRepo\agentic-terminal\wta\gemini-extension
```

Or via the path on your machine. Gemini will copy the extension into
`~/.gemini/extensions/wt-agent-hooks/`.

## Verify

After install, list installed extensions:

```bash
gemini extensions list
```

You should see `wt-agent-hooks` with its hooks active.

## Events Forwarded

| Gemini event   | wta event topic       |
| :------------- | :-------------------- |
| `SessionStart` | `agent.session.start` |
| `SessionEnd`   | `agent.session.end`   |
| `BeforeTool`   | `agent.tool.starting` |
| `AfterTool`    | `agent.tool.finished` |
| `Notification` | `agent.notification`  |

## Requirements

- Running inside a Windows Terminal pane (the script no-ops outside).
- `wtcli` on PATH (automatic inside the deployed Windows Terminal package).

## Notes

- This extension uses `${extensionPath}` (Gemini's variable). The Copilot/Claude
  plugin (`wta/agent-hooks-plugin/`) uses `${CLAUDE_PLUGIN_ROOT}` instead — both
  resolve to the plugin's installed root directory.
- Event names differ from Claude/Copilot (`BeforeTool`/`AfterTool` vs.
  `PreToolUse`/`PostToolUse`); see the [Gemini hooks reference][hooks-ref].

[hooks-ref]: https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/reference.md
