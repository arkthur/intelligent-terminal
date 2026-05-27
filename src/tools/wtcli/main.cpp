// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include <unknwn.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Microsoft.Terminal.Protocol.h>

#include "Formatting.h"
#include "wtcli_functions.h"

#include <CLI/CLI.hpp>

#include <chrono>
#include <cstdio>
#include <fcntl.h>
#include <io.h>
#include <sstream>
#include <string>
#include <thread>

namespace Protocol = winrt::Microsoft::Terminal::Protocol;

// ── EventCallback — receives push-based events from Terminal ──

struct EventCallback : winrt::implements<EventCallback, Protocol::IProtocolEventCallback>
{
    EventCallback(std::function<void(winrt::hstring const&)> handler) :
        _handler(std::move(handler)) {}

    void OnEvent(winrt::hstring const& eventJson)
    {
        if (_handler)
            _handler(eventJson);
    }

private:
    std::function<void(winrt::hstring const&)> _handler;
};

// ── Helpers ──

static Protocol::IProtocolServer ConnectToTerminal(Protocol::AuthResult* outAuth = nullptr)
{
    wchar_t clsid[128]{};
    if (!GetEnvironmentVariableW(L"WT_COM_CLSID", clsid, ARRAYSIZE(clsid)))
    {
        fprintf(stderr, "[wtcli] WT_COM_CLSID not set. Must run inside a Windows Terminal pane.\n");
        return nullptr;
    }

    CLSID cls{};
    if (FAILED(CLSIDFromString(clsid, &cls)))
    {
        fprintf(stderr, "[wtcli] Invalid CLSID: %ls\n", clsid);
        return nullptr;
    }

    try
    {
        auto server = winrt::create_instance<Protocol::IProtocolServer>(cls, CLSCTX_LOCAL_SERVER);
        auto authResult = server.Authenticate(L"");
        if (!authResult.Authenticated)
        {
            fprintf(stderr, "[wtcli] Authentication failed\n");
            return nullptr;
        }
        if (outAuth)
            *outAuth = authResult;
        return server;
    }
    catch (const winrt::hresult_error& e)
    {
        fprintf(stderr, "[wtcli] Connection failed: 0x%08X %ls\n",
                static_cast<uint32_t>(e.code()), e.message().c_str());
        return nullptr;
    }
}

static Protocol::IProtocolServer ConnectToTerminalQuiet()
{
    // Variant of ConnectToTerminal() that NEVER writes to stderr. The
    // forward-hook subcommand is invoked by Claude/Copilot/Gemini hook
    // pipelines; per their contracts, stdout/stderr from the hook command
    // are observed (stdout is fed into the model context for
    // SessionStart/UserPromptSubmit, stderr surfaces as "<hook> hook error"
    // in the transcript). Any noise we emit here leaks tokens or breaks UX,
    // so we silently return nullptr on every failure path.
    wchar_t clsid[128]{};
    if (!GetEnvironmentVariableW(L"WT_COM_CLSID", clsid, ARRAYSIZE(clsid)))
        return nullptr;

    CLSID cls{};
    if (FAILED(CLSIDFromString(clsid, &cls)))
        return nullptr;

    try
    {
        auto server = winrt::create_instance<Protocol::IProtocolServer>(cls, CLSCTX_LOCAL_SERVER);
        auto authResult = server.Authenticate(L"");
        if (!authResult.Authenticated)
            return nullptr;
        return server;
    }
    catch (...)
    {
        return nullptr;
    }
}

static winrt::guid ResolveSessionId(const Protocol::IProtocolServer& server, const std::string& target)
{
    if (!target.empty())
    {
        // Accept both plain and braced GUID formats
        auto wstr = winrt::to_hstring(target);
        std::wstring guidStr{ wstr };
        if (!guidStr.empty() && guidStr[0] != L'{')
            guidStr = L"{" + guidStr + L"}";
        GUID g{};
        if (SUCCEEDED(CLSIDFromString(guidStr.c_str(), &g)))
            return winrt::guid{ g };
        fprintf(stderr, "[wtcli] Invalid session ID: %s\n", target.c_str());
        return {};
    }
    auto info = server.GetActivePane();
    return info.SessionId;
}

static std::string GuidToString(const winrt::guid& g)
{
    wchar_t buf[40]{};
    StringFromGUID2(g, buf, ARRAYSIZE(buf));
    std::wstring ws(buf);
    if (ws.size() > 2 && ws.front() == L'{' && ws.back() == L'}')
        ws = ws.substr(1, ws.size() - 2);
    return winrt::to_string(winrt::hstring{ ws });
}

static uint64_t GetFirstWindowId(const Protocol::IProtocolServer& server)
{
    auto windows = server.ListWindows();
    if (windows.size() > 0)
        return windows[0].WindowId;
    return 0;
}

static uint32_t GetFirstTabId(const Protocol::IProtocolServer& server, uint64_t windowId)
{
    auto tabs = server.ListTabs(windowId);
    if (tabs.size() > 0)
        return tabs[0].TabId;
    return UINT32_MAX;
}

