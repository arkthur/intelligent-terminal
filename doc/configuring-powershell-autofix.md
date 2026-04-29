# Configuring PowerShell for Auto-Fix

The WTA auto-fix feature automatically detects when a command fails in another pane and suggests a fix. It works by listening for **OSC 133** shell integration sequences that PowerShell emits after each command.

## How It Works

1. PowerShell emits `OSC 133;D;<exit_code>` after every command finishes
2. Windows Terminal forwards this as a `vt_sequence` event to WTA
3. If `exit_code != 0`, WTA reads the pane's terminal buffer and asks the AI to diagnose the error and suggest a fix
4. The user reviews and confirms the suggestion before it runs

## Requirements

- **Windows Terminal** with the Intelligent Terminal build (handles event forwarding)
- **PowerShell 7+** with shell integration enabled (emits OSC 133 sequences)

## Enabling Shell Integration

Add the following to your PowerShell profile (open it with `notepad $PROFILE`):

```powershell
# Shell integration for Windows Terminal (OSC 133 marks)
$__origPrompt = $function:prompt
function prompt {
    $ec = if ($?) { 0 } else { 1 }
    "`e]133;D;$ec`a`e]133;A`a$($__origPrompt.Invoke())`e]133;B`a"
}
```

This wraps your existing prompt to emit three OSC 133 sequences on every command:

| Sequence | Meaning | Role |
|----------|---------|------|
| `133;D;$ec` | Command finished with exit code | **Triggers auto-fix when `$ec != 0`** |
| `133;A` | Prompt start | Marks where the new prompt begins |
| `133;B` | Command input start | Marks where user input begins |

The key is `133;D` — it reports the previous command's exit code. WTA listens for this and triggers auto-fix whenever the exit code is non-zero.

### Verifying It Works

1. Open a pane in Intelligent Terminal
2. Run a command that fails, e.g.: `Get-Item "C:\nonexistent-path"`
3. The WTA agent pane should show a notification and automatically suggest a fix

### Checking the Diagnostic Log

Autofix events are logged by the shared host process. Find the log directory:

```powershell
# Packaged install (F5 / MSIX):
$pkg = Get-AppxPackage | Where-Object { $_.Name -like '*IntelligentTerminal*' } | Select-Object -First 1
$logDir = "$env:LOCALAPPDATA\Packages\$($pkg.PackageFamilyName)\LocalCache\Local\IntelligentTerminal\logs"

# Unpackaged:
$logDir = "$env:LOCALAPPDATA\IntelligentTerminal\logs"

Get-Content "$logDir\wta-ensure-host.log" -Tail 20
```

Look for `target: "autofix"` lines — they show received events, classification, and whether auto-fix was triggered.

## Behavior Notes

- **One-shot**: Auto-fix triggers only once per user prompt. After a fix is suggested (whether accepted or not), it won't trigger again until the user manually submits a new prompt. This prevents cascading loops.
- **Idle only**: Auto-fix only fires when the agent is connected and not already processing a request.
- **Own-pane filtering**: Events from WTA's own pane are ignored to avoid self-triggering.
- **Buffer context**: When auto-fix triggers, it reads the last ~30 lines from the failing pane to provide error context to the AI.
