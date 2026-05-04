# send-event.ps1 — Forward Gemini CLI hook events to WTA via wtcli
# (Gemini variant: defaults WTA_CLI_SOURCE to "gemini" so the central registry
# tags entries correctly. Otherwise identical to the Copilot/Claude plugin's
# script in wta/agent-hooks-plugin/hooks/send-event.ps1.)
param([string]$EventType = "agent.hook")

# Tag this CLI as Gemini for the wrapper payload.
if (-not $env:WTA_CLI_SOURCE) { $env:WTA_CLI_SOURCE = "gemini" }

# Skip if not running inside Windows Terminal
if (-not $env:WT_COM_CLSID) { exit 0 }

# Locate wtcli.exe. Order:
#   1. PATH (works if the package registers a wtcli AppExecutionAlias).
#   2. $env:WTCLI_PATH override (escape hatch for dev builds / debugging).
#   3. The Windows Terminal package InstallLocation (where the build drops it).
$wtcliPath = (Get-Command wtcli -ErrorAction SilentlyContinue).Source
if (-not $wtcliPath -and $env:WTCLI_PATH -and (Test-Path $env:WTCLI_PATH)) {
    $wtcliPath = $env:WTCLI_PATH
}
if (-not $wtcliPath) {
    try {
        $pkgs = Get-AppxPackage -Name "*Terminal*" -ErrorAction SilentlyContinue
        foreach ($pkg in $pkgs) {
            $candidate = Join-Path $pkg.InstallLocation "wtcli.exe"
            if (Test-Path $candidate) { $wtcliPath = $candidate; break }
        }
    } catch { }
}
if (-not $wtcliPath) { exit 0 }

# Read hook JSON from stdin
$hookData = [Console]::In.ReadToEnd()
if (-not $hookData -or -not $hookData.Trim()) { exit 0 }

try {
    $parsed = $hookData | ConvertFrom-Json

    # Extract agent_session_id from stdin JSON or env (Gemini puts it in stdin's
    # session_id field per the hooks reference).
    $agentSessionId = ""
    if ($parsed.PSObject.Properties.Name -contains "session_id") {
        $agentSessionId = [string]$parsed.session_id
    } elseif ($env:GEMINI_SESSION_ID) {
        $agentSessionId = $env:GEMINI_SESSION_ID
    }

    $wrapper = @{
        cli_source       = $env:WTA_CLI_SOURCE
        agent_session_id = $agentSessionId
        payload          = $parsed
    }

    $payload = $wrapper | ConvertTo-Json -Compress -Depth 5

    # Escape quotes for raw command line: each " becomes \"
    $escaped = $payload.Replace('"', '\"')
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