// ── Hook bridge support (forward-hook subcommand) ──
//
// These helpers exist *only* for `forward-hook`. They are intentionally
// silent — no fprintf, no exceptions escaping — because hook stdout/stderr
// is observed by the agent CLI (Claude/Copilot/Gemini) and any noise either
// leaks tokens into the model context or surfaces as a "hook error" toast.

// Read all of stdin into a string. Returns empty on any I/O failure.
// May legitimately return empty: some hook events (SessionEnd, AfterTool)
// carry no payload, but we still want them to reach WTA so the agent state
// transitions out of Working back to Idle.
static std::string ReadAllStdin()
{
    std::string out;
    try
    {
        // Binary mode so CRLF translation doesn't corrupt JSON payloads
        // containing literal '\r' inside strings.
        _setmode(_fileno(stdin), _O_BINARY);
        constexpr size_t chunk = 4096;
        char buf[chunk];
        while (true)
        {
            const auto n = fread(buf, 1, chunk, stdin);
            if (n == 0)
                break;
            out.append(buf, n);
            if (out.size() > 1 * 1024 * 1024)
            {
                // 1 MB hard cap — hook payloads should be tiny (a few KB
                // of JSON metadata). Anything bigger is either runaway
                // tool output that should have been stripped upstream or
                // an attack vector; either way we'd rather drop than ship.
                break;
            }
        }
    }
    catch (...)
    {
    }
    return out;
}

static std::wstring GetEnvW(const wchar_t* name)
{
    wchar_t buf[1024]{};
    const DWORD n = GetEnvironmentVariableW(name, buf, ARRAYSIZE(buf));
    if (n == 0 || n >= ARRAYSIZE(buf))
        return {};
    return std::wstring(buf, n);
}

static std::string GetEnvUtf8(const wchar_t* name)
{
    const auto w = GetEnvW(name);
    if (w.empty())
        return {};
    return winrt::to_string(winrt::hstring{ w });
}

// Append one line to `%LOCALAPPDATA%\IntelligentTerminal\logs\hook-trace.log`,
// best-effort, never throws. Rotates the file when it crosses 5 MB by renaming
// to `.1` (overwriting any prior `.1`). Rotation is racy under concurrent fire
// — the rename may fail or interleave — and that's deliberately acceptable; a
// missed line never blocks or fails the hook.
static void AppendHookTrace(const std::string& line)
{
    try
    {
        const auto root = GetEnvW(L"LOCALAPPDATA");
        if (root.empty())
            return;

        const std::wstring dir = root + L"\\IntelligentTerminal\\logs";
        // SHCreateDirectoryExW would be ideal but pulls in shell32; build the
        // path piecewise instead.
        CreateDirectoryW((root + L"\\IntelligentTerminal").c_str(), nullptr);
        CreateDirectoryW(dir.c_str(), nullptr);

        const std::wstring path = dir + L"\\hook-trace.log";
        const std::wstring rotated = path + L".1";

        // Best-effort rotation. WIN32_FILE_ATTRIBUTE_DATA is enough to read
        // the size without locking the file.
        WIN32_FILE_ATTRIBUTE_DATA attr{};
        if (GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &attr))
        {
            const uint64_t size =
                (uint64_t{ attr.nFileSizeHigh } << 32) | uint64_t{ attr.nFileSizeLow };
            if (size > 5 * 1024 * 1024)
            {
                DeleteFileW(rotated.c_str());
                MoveFileW(path.c_str(), rotated.c_str());
            }
        }

        // Open with FILE_SHARE_READ|FILE_SHARE_WRITE so concurrent hook
        // fires don't lock each other out. OPEN_ALWAYS + SetFilePointer(END)
        // appends; on Windows there is no native O_APPEND-style atomic append
        // for plain files, so interleaving is possible — that's acceptable
        // here, the trace is for human troubleshooting, not parsing.
        HANDLE h = CreateFileW(
            path.c_str(),
            FILE_APPEND_DATA,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            nullptr,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        if (h == INVALID_HANDLE_VALUE)
            return;
        DWORD written = 0;
        WriteFile(h, line.data(), static_cast<DWORD>(line.size()), &written, nullptr);
        constexpr char newline = '\n';
        WriteFile(h, &newline, 1, &written, nullptr);
        CloseHandle(h);
    }
    catch (...)
    {
    }
}

static std::string CurrentTimestamp()
{
    SYSTEMTIME t{};
    GetLocalTime(&t);
    char buf[32];
    std::snprintf(buf, sizeof(buf), "%04u-%02u-%02u %02u:%02u:%02u.%03u",
                  t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds);
    return std::string(buf);
}

