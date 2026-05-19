// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// ShellIntegration.h
//
// Pure Win32 + STL functions for installing PowerShell shell integration
// scripts (OSC 133 prompt marks). Shared by FreOverlay (FRE wizard) and
// TerminalPage (Settings UI).
//
// The shell integration script wraps the user's prompt to emit:
//   OSC 133;D;<exit_code>  — command finished (triggers autofix)
//   OSC 133;A              — prompt started
//   OSC 133;B              — command input starts
//   OSC 9;9;"<cwd>"        — current working directory

#pragma once

#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <ShlObj.h>

namespace Microsoft::Terminal::ShellIntegration
{
    enum class Target
    {
        Pwsh,
        WindowsPowerShell,
    };

    // Result of an installation attempt.
    struct InstallResult
    {
        bool success{ false };
        bool alreadyInstalled{ false }; // true when skipped because already configured
        std::wstring errorMessage;
    };

    // Discover the PowerShell $PROFILE path.
    // Uses SHGetKnownFolderPath for the Documents folder instead of spawning
    // a shell process, which hangs indefinitely in packaged-app environments
    // (confirmed on both our FRE code and the remote's _InitShellIntegration).
    // SHGetKnownFolderPath respects OneDrive redirection and group policy.
    inline std::wstring DiscoverProfilePath(Target target)
    {
        wil::unique_cotaskmem_string documentsPath;
        if (FAILED(SHGetKnownFolderPath(FOLDERID_Documents, 0, nullptr, &documentsPath)) || !documentsPath)
        {
            return {};
        }

        std::filesystem::path profilePath{ documentsPath.get() };
        profilePath /= (target == Target::Pwsh) ? L"PowerShell" : L"WindowsPowerShell";
        profilePath /= L"Microsoft.PowerShell_profile.ps1";

        return profilePath.wstring();
    }