// Watchdog: forcibly terminate the process after `timeoutMs` to bound the
// hook's wall-clock cost. Old send-event.ps1 killed its wtcli child after
// 5s; we apply the same budget to the whole forward-hook process. Returns
// the thread handle so callers can leave it running and let it self-destruct
// at process exit (we never join — the whole point is "fire and forget,
// process dies one way or another").
static void StartHookWatchdog(DWORD timeoutMs)
{
    auto* arg = new DWORD{ timeoutMs };
    HANDLE th = CreateThread(
        nullptr, 0,
        [](LPVOID p) -> DWORD {
            const DWORD ms = *static_cast<DWORD*>(p);
            delete static_cast<DWORD*>(p);
            Sleep(ms);
            // Best-effort: append a final trace line so debugging shows the
            // timeout was hit. Use a tight call (no I/O retry) to keep
            // shutdown fast.
            AppendHookTrace(CurrentTimestamp() + " | TIMEOUT (forward-hook exceeded budget)");
            TerminateProcess(GetCurrentProcess(), 0);
            return 0;
        },
        arg, 0, nullptr);
    if (th)
        CloseHandle(th); // detach
}

// ── Main ──

// Detect `forward-hook` / `fh` subcommand by scanning argv before CLI11
// parsing. When present, redirect stdout AND stderr to NUL for the rest
// of the process lifetime. This silences not only our own writes but also
// CLI11's auto-printed help / parse-error messages, satisfying the hook
// contract even when the caller passes garbled args. Returns true iff
// forward-hook mode was detected.
//
// Uses `CommandLineToArgvW(GetCommandLineW(), ...)` rather than the CRT's
// `__wargv` because that global is `nullptr` for programs declared with
// `int main()` (it is only initialized for `wmain`). Dereferencing it
// would crash the process before any breadcrumb could be written.
static bool SilenceForwardHookIfApplicable()
{
    int wargc = 0;
    LPWSTR* wargv = ::CommandLineToArgvW(::GetCommandLineW(), &wargc);
    if (!wargv)
        return false;

    bool isHook = false;
    for (int i = 1; i < wargc; ++i)
    {
        const std::wstring a = wargv[i];
        if (a.empty() || a[0] == L'-')
            continue;
        if (a == L"forward-hook" || a == L"fh")
            isHook = true;
        break;
    }
    ::LocalFree(wargv);

    if (isHook)
    {
        FILE* dummy = nullptr;
        _wfreopen_s(&dummy, L"NUL", L"w", stdout);
        _wfreopen_s(&dummy, L"NUL", L"w", stderr);
    }
    return isHook;
}