    // The shell integration script content.
    inline constexpr std::wstring_view ScriptContent{
        LR"(# Shell Integration — non-invasive prompt wrapper
# Emits OSC 133 (command marks / exit code) and OSC 9;9 (CWD) escape
# sequences WITHOUT altering the visual appearance of the user's prompt.
#
# USAGE: dot-source this AFTER the user's profile has loaded:
#   . "path\to\shell-integration.ps1"
#
# Compatible with Windows PowerShell 5.1+ and PowerShell 7+.
# Safe to source multiple times (idempotent guard).

if (-not $Global:__ShellInteg_Installed) {

    # ── Escape characters (PS 5.1 doesn't support `e / `a literals) ──
    $Global:__ShellInteg_ESC = [char]0x1B   # ESC
    $Global:__ShellInteg_BEL = [char]0x07   # BEL (OSC string terminator)

    # ── Snapshot the user's current prompt before we touch it ──────────
    $Global:__ShellInteg_OriginalPrompt = $function:prompt
    $Global:__ShellInteg_LastHistoryId  = -1
    $Global:__ShellInteg_Installed      = $true

    function Global:__ShellInteg_GetLastExitCode {
        # $? still reflects the *user's* last command here because this
        # is the very first call inside the prompt function.
        if ($? -eq $True) { return 0 }
        $entry = Get-History -Count 1
        if ($entry -and $Error[0].InvocationInfo.HistoryId -eq $entry.Id) {
            return -1          # PowerShell-level error
        }
        return $LastExitCode   # native command exit code
    }

    function prompt {
        # ── Capture exit code FIRST — before anything else can clobber $? ──
        $gle   = $(__ShellInteg_GetLastExitCode)
        $entry = Get-History -Count 1
        $loc   = $executionContext.SessionState.Path.CurrentLocation
        $E     = $Global:__ShellInteg_ESC
        $B     = $Global:__ShellInteg_BEL

        $prefix = ''
        $suffix = ''

        # ── Previous command finished (OSC 133;D with exit code) ──
        if ($entry -and $entry.Id -ne $Global:__ShellInteg_LastHistoryId) {
            $prefix += "${E}]133;D;${gle}${B}"
        }

        # ── Prompt started (OSC 133;A) ──
        $prefix += "${E}]133;A${B}"

        # ── Report current working directory (OSC 9;9) ──
        $prefix += "${E}]9;9;`"${loc}`"${B}"

        # ── Prompt ended, command input starts (OSC 133;B) ──
        $suffix = "${E}]133;B${B}"

        # ── Delegate to the user's ORIGINAL prompt — visual output is theirs ──
        $originalOutput = & $Global:__ShellInteg_OriginalPrompt

        $Global:__ShellInteg_LastHistoryId = if ($entry) { $entry.Id } else { -1 }

        return "${prefix}${originalOutput}${suffix}"
    }
}
)"
    };

    // Install shell integration for a given PowerShell profile path.
    // Writes shell-integration.ps1 next to the profile and appends a
    // dot-source line to the profile. Idempotent — skips if already configured.
    // Synchronous — call from a background thread.
    inline InstallResult Install(const std::wstring& profilePathW)
    {
        if (profilePathW.empty())
        {
            return { false, false, L"Profile path is empty" };
        }

        const std::filesystem::path profilePath{ profilePathW };
        const auto profileDir = profilePath.parent_path();
        const auto scriptPath = profileDir / L"shell-integration.ps1";

        // Check if already configured
        if (std::filesystem::exists(profilePath))
        {
            std::ifstream profileIn(profilePath, std::ios::binary);
            if (profileIn)
            {
                std::string contents((std::istreambuf_iterator<char>(profileIn)),
                                      std::istreambuf_iterator<char>());
                profileIn.close();
                if (contents.find("shell-integration.ps1") != std::string::npos)
                {
                    return { true, true, {} }; // already configured
                }
            }
        }

        // Ensure profile directory exists
        std::error_code ec;
        std::filesystem::create_directories(profileDir, ec);
        if (ec)
        {
            return { false, false, L"Failed to create profile directory" };
        }

        // Write shell-integration.ps1
        {
            std::ofstream scriptOut(scriptPath, std::ios::binary | std::ios::trunc);
            if (!scriptOut)
            {
                return { false, false, L"Failed to write shell-integration.ps1" };
            }
            const auto scriptUtf8 = til::u16u8(ScriptContent);
            scriptOut.write(scriptUtf8.data(), scriptUtf8.size());
        }

        // Back up existing $PROFILE before modifying
        if (std::filesystem::exists(profilePath))
        {
            const auto now = std::chrono::system_clock::now();
            const auto tt = std::chrono::system_clock::to_time_t(now);
            struct tm tm{};
            localtime_s(&tm, &tt);
            wchar_t timeBuf[32]{};
            wcsftime(timeBuf, std::size(timeBuf), L"%Y%m%d-%H%M%S", &tm);

            std::ifstream backupIn(profilePath, std::ios::binary);
            std::string backupContent((std::istreambuf_iterator<char>(backupIn)),
                                      std::istreambuf_iterator<char>());
            backupIn.close();
            const auto contentHash = std::hash<std::string>{}(backupContent);

            auto backupPath = profilePath.wstring() +
                L".bak." + timeBuf + L"." +
                fmt::format(FMT_COMPILE(L"{:08x}"), contentHash & 0xFFFFFFFF);
            std::filesystem::copy_file(profilePath, backupPath,
                                       std::filesystem::copy_options::overwrite_existing, ec);
            // Non-fatal if backup fails
        }

        // Append dot-source line to $PROFILE
        auto dotSourceLine = fmt::format(
            FMT_COMPILE(L"\n# Shell integration \u2014 emit OSC 133 (exit code) + OSC 9;9 (CWD) without\n"
                        L"# altering the visual prompt.  Must load LAST so it can wrap whatever\n"
                        L"# prompt function exists at this point.\n"
                        L". \"{}\""),
            scriptPath.wstring());

        {
            std::ofstream profileOut(profilePath, std::ios::binary | std::ios::app);
            if (!profileOut)
            {
                return { false, false, L"Failed to append to PowerShell profile" };
            }
            const auto lineUtf8 = til::u16u8(dotSourceLine);
            profileOut.write(lineUtf8.data(), lineUtf8.size());
        }

        return { true, false, {} };
    }

    // Convenience: discover profile path + install, for a given target.
    // Synchronous — call from a background thread.
    inline InstallResult InstallForTarget(Target target)
    {
        auto profilePath = DiscoverProfilePath(target);
        if (profilePath.empty())
        {
            return { false, false, L"Could not discover PowerShell profile path" };
        }
        return Install(profilePath);
    }
}