int main()
{
    const bool hookMode = SilenceForwardHookIfApplicable();

    winrt::init_apartment(winrt::apartment_type::multi_threaded);

    CLI::App app{ "wtcli — Windows Terminal CLI" };
    app.require_subcommand(0, 1);

    // Global options
    bool jsonMode = false;
    int exitCode = 0;
    app.add_flag("--json", jsonMode, "Output raw JSON");

    // Helper: connect to Windows Terminal
    auto connect = [&]() -> Protocol::IProtocolServer {
        auto server = ConnectToTerminal();
        if (!server)
            exitCode = 1;
        return server;
    };

    // ── list-windows ──
    auto* listWindowsCmd = app.add_subcommand("list-windows", "List all windows")->alias("lsw");
    listWindowsCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto windows = server.ListWindows();
            if (jsonMode)
            {
                Json::Value arr(Json::objectValue);
                Json::Value list(Json::arrayValue);
                for (const auto& w : windows) list.append(WindowInfoToJson(w));
                arr["windows"] = list;
                PrintJson(arr);
            }
            else
            {
                FormatWindowsHuman(windows);
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "ListWindows failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── list-tabs ──
    std::string listTabsWindowId;
    auto* listTabsCmd = app.add_subcommand("list-tabs", "List tabs in a window")->alias("lst");
    listTabsCmd->add_option("-w,--window-id", listTabsWindowId, "Window ID");
    listTabsCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            uint64_t wid = listTabsWindowId.empty() ? GetFirstWindowId(server) : std::stoull(listTabsWindowId);
            auto tabs = server.ListTabs(wid);
            if (jsonMode)
            {
                Json::Value arr(Json::objectValue);
                Json::Value list(Json::arrayValue);
                for (const auto& t : tabs) list.append(TabInfoToJson(t));
                arr["tabs"] = list;
                PrintJson(arr);
            }
            else
            {
                FormatTabsHuman(tabs);
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "ListTabs failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── list-panes ──
    std::string listPanesTabId, listPanesWindowId;
    auto* listPanesCmd = app.add_subcommand("list-panes", "List panes in a tab")->alias("lsp");
    listPanesCmd->add_option("-t,--tab-id", listPanesTabId, "Tab ID");
    listPanesCmd->add_option("-w,--window-id", listPanesWindowId, "Window ID");
    listPanesCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            uint64_t wid = listPanesWindowId.empty() ? 0 : std::stoull(listPanesWindowId);
            uint32_t tid = listPanesTabId.empty() ? UINT32_MAX : static_cast<uint32_t>(std::stoul(listPanesTabId));
            if (tid == UINT32_MAX)
            {
                if (wid == 0) wid = GetFirstWindowId(server);
                tid = GetFirstTabId(server, wid);
            }
            auto panes = server.ListPanes(wid, tid);
            if (jsonMode)
            {
                Json::Value arr(Json::objectValue);
                Json::Value list(Json::arrayValue);
                for (const auto& p : panes) list.append(PaneInfoToJson(p));
                arr["panes"] = list;
                PrintJson(arr);
            }
            else
            {
                FormatPanesHuman(panes);
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "ListPanes failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── active-pane ──
    auto* activePaneCmd = app.add_subcommand("active-pane", "Show the currently active pane");
    activePaneCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto info = server.GetActivePane();
            if (jsonMode)
                PrintJson(PaneInfoToJson(info));
            else
                FormatActivePaneHuman(info);
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "GetActivePane failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── capture-pane ──
    std::string capturePaneTarget;
    int captureMaxLines = 200;
    bool captureLastPrompt = false;
    auto* capturePaneCmd = app.add_subcommand("capture-pane", "Capture pane output")->alias("capturep");
    capturePaneCmd->add_option("-t,--target", capturePaneTarget, "Session ID (GUID)");
    capturePaneCmd->add_option("-l,--max-lines", captureMaxLines, "Max lines");
    capturePaneCmd->add_flag("--last-prompt", captureLastPrompt,
        "Only return the most recent completed shell prompt (command + output, requires OSC 133 shell integration)");
    capturePaneCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, capturePaneTarget);
            const auto sourceArg = captureLastPrompt ? L"last_prompt" : L"scrollback";
            auto output = server.ReadPaneOutput(sessionId, sourceArg, captureMaxLines);
            if (jsonMode)
            {
                PrintJson(PaneOutputToJson(output));
            }
            else
            {
                auto content = winrt::to_string(output.Content);
                printf("%s\n", content.c_str());
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "ReadPaneOutput failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── pane-status ──
    std::string paneStatusTarget;
    auto* paneStatusCmd = app.add_subcommand("pane-status", "Show pane process status");
    paneStatusCmd->add_option("-t,--target", paneStatusTarget, "Session ID (GUID)");
    paneStatusCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, paneStatusTarget);
            auto status = server.GetProcessStatus(sessionId);
            if (jsonMode)
            {
                Json::Value v;
                v["session_id"] = GuidToString(status.SessionId);
                v["state"] = winrt::to_string(status.State);
                v["pid"] = static_cast<Json::UInt>(status.Pid);
                if (status.HasExitCode) v["exit_code"] = status.ExitCode;
                PrintJson(v);
            }
            else
            {
                FormatPaneStatusHuman(status);
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "GetProcessStatus failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── new-tab ──
    std::string newTabCommand, newTabTitle, newTabCwd;
    auto* newTabCmd = app.add_subcommand("new-tab", "Create a new tab")->alias("neww");
    newTabCmd->add_option("-c,--command", newTabCommand, "Command to run");
    newTabCmd->add_option("-n,--title", newTabTitle, "Tab title");
    newTabCmd->add_option("-d,--cwd", newTabCwd, "Starting directory");
    newTabCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto result = server.CreateTab(
                0, L"",
                winrt::to_hstring(newTabCommand),
                winrt::to_hstring(newTabTitle),
                winrt::to_hstring(newTabCwd),
                false, true);
            if (jsonMode)
                PrintJson(CreationResultToJson(result));
            else
                FormatCreatedTabHuman(result);
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "CreateTab failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── split-pane ──
    std::string splitPaneTarget, splitPaneCommand, splitPaneDirection;
    bool splitHorizontal = false, splitVertical = false;
    double splitSize = 0.5;
    auto* splitPaneCmd = app.add_subcommand("split-pane", "Split a pane")->alias("splitw");
    splitPaneCmd->add_option("-t,--target", splitPaneTarget, "Session ID (GUID)");
    splitPaneCmd->add_option("-d,--direction", splitPaneDirection, "Split direction: right|left|up|down|auto");
    splitPaneCmd->add_flag("-H,--horizontal", splitHorizontal, "Split horizontally (legacy alias for --direction down)");
    splitPaneCmd->add_flag("-v,--vertical", splitVertical, "Split vertically (legacy alias for --direction right)");
    splitPaneCmd->add_option("-s,--size", splitSize, "Size fraction");
    splitPaneCmd->add_option("-c,--command", splitPaneCommand, "Command to run");
    splitPaneCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, splitPaneTarget);
            // --direction wins over the legacy boolean flags. If neither is
            // given, send "automatic" so the COM server picks the longer
            // dimension (matches the WT default for `splitPane`).
            std::wstring dir;
            if (!splitPaneDirection.empty())
                dir = winrt::to_hstring(splitPaneDirection).c_str();
            else if (splitHorizontal)
                dir = L"down";
            else if (splitVertical)
                dir = L"right";
            else
                dir = L"automatic";
            auto result = server.SplitPane(
                sessionId, winrt::hstring{ dir }, static_cast<float>(splitSize),
                L"", winrt::to_hstring(splitPaneCommand), true);
            if (jsonMode)
                PrintJson(CreationResultToJson(result));
            else
                FormatCreatedPaneHuman(result);
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "SplitPane failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── kill-pane ──
    std::string killPaneTarget;
    auto* killPaneCmd = app.add_subcommand("kill-pane", "Close a pane")->alias("killp");
    killPaneCmd->add_option("-t,--target", killPaneTarget, "Session ID (GUID)");
    killPaneCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, killPaneTarget);
            server.ClosePane(sessionId);
            if (jsonMode)
            {
                Json::Value v;
                v["ok"] = true;
                v["session_id"] = GuidToString(sessionId);
                PrintJson(v);
            }
            else
            {
                printf("Session %s closed.\n", GuidToString(sessionId).c_str());
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "ClosePane failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── send-keys ──
    std::string sendKeysTarget;
    std::vector<std::string> sendKeysArgs;
    bool sendKeysRaw = false;
    auto* sendKeysCmd = app.add_subcommand("send-keys", "Send keys to a pane")->alias("send");
    sendKeysCmd->add_option("-t,--target", sendKeysTarget, "Session ID (GUID)");
    sendKeysCmd->add_flag("--raw", sendKeysRaw,
                          "Treat the payload as literal UTF-8 text — skip tmux-style "
                          "token translation (Enter/Tab/Escape/BSpace/C-x). Use this when "
                          "forwarding arbitrary agent-supplied text.");
    sendKeysCmd->add_option("keys", sendKeysArgs, "Keys to send")->required();
    sendKeysCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, sendKeysTarget);
            auto text = sendKeysRaw
                ? wtcli::JoinAsUtf16(sendKeysArgs)
                : wtcli::TranslateKeys(sendKeysArgs);
            server.SendInput(sessionId, text);
            if (jsonMode)
            {
                Json::Value v;
                v["ok"] = true;
                v["session_id"] = GuidToString(sessionId);
                PrintJson(v);
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "SendInput failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── focus-pane ──
    std::string focusPaneTarget;
    auto* focusPaneCmd = app.add_subcommand("focus-pane", "Switch focus to a pane")->alias("focusp");
    focusPaneCmd->add_option("-t,--target", focusPaneTarget, "Session ID (GUID)");
    focusPaneCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        try
        {
            auto sessionId = ResolveSessionId(server, focusPaneTarget);
            server.FocusPane(sessionId);
            if (jsonMode)
            {
                Json::Value v;
                v["ok"] = true;
                v["session_id"] = GuidToString(sessionId);
                PrintJson(v);
            }
            else
            {
                printf("Focused pane %s.\n", GuidToString(sessionId).c_str());
            }
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "FocusPane failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── test-pipe ──
    auto* testPipeCmd = app.add_subcommand("test-pipe", "Test connection to Windows Terminal");
    testPipeCmd->callback([&]() {
        printf("Connecting to Windows Terminal...\n");
        auto server = connect();
        if (!server) { fprintf(stderr, "Connection failed.\n"); return; }
        printf("Connected and authenticated!\n\n");

        try
        {
            auto windows = server.ListWindows();
            Json::Value arr(Json::objectValue);
            Json::Value list(Json::arrayValue);
            for (const auto& w : windows) list.append(WindowInfoToJson(w));
            arr["windows"] = list;
            printf("list_windows:\n");
            PrintJson(arr);
        }
        catch (const winrt::hresult_error&) {}

        printf("\n");

        try
        {
            auto capsJson = server.GetCapabilities();
            Json::Value cap;
            Json::CharReaderBuilder rb;
            std::string errs;
            auto capsStr = winrt::to_string(capsJson);
            std::istringstream ss(capsStr);
            Json::parseFromStream(rb, ss, &cap, &errs);
            printf("get_capabilities:\n");
            PrintJson(cap);
        }
        catch (const winrt::hresult_error&) {}
    });

    // ── info ──
    auto* infoCmd = app.add_subcommand("info", "Show connection info");
    infoCmd->callback([&]() {
        wchar_t clsid[128]{};
        auto hasClsid = GetEnvironmentVariableW(L"WT_COM_CLSID", clsid, ARRAYSIZE(clsid)) > 0;

        Protocol::AuthResult authResult{};
        auto server = ConnectToTerminal(&authResult);
        auto version = server ? winrt::to_string(authResult.ProtocolVersion) : std::string{};

        Json::Value methods(Json::arrayValue);
        if (server)
        {
            try
            {
                auto capsJson = server.GetCapabilities();
                Json::Value cap;
                Json::CharReaderBuilder rb;
                std::string errs;
                auto capsStr = winrt::to_string(capsJson);
                std::istringstream ss(capsStr);
                if (Json::parseFromStream(rb, ss, &cap, &errs) && cap.isArray())
                    methods = cap;
            }
            catch (const winrt::hresult_error&) {}
        }

        if (jsonMode)
        {
            Json::Value v;
            if (hasClsid)
                v["com_clsid"] = winrt::to_string(winrt::hstring{ clsid });
            v["connected"] = (server != nullptr);
            if (!version.empty())
                v["protocol_version"] = version;
            v["methods"] = methods;
            PrintJson(v);
        }
        else
        {
            printf("Windows Terminal Protocol Info\n");
            printf("========================================\n");
            if (hasClsid)
                printf("  COM CLSID:  %ls\n", clsid);
            else
                printf("  COM CLSID:  (not set)\n");
            printf("\n");
            if (!server)
            {
                printf("  Connection: FAILED\n");
            }
            else
            {
                printf("  Connection: OK\n");
                if (!version.empty())
                    printf("  Protocol:   %s\n", version.c_str());
                printf("\n");
                if (methods.size() > 0)
                {
                    printf("  Methods:    %u supported\n", methods.size());
                    for (const auto& m : methods)
                        printf("              - %s\n", m.asString().c_str());
                }
            }
        }

        if (!server)
            exitCode = 1;
    });

    // ── wait-for ──
    std::string waitForTarget;
    int waitInterval = 500;
    int waitTimeout = 0;
    auto* waitForCmd = app.add_subcommand("wait-for", "Wait for a pane to exit");
    waitForCmd->add_option("-t,--target", waitForTarget, "Session ID (GUID)")->required();
    waitForCmd->add_option("--interval", waitInterval, "Poll interval (ms)");
    waitForCmd->add_option("--timeout", waitTimeout, "Timeout (seconds, 0=forever)");
    waitForCmd->callback([&]() {
        auto server = connect();
        if (!server) return;
        // Parse target as GUID
        auto sessionId = ResolveSessionId(server, waitForTarget);
        auto start = std::chrono::steady_clock::now();

        while (true)
        {
            try
            {
                auto status = server.GetProcessStatus(sessionId);
                auto state = winrt::to_string(status.State);
                if (state == "exited")
                {
                    if (jsonMode)
                    {
                        Json::Value v;
                        v["state"] = state;
                        v["exit_code"] = status.ExitCode;
                        PrintJson(v);
                    }
                    else
                    {
                        printf("Process exited");
                        if (status.HasExitCode)
                            printf(" (code %d)", status.ExitCode);
                        printf("\n");
                    }
                    return;
                }
            }
            catch (const winrt::hresult_error& e)
            {
                fprintf(stderr, "GetProcessStatus failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
                exitCode = 1;
                return;
            }

            if (waitTimeout > 0)
            {
                auto elapsed = std::chrono::duration_cast<std::chrono::seconds>(
                    std::chrono::steady_clock::now() - start).count();
                if (elapsed >= waitTimeout)
                {
                    fprintf(stderr, "Timeout waiting for pane %s\n", waitForTarget.c_str());
                    exitCode = 1;
                    return;
                }
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(waitInterval));
        }
    });

    // ── set-env ──
    std::string setEnvShell = "powershell";
    auto* setEnvCmd = app.add_subcommand("set-env", "Print env setup commands")->alias("setenv");
    setEnvCmd->add_option("-s,--shell", setEnvShell, "Shell: powershell, bash, cmd");
    setEnvCmd->callback([&]() {
        wchar_t clsid[128]{};
        GetEnvironmentVariableW(L"WT_COM_CLSID", clsid, ARRAYSIZE(clsid));

        auto cl = winrt::to_string(winrt::hstring{ clsid });

        if (setEnvShell == "powershell" || setEnvShell == "pwsh")
        {
            if (!cl.empty()) printf("$env:WT_COM_CLSID = '%s'\n", cl.c_str());
        }
        else if (setEnvShell == "bash" || setEnvShell == "sh" || setEnvShell == "zsh")
        {
            if (!cl.empty()) printf("export WT_COM_CLSID='%s'\n", cl.c_str());
        }
        else if (setEnvShell == "cmd")
        {
            if (!cl.empty()) printf("set WT_COM_CLSID=%s\n", cl.c_str());
        }
    });

    // ── publish ──
    // Low-level "pass this JSON through to IProtocolServer::SendEvent verbatim"
    // escape hatch, for event shapes that don't fit the legacy send-event
    // envelope (method=agent_event, params.event required). Examples:
    // autofix_state updates from WTA that the COM server dispatches directly
    // to TerminalPage rather than broadcasting.
    std::string publishJson;
    auto* publishCmd = app.add_subcommand("publish", "Forward raw JSON to IProtocolServer::SendEvent");
    publishCmd->add_option("json", publishJson, "Full event JSON (e.g. {\"method\":\"autofix_state\",\"params\":{...}})")->required();
    publishCmd->callback([&]() {
        auto server = connect();
        if (!server)
        {
            return;
        }
        try
        {
            server.SendEvent(winrt::to_hstring(publishJson));
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "publish failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── send-event ──
    std::string sendEventType, sendEventJson, sendEventPaneTarget;
    auto* sendEventCmd = app.add_subcommand("send-event", "Publish an event to all listeners")->alias("se");
    sendEventCmd->add_option("-p,--pane", sendEventPaneTarget, "Source session ID (GUID)");
    sendEventCmd->add_option("-e,--event", sendEventType, "Event type (e.g. agent.task.started)")->required();
    sendEventCmd->add_option("json", sendEventJson, "Event params as JSON object");
    sendEventCmd->callback([&]() {
        auto server = connect();
        if (!server)
            return;
        try
        {
            Json::Value evt;
            auto resolvedSessionId = !sendEventPaneTarget.empty()
                ? sendEventPaneTarget
                : GuidToString(ResolveSessionId(server, ""));
            if (!wtcli::BuildSendEventJson(sendEventType, sendEventJson, resolvedSessionId, evt))
            {
                fprintf(stderr, "Invalid JSON for --json: value must be a JSON object (e.g. '{\"key\":\"val\"}')\n");
                exitCode = 1;
                return;
            }

            Json::StreamWriterBuilder wb;
            wb["indentation"] = "";
            server.SendEvent(winrt::to_hstring(Json::writeString(wb, evt)));
        }
        catch (const winrt::hresult_error& e)
        {
            fprintf(stderr, "SendEvent failed: 0x%08X\n", static_cast<uint32_t>(e.code()));
            exitCode = 1;
        }
    });

    // ── forward-hook ───────────────────────────────────────────────────
    //
    // The hook bridge invoked by Claude / Copilot / Gemini hook
    // pipelines, replacing the prior `send-event.ps1` PowerShell script.
    // Each agent CLI fires this subcommand with `-c <cli> -e <event>`
    // and pipes its hook JSON to stdin; we wrap it into a
    // `{cli_source, agent_session_id, payload}` envelope and SendEvent
    // it to the WT COM server as an `agent_event`.
    //
    // STRICT CONTRACT (see send-event.ps1 header comments for rationale):
    //   * Always exit 0. Non-zero exit codes block tool calls (exit 2) or
    //     show "<hook> hook error" toasts (other non-zero) in the agent
    //     transcript.
    //   * Never write to stdout. SessionStart / UserPromptSubmit stdout is
    //     fed into the model context — leaks tokens, attack vector.
    //   * Never write to stderr. Surfaces as a transcript error.
    //   * Bounded runtime. A 5s watchdog terminates the process if the
    //     COM call hangs.
    //
    // All failure paths are silent and exit 0.
    std::string fwdHookEvent, fwdHookCli;
    auto* fwdHookCmd = app.add_subcommand("forward-hook",
        "Internal: bridge an agent-CLI hook fire into a WT agent_event")
        ->alias("fh");
    fwdHookCmd->add_option("-e,--event", fwdHookEvent,
        "WTA topic name (e.g. agent.session.start)")->required();
    fwdHookCmd->add_option("-c,--cli-source", fwdHookCli,
        "CLI source: claude | copilot | gemini")->required();
    fwdHookCmd->callback([&]() {
        // Force-disable the outer exitCode bookkeeping; this subcommand
        // must return 0 regardless of what happens inside.
        exitCode = 0;

        try
        {
            // Bound the process lifetime. The watchdog runs even if COM
            // never returns. Override via WTA_HOOK_TIMEOUT_MS for testing.
            DWORD timeoutMs = 5000;
            if (const auto envTo = GetEnvUtf8(L"WTA_HOOK_TIMEOUT_MS"); !envTo.empty())
            {
                try { timeoutMs = static_cast<DWORD>(std::stoul(envTo)); }
                catch (...) {}
            }
            StartHookWatchdog(timeoutMs);

            // Resolve agent_session_id and cli_source from inputs + env.
            // Order matches send-event.ps1 to preserve compatibility for
            // older hook installs that re-trigger during a migration.
            const auto stdinJson = ReadAllStdin();

            std::string sessionId;
            {
                // First try parsing stdin to grab session_id; if that
                // fails or the field is missing, fall back to env vars
                // (Copilot CLI populates COPILOT_SESSION_ID etc).
                Json::CharReaderBuilder rb;
                std::string errs;
                std::istringstream ss(stdinJson);
                Json::Value parsed;
                if (Json::parseFromStream(rb, ss, &parsed, &errs))
                {
                    sessionId = wtcli::ExtractSessionId(parsed);
                }
                if (sessionId.empty()) sessionId = GetEnvUtf8(L"COPILOT_SESSION_ID");
                if (sessionId.empty()) sessionId = GetEnvUtf8(L"CLAUDE_SESSION_ID");
                if (sessionId.empty()) sessionId = GetEnvUtf8(L"GEMINI_SESSION_ID");
            }

            std::string cliSource = fwdHookCli;
            if (cliSource.empty()) cliSource = GetEnvUtf8(L"WTA_CLI_SOURCE");

            // Pane GUID — pass through from WT_SESSION when set. Do NOT
            // fall back to GetActivePane(): inside a multi-pane window
            // that would route every hook to the focused pane, breaking
            // F2 list routing.
            const std::string paneId = GetEnvUtf8(L"WT_SESSION");

            // ENTER breadcrumb written before the COM call so a hang or
            // timeout still leaves an audit trail.
            AppendHookTrace(CurrentTimestamp() +
                " | ENTER cli=" + cliSource +
                " event=" + fwdHookEvent +
                " sid=" + (sessionId.empty() ? "<none>" : sessionId.substr(0, 8) + "...") +
                " pane=" + (paneId.empty() ? "<no-WT_SESSION>" : paneId) +
                " pid=" + std::to_string(GetCurrentProcessId()));

            // Now connect. Silent on every failure — no stderr writes.
            auto server = ConnectToTerminalQuiet();
            if (!server)
            {
                AppendHookTrace(CurrentTimestamp() + " | SKIP no-server");
                return;
            }

            Json::Value evt;
            if (!wtcli::BuildHookEventEnvelope(stdinJson, fwdHookEvent, cliSource, sessionId, paneId, evt))
            {
                AppendHookTrace(CurrentTimestamp() + " | SKIP build-failed");
                return;
            }

            Json::StreamWriterBuilder wb;
            wb["indentation"] = "";
            const auto json = Json::writeString(wb, evt);

            try
            {
                server.SendEvent(winrt::to_hstring(json));
                AppendHookTrace(CurrentTimestamp() + " | OK bytes=" + std::to_string(json.size()));
            }
            catch (const winrt::hresult_error& e)
            {
                char hbuf[32];
                std::snprintf(hbuf, sizeof(hbuf), "0x%08X", static_cast<uint32_t>(e.code()));
                AppendHookTrace(CurrentTimestamp() + " | FAIL hr=" + hbuf);
            }
            catch (...)
            {
                AppendHookTrace(CurrentTimestamp() + " | FAIL unknown");
            }
        }
        catch (...)
        {
            // Last-resort guard. Anything escaping the inner blocks lands
            // here. Still exit 0.
        }
    });

    // ── listen ──
    std::string listenTarget;
    std::string listenEventFilter;
    auto* listenCmd = app.add_subcommand("listen", "Stream real-time events from Windows Terminal");
    listenCmd->add_option("-t,--target", listenTarget, "Filter by session ID (GUID)");
    listenCmd->add_option("--event", listenEventFilter, "Filter by event type (supports trailing wildcard, e.g. agent.*)");
    listenCmd->callback([&]() {
        auto server = ConnectToTerminal();
        if (!server) { exitCode = 1; return; }

        // Set up Ctrl-C handler to unblock the wait.
        static HANDLE s_stopEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        SetConsoleCtrlHandler([](DWORD) -> BOOL {
            SetEvent(s_stopEvent);
            return TRUE;
        }, TRUE);

        if (!jsonMode)
            fprintf(stderr, "Listening for events... (Ctrl-C to stop)\n");

        auto callback = winrt::make<EventCallback>([&](winrt::hstring const& eventJson) {
            auto eventUtf8 = winrt::to_string(eventJson);

            // Optionally filter by session_id and/or event type
            if (!wtcli::MatchesEventFilter(eventUtf8, listenTarget, listenEventFilter))
            {
                return;
            }

            printf("%s\n", eventUtf8.c_str());
            fflush(stdout);
        });

        try
        {
            server.Subscribe(callback);
        }
        catch (winrt::hresult_error const& e)
        {
            fprintf(stderr, "Subscribe failed: %ls\n", e.message().c_str());
            exitCode = 1;
            CloseHandle(s_stopEvent);
            return;
        }

        // Block until Ctrl-C.
        WaitForSingleObject(s_stopEvent, INFINITE);
        server.Unsubscribe();
        CloseHandle(s_stopEvent);
    });

    // ── Default (no subcommand) ──
    app.callback([&]() {
        if (app.get_subcommands().empty())
        {
            printf("wtcli — Windows Terminal CLI\n\n");
            printf("Usage: wtcli [--json] [--pipe-name NAME] <subcommand>\n\n");
            printf("Run 'wtcli --help' for available subcommands.\n");
        }
    });

    // CLI11_PARSE expands to a `return app.exit(e)` on parse error, which
    // returns non-zero for missing args / typos. In hook mode we MUST exit
    // 0 regardless (any non-zero shows as "<hook> hook error" in the agent
    // transcript). Expand the macro manually so we can swallow the error.
    try
    {
        app.parse(__argc, __argv);
    }
    catch (const CLI::ParseError& e)
    {
        if (hookMode)
            return 0;
        return app.exit(e);
    }

    return hookMode ? 0 : exitCode;
}
