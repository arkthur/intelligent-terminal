// wta/src/agent_hooks_installer.rs
//
// Auto-install the wt-agent-hooks bridge into Claude Code, Copilot CLI,
// and Gemini CLI on wta startup.
//
// Why this exists
// ===============
//
// The wta agent-pane registry transitions a session out of `IDLE` only when
// it receives `agent_event` broadcasts from the COM server. Those events
// originate from a small PowerShell bridge (`send-event.ps1`) that the
// CLI invokes through its hook system. If the user hasn't run a manual
// plugin-install step, the CLI never invokes the bridge, the registry
// stays empty, and the F2 list looks frozen.
//
// Each supported CLI loads hooks differently, so this module installs the
// bridge through the mechanism each CLI actually honors:
//
//   * Claude Code and Copilot CLI both expose a `plugin install` command
//     with marketplace-add support for local-path sources. We integrate
//     by **spawning the CLI itself** to register and install our plugin —
//     never by editing the CLI's settings/config files directly. Direct
//     edits would have to re-serialize JSONC files and would silently
//     strip header comments and any unknown user-managed fields.
//
//     Steps performed (per CLI):
//       1. Stage source files at
//          `%LOCALAPPDATA%\IntelligentTerminal\<cli>-plugin-src\wt-local\`
//          (a path *separate* from the CLI's install destination).
//       2. Spawn `<cli> plugin marketplace add <source-path>`.
//       3. Spawn `<cli> plugin install wt-agent-hooks@wt-local`.
//
//     All spawns are best-effort: failures (e.g. `<cli>.exe` not on
//     PATH, or "marketplace already added") are logged at warn/info and
//     never crash startup.
//
//     For Claude specifically: prior wta builds wrote a wta-tagged
//     `hooks` block directly into `~/.claude/settings.json`. We strip
//     that legacy block on every startup before invoking
//     `claude plugin install` so duplicate hook entries don't fire.
//
//   * Gemini CLI — written as a self-contained extension under
//     `~/.gemini/extensions/wt-agent-hooks/`. Gemini doesn't expose a
//     plugin-install equivalent that accepts local paths, so we keep
//     the on-disk extension layout for now.
//
// Each plugin folder bundles its own copy of the bridge script
// (`hooks/send-event.ps1`) so `${CLAUDE_PLUGIN_ROOT}` /
// `${extensionPath}` resolution stays inside the plugin layout.
//
// All writes are best-effort: failures are logged but do not block startup.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

/// Identifies a file inside the `wt-agent-hooks` bundle. Used by
/// [`bundle::read`] to locate either a loose on-disk copy or fall
/// back to the content embedded into `wta.exe` at build time.
#[derive(Clone, Copy, Debug)]
enum BundleFile {
    /// `agent-hooks-plugin/hooks/send-event.ps1`
    SendEventPs1,
    /// `gemini-extension/gemini-extension.json`
    GeminiExtensionJson,
    /// `gemini-extension/hooks/hooks.json`
    GeminiHooksJson,
}

impl BundleFile {
    fn rel_path(self) -> &'static str {
        match self {
            Self::SendEventPs1 => "agent-hooks-plugin/hooks/send-event.ps1",
            Self::GeminiExtensionJson => "gemini-extension/gemini-extension.json",
            Self::GeminiHooksJson => "gemini-extension/hooks/hooks.json",
        }
    }

    fn embedded(self) -> &'static str {
        match self {
            Self::SendEventPs1 => EMBEDDED_SEND_EVENT_PS1,
            Self::GeminiExtensionJson => EMBEDDED_GEMINI_EXTENSION_JSON,
            Self::GeminiHooksJson => EMBEDDED_GEMINI_HOOKS_JSON,
        }
    }
}

/// Embedded fallbacks. These compile-time blobs guarantee the installer
/// can always produce a working plugin even when no loose copy of the
/// `wt-agent-hooks/` directory exists next to `wta.exe`. The runtime
/// resolver in [`bundle::read`] prefers loose files when available.
const EMBEDDED_SEND_EVENT_PS1: &str =
    include_str!("../wt-agent-hooks/agent-hooks-plugin/hooks/send-event.ps1");
const EMBEDDED_GEMINI_EXTENSION_JSON: &str =
    include_str!("../wt-agent-hooks/gemini-extension/gemini-extension.json");
const EMBEDDED_GEMINI_HOOKS_JSON: &str =
    include_str!("../wt-agent-hooks/gemini-extension/hooks/hooks.json");

mod bundle {
    //! Runtime resolution of bundled hook files.
    //!
    //! At build time, `wta.exe` embeds copies of every file the
    //! installer needs (see `EMBEDDED_*` constants in the parent
    //! module). At runtime, [`read`] prefers a loose copy of the bundle
    //! so distributors / testers can patch the hooks without rebuilding
    //! `wta.exe`. Lookup chain (first hit wins, embedded is the final
    //! fallback):
    //!
    //!   1. `WTA_HOOKS_BUNDLE_DIR` env var ΓÇö absolute path to a
    //!      `wt-agent-hooks/`-shaped directory (highest priority).
    //!   2. `<dir-of-current-exe>/wt-agent-hooks/` ΓÇö where the MSIX /
    //!      installer is expected to deposit the loose bundle next to
    //!      `wta.exe`.
    //!   3. Walk parents of `current_exe()` looking for
    //!      `wta/wt-agent-hooks/` ΓÇö dev-tree fallback that mirrors the
    //!      walk in `_ResolveWtaExePath` (TerminalSettingsEditor).
    //!   4. Embedded `include_str!` blob ΓÇö ships with the binary.

    use super::BundleFile;
    use std::borrow::Cow;
    use std::path::PathBuf;

    /// Read the contents of a bundle file. Returns owned text when
    /// loaded from a loose on-disk copy, or a borrow of the embedded
    /// fallback otherwise.
    pub(super) fn read(file: BundleFile) -> Cow<'static, str> {
        read_with_roots(file, &candidate_roots())
    }

    /// Identify which root in the lookup chain (or the embedded
    /// fallback) supplied the bundle. Used by `wta hooks status` to
    /// surface the resolved source for support diagnosis.
    pub(super) fn resolve_source() -> super::BundleSourceInfo {
        let env = std::env::var_os("WTA_HOOKS_BUNDLE_DIR")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        if let Some(p) = &env {
            if p.join(BundleFile::SendEventPs1.rel_path()).is_file() {
                return super::BundleSourceInfo {
                    kind: "env",
                    path: Some(p.display().to_string()),
                };
            }
        }

        let exe = std::env::current_exe().ok();
        if let Some(exe_dir) = exe.as_ref().and_then(|p| p.parent()) {
            let sib = exe_dir.join("wt-agent-hooks");
            if sib.join(BundleFile::SendEventPs1.rel_path()).is_file() {
                return super::BundleSourceInfo {
                    kind: "exe-sibling",
                    path: Some(sib.display().to_string()),
                };
            }
        }

        if let Some(exe) = exe.as_ref() {
            let mut cursor = exe.parent().map(|p| p.to_path_buf());
            while let Some(dir) = cursor {
                let candidate = dir.join("wta").join("wt-agent-hooks");
                if candidate.is_dir() {
                    return super::BundleSourceInfo {
                        kind: "dev-tree",
                        path: Some(candidate.display().to_string()),
                    };
                }
                let parent = dir.parent().map(|p| p.to_path_buf());
                if parent.as_ref().map(|p| p == &dir).unwrap_or(true) {
                    break;
                }
                cursor = parent;
            }
        }

        super::BundleSourceInfo {
            kind: "embedded",
            path: None,
        }
    }

    /// Test seam: separate the file lookup from candidate-root
    /// computation so unit tests can inject a deterministic chain
    /// without mutating process-wide env state.
    pub(super) fn read_with_roots(file: BundleFile, roots: &[PathBuf]) -> Cow<'static, str> {
        if let Some(text) = read_loose(file, roots) {
            return Cow::Owned(text);
        }
        Cow::Borrowed(file.embedded())
    }

    fn read_loose(file: BundleFile, roots: &[PathBuf]) -> Option<String> {
        for root in roots {
            let path = root.join(file.rel_path());
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    tracing::debug!(
                        target: "agent_hooks",
                        path = %path.display(),
                        "loaded bundle file from loose copy",
                    );
                    return Some(text);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        target: "agent_hooks",
                        path = %path.display(),
                        err = %e,
                        "failed to read loose bundle file; falling through",
                    );
                }
            }
        }
        None
    }

    /// Resolve candidate roots fresh on every call. The installer only
    /// reads ~5 files per run, so the cost (a few `parent()` hops + an
    /// `is_dir` stat) is negligible. Computing per-call also keeps tests
    /// honest: a `OnceLock` cache caused races where one test populated
    /// the chain before another test could set `WTA_HOOKS_BUNDLE_DIR`.
    fn candidate_roots() -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(3);

        if let Some(env) = std::env::var_os("WTA_HOOKS_BUNDLE_DIR") {
            let p = PathBuf::from(env);
            if !p.as_os_str().is_empty() {
                out.push(p);
            }
        }

        let exe = std::env::current_exe().ok();
        if let Some(exe_dir) = exe.as_ref().and_then(|p| p.parent()) {
            out.push(exe_dir.join("wt-agent-hooks"));
        }

        if let Some(exe) = exe.as_ref() {
            let mut cursor = exe.parent().map(|p| p.to_path_buf());
            while let Some(dir) = cursor {
                let candidate = dir.join("wta").join("wt-agent-hooks");
                if candidate.is_dir() {
                    out.push(candidate);
                    break;
                }
                let parent = dir.parent().map(|p| p.to_path_buf());
                if parent.as_ref().map(|p| p == &dir).unwrap_or(true) {
                    break;
                }
                cursor = parent;
            }
        }

        out
    }
}

/// String used to tag every hook entry we manage so we can re-detect them
/// across runs and avoid duplicating entries on each wta launch.
const WTA_TAG: &str = "wt-agent-hooks";

/// Plugin name used in the Copilot plugin manifest and the
/// `enabledPlugins` map key. Must match `plugin.json` `name`.
const COPILOT_PLUGIN_NAME: &str = "wt-agent-hooks";

/// Marketplace identifier under which our plugin lives. Copilot CLI requires
/// marketplace names to be kebab-case (letters, numbers, hyphens — no
/// underscores). Used as:
///   * Folder name under `installed-plugins/<marketplace>/`.
///   * Key in `extraKnownMarketplaces` in settings.json.
///   * Suffix on `enabledPlugins` map keys (`<plugin>@<marketplace>`).
///
/// Older wta builds used `_direct` here, which Copilot CLI silently rejected
/// as a marketplace name (failing the kebab-case validator), causing the
/// plugin to never load even when the folder existed on disk.
const COPILOT_MARKETPLACE_NAME: &str = "wt-local";

/// Folder name under the marketplace folder that holds the plugin itself.
/// Copilot CLI's `plugin install` resolves the source path from
/// marketplace.json, then **copies** the plugin into a folder named after
/// the plugin's `name` field — so the canonical install destination is
/// `wt-local/<plugin-name>/`. We skip the source-folder copy step and
/// write the plugin directly to the canonical location, matching what
/// `copilot plugin list` validates against `installedPlugins[].cache_path`.
const COPILOT_PLUGIN_DIR_NAME: &str = COPILOT_PLUGIN_NAME;

/// Plugin version string written into `installedPlugins[].version`,
/// `plugin.json`, and `marketplace.json`. Bumped only when the wire format /
/// hook surface changes in a way users need to notice.
const COPILOT_PLUGIN_VERSION: &str = "0.1.0";

/// Embedded copy of the bridge script. **Loose copies (next to wta.exe
/// or under `WTA_HOOKS_BUNDLE_DIR`) take precedence** ΓÇö see
/// [`bundle::read`]. The `EMBEDDED_SEND_EVENT_PS1` constant declared
/// further up the file is the last-resort fallback baked into the
/// binary at build time from
/// `wta/wt-agent-hooks/agent-hooks-plugin/hooks/send-event.ps1`.

/// Folder name installed under `~/.gemini/extensions/` for Gemini CLI.
const GEMINI_EXTENSION_DIR_NAME: &str = "wt-agent-hooks";

/// Embedded copies of the Gemini extension files. Loose copies (next to
/// wta.exe or under `WTA_HOOKS_BUNDLE_DIR`) take precedence ΓÇö see
/// [`bundle::read`]. The `EMBEDDED_GEMINI_*` constants declared further
/// up the file are the last-resort fallback content sourced at build
/// time from `wta/wt-agent-hooks/gemini-extension/`.

/// Human-readable description used in both `plugin.json` and
/// `marketplace.json`. Kept short on purpose — Copilot CLI surfaces this
/// in `copilot plugin list` output.
const COPILOT_PLUGIN_DESCRIPTION: &str =
    "Forward CLI agent hook events to Windows Terminal for WTA display";

/// Hook event names → wta-side event-type identifier passed to the script.
/// Order mirrors `wta/wt-agent-hooks/agent-hooks-plugin/hooks/hooks.json` so the on-disk
/// behavior matches what a plugin install would have produced.
///
/// Only events Claude recognizes natively are listed here. Unknown event
/// names cause Claude to surface a "Quick safety check" warning at startup
/// asking the user how to handle the malformed settings.json — that's
/// hostile UX, so we keep this list strictly within Claude's documented
/// catalog (https://code.claude.com/docs/en/hooks). Copilot CLI accepts
/// the same set (a subset of the Claude format), so we reuse the table.
const HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart",      "agent.session.start"),
    ("SessionEnd",        "agent.session.end"),
    ("Notification",      "agent.notification"),
    ("UserPromptSubmit",  "agent.prompt.submit"),
    ("PreToolUse",        "agent.tool.starting"),
    ("PostToolUse",       "agent.tool.finished"),
    ("Stop",              "agent.stop"),
    ("SubagentStop",      "agent.subagent.stop"),
];

/// Top-level entry point. Run once at wta startup. Idempotent and silent on
/// failure: if a CLI isn't installed, we skip it; if its settings.json is
/// malformed, we leave it alone.
pub fn ensure_installed() {
    let Some(home) = home_dir() else {
        tracing::debug!(target: "agent_hooks", "no HOME/USERPROFILE; skipping");
        return;
    };
    ensure_installed_in(&home);
}

/// Run the installer against a specific home directory. Split out from
/// `ensure_installed` so tests can drive it with an isolated tempdir
/// without mutating `USERPROFILE`/`HOME` for the whole process.
fn ensure_installed_in(home: &Path) {
    install_for_claude(home);
    install_for_copilot(home);
    install_for_gemini(home);
}

/// Install the Gemini extension by writing the bundled
/// `wt-agent-hooks` extension into `~/.gemini/extensions/wt-agent-hooks/`.
///
/// Layout produced (matches `gemini extensions install <local-path>`):
///   ~/.gemini/extensions/wt-agent-hooks/
///     gemini-extension.json   # manifest (name + version + description)
///     hooks/
///       hooks.json            # event -> command mapping (uses
///                             # ${extensionPath} for the script path)
///       send-event.ps1        # embedded bridge script (same content as
///                             # the Claude/Copilot one — single source)
///
/// Idempotent: only writes when the on-disk content differs.
/// No-op when `~/.gemini/` is absent (Gemini CLI not installed).
/// Install the Gemini extension by spawning `gemini extensions install
/// <staging-path> --consent --skip-settings`.
///
/// Mirrors the claude/copilot pattern (#17): stage a valid extension
/// layout under `%LOCALAPPDATA%\IntelligentTerminal\gemini-plugin-src\`
/// then hand off to the CLI's own extension manager. This replaces the
/// pre-#17 direct file-write into `~/.gemini/extensions/wt-agent-hooks/`
/// — both produce a working extension, but the plugin-CLI flow gives
/// `wta hooks status` a single source of truth (`gemini extensions
/// list -o json`) instead of two divergent code paths.
///
/// `--consent --skip-settings` are required to defuse the security-
/// consent and config-on-install prompts. Without them, `gemini
/// extensions install` blocks on stdin and a background install (e.g.
/// from the Settings UI's "Install hooks" button) hangs the 60-second
/// timeout. Verified on Gemini 0.41.2 by manual probe.
///
/// Idempotency: `gemini extensions install` exits 1 with stderr
/// "Extension \"wt-agent-hooks\" is already installed. Please uninstall
/// it first." when the extension is already present. We match the
/// `already installed` substring to convert that to success.
fn install_for_gemini(home: &Path) {
    let gemini_dir = home.join(".gemini");
    if !gemini_dir.is_dir() {
        tracing::debug!(target: "gemini_hooks", "no ~/.gemini dir; Gemini CLI not present");
        return;
    }

    let Some(source_dir) = gemini_plugin_source_dir() else {
        tracing::warn!(
            target: "gemini_hooks",
            "could not resolve LOCALAPPDATA; skipping Gemini extension install",
        );
        return;
    };
    if let Err(e) = write_gemini_extension_files(&source_dir) {
        tracing::warn!(
            target: "gemini_hooks",
            err = %e,
            path = %source_dir.display(),
            "failed to stage Gemini extension source files",
        );
        return;
    }

    let source_path = source_dir.to_string_lossy().into_owned();
    if let Err(e) = run_plugin_cli(
        "gemini",
        &[
            "extensions",
            "install",
            &source_path,
            "--consent",
            "--skip-settings",
        ],
        "gemini_hooks",
        &["already installed"],
    ) {
        tracing::warn!(
            target: "gemini_hooks",
            err = %e,
            source = %source_path,
            "gemini extensions install failed",
        );
    }
}

/// Install hooks for Claude Code by spawning `claude plugin install`.
///
/// Always uses Claude Code's own plugin manager — never edits
/// `~/.claude/settings.json` directly. Letting Claude manage its own
/// settings preserves any unknown / user-managed fields the user may
/// have added.
///
/// Steps:
///   1. Strip any wta-tagged top-level `hooks` block left behind by
///      pre-plugin-install wta builds (so duplicate entries don't fire).
///   2. Stage marketplace + plugin source files under
///      `%LOCALAPPDATA%\IntelligentTerminal\claude-plugin-src\wt-local\`.
///   3. Spawn `claude plugin marketplace add <source-path>`.
///   4. Spawn `claude plugin install wt-agent-hooks@wt-local`.
///
/// Idempotent: rewriting source files is a no-op when content matches;
/// the spawned commands are expected to be idempotent on Claude's side.
/// Failures (CLI not on PATH, "marketplace already added", etc.) are
/// logged but never fatal.
fn install_for_claude(home: &Path) {
    let claude_dir = home.join(".claude");
    if !claude_dir.is_dir() {
        tracing::debug!(target: "agent_hooks", "no ~/.claude dir; Claude not present");
        return;
    }

    // Round-8 cleanup: prior wta builds merged a tagged `hooks` block
    // directly into ~/.claude/settings.json. Now that we register the
    // plugin via `claude plugin install`, leaving that block in place
    // would fire each event twice — once from settings.json and once
    // from the plugin. Strip our entries on every startup.
    let settings_path = claude_dir.join("settings.json");
    if let Err(e) = cleanup_legacy_claude_hooks(&settings_path) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %settings_path.display(),
            "failed to strip legacy wta hooks from settings.json; non-fatal",
        );
    }

    let source_marketplace_dir = match claude_plugin_source_dir() {
        Some(p) => p,
        None => {
            tracing::warn!(
                target: "agent_hooks",
                "could not resolve LOCALAPPDATA; skipping Claude plugin install",
            );
            return;
        }
    };
    let source_plugin_dir = source_marketplace_dir.join(COPILOT_PLUGIN_DIR_NAME);

    if let Err(e) = write_marketplace_files(&source_marketplace_dir) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %source_marketplace_dir.display(),
            "failed to stage Claude marketplace source files",
        );
        return;
    }
    if let Err(e) = write_plugin_files(&source_plugin_dir, "claude") {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %source_plugin_dir.display(),
            "failed to stage Claude plugin source files",
        );
        return;
    }

    // Hand off to Claude CLI for the actual registration + install.
    // Claude's marketplace add and plugin install are already idempotent
    // (exit 0 with "already on disk" / "already installed" messages),
    // so no idempotency_substrings are needed.
    let source_path = source_marketplace_dir.to_string_lossy().into_owned();
    if let Err(e) = run_plugin_cli(
        "claude",
        &["plugin", "marketplace", "add", &source_path],
        "agent_hooks",
        &[],
    ) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            "claude plugin marketplace add failed; aborting plugin install",
        );
        return;
    }

    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    if let Err(e) = run_plugin_cli(
        "claude",
        &["plugin", "install", &plugin_ref],
        "agent_hooks",
        &[],
    ) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            plugin = %plugin_ref,
            "claude plugin install failed",
        );
    }
}

/// Install hooks for Copilot CLI by spawning `copilot plugin install`.
///
/// Always uses Copilot CLI's own plugin manager — never edits
/// `~/.copilot/settings.json` or `~/.copilot/config.json` directly.
/// Letting Copilot manage its own files preserves JSONC comments,
/// formatting, and any unknown fields the user may have added.
///
/// Steps:
///   1. Stage marketplace + plugin source files under
///      `%LOCALAPPDATA%\IntelligentTerminal\copilot-plugin-src\wt-local\`
///      (a path *separate* from the install destination).
///   2. Spawn `copilot plugin marketplace add <source-path>`.
///   3. Spawn `copilot plugin install wt-agent-hooks@wt-local`.
///
/// Idempotent: rewriting source files is a no-op when content matches;
/// the spawned commands are expected to be idempotent on Copilot CLI's
/// side. Failures (CLI not on PATH, "marketplace already added", etc.)
/// are logged but never fatal.
fn install_for_copilot(home: &Path) {
    let copilot_dir = home.join(".copilot");
    if !copilot_dir.is_dir() {
        tracing::debug!(target: "copilot_hooks", "no ~/.copilot dir; Copilot CLI not present");
        return;
    }

    // Source dir: where we *stage* the plugin layout that `copilot plugin
    // install` reads from. MUST be different from the install destination
    // (`~/.copilot/installed-plugins/wt-local/`) — Copilot copies source
    // → destination, and overlapping the two trips Copilot's loader.
    let source_marketplace_dir = match copilot_plugin_source_dir() {
        Some(p) => p,
        None => {
            tracing::warn!(
                target: "copilot_hooks",
                "could not resolve LOCALAPPDATA; skipping Copilot plugin install",
            );
            return;
        }
    };
    let source_plugin_dir = source_marketplace_dir.join(COPILOT_PLUGIN_DIR_NAME);

    if let Err(e) = write_marketplace_files(&source_marketplace_dir) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            path = %source_marketplace_dir.display(),
            "failed to stage marketplace source files",
        );
        return;
    }
    if let Err(e) = write_plugin_files(&source_plugin_dir, "copilot") {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            path = %source_plugin_dir.display(),
            "failed to stage plugin source files",
        );
        return;
    }

    // Hand off to Copilot CLI for the actual registration + install.
    // copilot plugin marketplace add returns exit 1 when the marketplace
    // is already registered — verified by manual probe with stderr
    // "Failed to add marketplace: Marketplace \"wt-local\" already
    // registered". Match that substring to keep the install idempotent.
    // copilot plugin install is already exit-0 idempotent.
    let source_path = source_marketplace_dir.to_string_lossy().into_owned();
    if let Err(e) = run_plugin_cli(
        "copilot",
        &["plugin", "marketplace", "add", &source_path],
        "copilot_hooks",
        &["already registered"],
    ) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            "copilot plugin marketplace add failed; aborting plugin install",
        );
        return;
    }

    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    if let Err(e) = run_plugin_cli(
        "copilot",
        &["plugin", "install", &plugin_ref],
        "copilot_hooks",
        &[],
    ) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            plugin = %plugin_ref,
            "copilot plugin install failed",
        );
        return;
    }

    // Round-7 cleanup: a previous wta wrote files to `_direct/` (which
    // Copilot rejected as an invalid marketplace name). Remove the stale
    // folder so users don't see two copies of the plugin on disk.
    let stale = copilot_dir.join("installed-plugins").join("_direct");
    if stale.is_dir() {
        if let Err(e) = fs::remove_dir_all(&stale) {
            tracing::warn!(
                target: "copilot_hooks",
                err = %e,
                path = %stale.display(),
                "failed to remove stale _direct folder; non-fatal",
            );
        } else {
            tracing::info!(
                target: "copilot_hooks",
                path = %stale.display(),
                "removed stale _direct plugin folder",
            );
        }
    }
}

/// Resolve the staging directory passed as the `<source>` argument of
/// `copilot plugin marketplace add`. Persistent across runs so the
/// marketplace path Copilot stores in its settings.json doesn't churn.
///
/// Layout produced (matches what `copilot plugin marketplace add` expects):
///
///   %LOCALAPPDATA%\IntelligentTerminal\copilot-plugin-src\wt-local\
///     .claude-plugin\marketplace.json
///     wt-agent-hooks\
///       .claude-plugin\plugin.json
///       hooks\hooks.json
///       hooks\send-event.ps1
fn copilot_plugin_source_dir() -> Option<PathBuf> {
    let root = crate::runtime_paths::intelligent_terminal_root()?;
    Some(root.join("copilot-plugin-src").join(COPILOT_MARKETPLACE_NAME))
}

/// Resolve the staging directory passed as the `<source>` argument of
/// `claude plugin marketplace add`. Mirrors `copilot_plugin_source_dir`
/// but lives under `claude-plugin-src/` so the two CLIs don't collide.
fn claude_plugin_source_dir() -> Option<PathBuf> {
    let root = crate::runtime_paths::intelligent_terminal_root()?;
    Some(root.join("claude-plugin-src").join(COPILOT_MARKETPLACE_NAME))
}

/// Resolve the staging directory passed as the `<source>` argument of
/// `gemini extensions install`. Lives under `gemini-plugin-src/` so it
/// doesn't collide with the claude/copilot staging dirs.
///
/// Note: Gemini's extension layout is **not** marketplace-shaped. The
/// staging dir holds the extension directly (`gemini-extension.json` +
/// `hooks/`), not a marketplace wrapper. The `wt-agent-hooks` folder
/// name matches `GEMINI_EXTENSION_DIR_NAME` so the path is symmetrical
/// with the `wt-local` marketplace folder used by claude/copilot.
fn gemini_plugin_source_dir() -> Option<PathBuf> {
    let root = crate::runtime_paths::intelligent_terminal_root()?;
    Some(root.join("gemini-plugin-src").join(GEMINI_EXTENSION_DIR_NAME))
}

/// Path to the Gemini extension directory we install / inspect / remove.
fn gemini_extension_dir(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("extensions")
        .join(GEMINI_EXTENSION_DIR_NAME)
}

/// Spawn `<exe>` with the given args, capture stdout/stderr for the
/// trace log, and return Err on spawn failure or non-zero exit.
///
/// `idempotency_substrings` is a case-insensitive substring set that
/// classifies non-zero exits as success when the captured stderr/stdout
/// contains any entry. This is how we tolerate the "already registered"
/// / "already installed" responses that some CLIs emit with a non-zero
/// exit code (verified by manual probe — see PR description for raw
/// captured output):
///   * `copilot plugin marketplace add` → exit 1, stderr `already registered`
///   * `gemini extensions install`      → exit 1, stderr `already installed`
///
/// Most-likely failure modes:
///   * `NotFound` — `<exe>` isn't on PATH (after `which::which` resolution
///     for `.cmd` shims). Caller skips remaining steps; the legacy log
///     line stays at `warn!` from `run_plugin_cli_capture` since callers
///     of this wrapper are install paths that will skip the rest of the
///     flow anyway.
///   * Non-zero exit not matching `idempotency_substrings` — logged at
///     `warn!` by `run_plugin_cli_capture`. Caller skips remaining
///     steps; next wta startup retries.
///
/// On Windows the child is launched with `CREATE_NO_WINDOW` so it
/// doesn't briefly pop a console when wta is itself running headless
/// (e.g. invoked from the Settings UI's "Install hooks" button via
/// `wta install-hooks`).
fn run_plugin_cli(
    exe: &str,
    args: &[&str],
    _log_target: &str,
    idempotency_substrings: &[&str],
) -> std::io::Result<()> {
    let outcome = run_plugin_cli_capture(exe, args)?;
    if outcome.success {
        return Ok(());
    }
    if matches_idempotency_substring(&outcome.stdout, &outcome.stderr, idempotency_substrings) {
        tracing::info!(
            target: "agent_hooks",
            exe = exe,
            args = ?args,
            stdout = %outcome.stdout.trim(),
            stderr = %outcome.stderr.trim(),
            status = ?outcome.status_code,
            "plugin CLI returned non-zero exit but output indicates already-installed; treating as success",
        );
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!(
            "{} {} exited {}",
            exe,
            args.join(" "),
            outcome
                .status_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
    ))
}

/// Pure helper: return true if any substring in `idempotency_substrings`
/// appears in either captured stream (case-insensitive). Substrings are
/// expected to already be lowercase; we only lowercase the haystacks.
fn matches_idempotency_substring(stdout: &str, stderr: &str, needles: &[&str]) -> bool {
    if needles.is_empty() {
        return false;
    }
    let stdout_lc = stdout.to_lowercase();
    let stderr_lc = stderr.to_lowercase();
    needles
        .iter()
        .any(|n| stdout_lc.contains(n) || stderr_lc.contains(n))
}

/// Outcome of spawning a CLI, with stdout/stderr captured for callers
/// that need to parse the output (`wta hooks status`).
#[derive(Debug, Clone)]
struct CliRunOutcome {
    success: bool,
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Spawn `<exe>` with the given args, capture stdout/stderr, and trace
/// the result. Never returns Err on non-zero exit — callers inspect
/// `outcome.success` themselves so they can keep parsing partial
/// output (e.g. a `plugin list` that prints rows then warns at the
/// end). Only returns Err when the process couldn't be spawned at all
/// (e.g. CLI not on PATH).
///
/// On Windows, `Command::new("foo")` does **not** consult `PATHEXT`,
/// so `.cmd` / `.bat` shims (which is how every Node-based CLI ships
/// here — `copilot.cmd`, `gemini.cmd`) won't be found by name. We
/// resolve through `which::which` first to get the full path
/// (including the extension) and spawn that.
fn run_plugin_cli_capture(exe: &str, args: &[&str]) -> std::io::Result<CliRunOutcome> {
    use std::process::Stdio;
    let resolved = which::which(exe).ok();
    let mut cmd = match &resolved {
        Some(p) => std::process::Command::new(p),
        None => std::process::Command::new(exe),
    };
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        tracing::warn!(
            target: "agent_hooks",
            exe = exe,
            args = ?args,
            stdout = %stdout.trim(),
            stderr = %stderr.trim(),
            status = ?output.status.code(),
            "plugin CLI returned non-zero exit",
        );
    } else {
        tracing::info!(
            target: "agent_hooks",
            exe = exe,
            args = ?args,
            stdout = %stdout.trim(),
            "plugin CLI succeeded",
        );
    }
    Ok(CliRunOutcome {
        success: output.status.success(),
        status_code: output.status.code(),
        stdout,
        stderr,
    })
}

/// Return the discovered home directory. Mirrors `history_loader::home_dir`
/// so behavior is consistent between the two modules.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Copilot plugin install — separate code path because Copilot CLI ignores
// the top-level `hooks` block and only loads hooks declared by registered
// plugins.
// ---------------------------------------------------------------------------

/// Write the marketplace catalog files (`marketplace.json`) into
/// `installed-plugins/wt-local/.claude-plugin/`. Copilot CLI's plugin
/// manager scans `extraKnownMarketplaces` and reads each
/// `<marketplace>/.claude-plugin/marketplace.json` to discover plugins.
fn write_marketplace_files(marketplace_dir: &Path) -> std::io::Result<()> {
    let claude_plugin_dir = marketplace_dir.join(".claude-plugin");
    fs::create_dir_all(&claude_plugin_dir)?;

    let marketplace_json = serde_json::to_string_pretty(&marketplace_json_value())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(
        &claude_plugin_dir.join("marketplace.json"),
        &marketplace_json,
    )?;
    Ok(())
}

/// Build the `marketplace.json` document the plugin manager reads.
/// The `source: "./<plugin-folder>"` is resolved relative to the
/// marketplace folder when the CLI loads it. Identical content for
/// Claude and Copilot — both honor the `.claude-plugin` convention.
fn marketplace_json_value() -> Value {
    json!({
        "name":        COPILOT_MARKETPLACE_NAME,
        "description": "Local marketplace populated by wta",
        "owner":       { "name": "Agentic Terminal" },
        "plugins": [
            {
                "name":        COPILOT_PLUGIN_NAME,
                "description": COPILOT_PLUGIN_DESCRIPTION,
                "version":     COPILOT_PLUGIN_VERSION,
                "source":      format!("./{}", COPILOT_PLUGIN_DIR_NAME),
            }
        ],
    })
}

/// Write the plugin files (`.claude-plugin/plugin.json`,
/// `hooks/hooks.json`, `hooks/send-event.ps1`) into the plugin folder.
/// Idempotent: each file is only rewritten when its on-disk content
/// differs from what we'd produce.
///
/// **Manifest path** is `.claude-plugin/plugin.json`, NOT `plugin.json`
/// at the plugin root. Copilot's loader silently ignores a root-level
/// manifest (matching the `superpowers` plugin convention). Earlier wta
/// builds wrote to the root and the plugin never loaded.
fn write_plugin_files(plugin_dir: &Path, cli_source: &str) -> std::io::Result<()> {
    let claude_plugin_subdir = plugin_dir.join(".claude-plugin");
    let hooks_subdir = plugin_dir.join("hooks");
    fs::create_dir_all(&claude_plugin_subdir)?;
    fs::create_dir_all(&hooks_subdir)?;

    let plugin_json = serde_json::to_string_pretty(&plugin_json_value())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(&claude_plugin_subdir.join("plugin.json"), &plugin_json)?;
    let send_event_text = bundle::read(BundleFile::SendEventPs1);
    write_if_changed(&hooks_subdir.join("send-event.ps1"), &send_event_text)?;

    // Generate hooks.json from `HOOK_EVENTS`. Use `${CLAUDE_PLUGIN_ROOT}`
    // resolution so the plugin keeps working if the user moves their
    // CLI home dir (both Claude and Copilot substitute the plugin's own
    // folder for that variable). The `cli_source` flag is what the
    // bridge script keys off to tag emitted events with the right CLI.
    let hooks_json = serde_json::to_string_pretty(&plugin_hooks_json_value(cli_source))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(&hooks_subdir.join("hooks.json"), &hooks_json)?;

    // Pre-round-7 wta wrote a root-level `plugin.json` that Copilot
    // ignored. Remove it so users don't see two copies of the manifest.
    let stale_root_manifest = plugin_dir.join("plugin.json");
    if stale_root_manifest.is_file() {
        if let Err(e) = fs::remove_file(&stale_root_manifest) {
            tracing::warn!(
                target: "copilot_hooks",
                err = %e,
                "failed to remove stale root plugin.json; non-fatal",
            );
        }
    }

    Ok(())
}

/// Write the Gemini extension files into the staging dir
/// (`gemini-extension.json` at the root + `hooks/hooks.json` +
/// `hooks/send-event.ps1`). Idempotent via `write_if_changed`.
///
/// Layout produced (matches what `gemini extensions install <local-path>`
/// expects):
///
///   %LOCALAPPDATA%\IntelligentTerminal\gemini-plugin-src\wt-agent-hooks\
///     gemini-extension.json
///     hooks\hooks.json
///     hooks\send-event.ps1
fn write_gemini_extension_files(extension_dir: &Path) -> std::io::Result<()> {
    let hooks_subdir = extension_dir.join("hooks");
    fs::create_dir_all(&hooks_subdir)?;

    let manifest_text = bundle::read(BundleFile::GeminiExtensionJson);
    write_if_changed(&extension_dir.join("gemini-extension.json"), &manifest_text)?;

    let hooks_text = bundle::read(BundleFile::GeminiHooksJson);
    write_if_changed(&hooks_subdir.join("hooks.json"), &hooks_text)?;

    let script_text = bundle::read(BundleFile::SendEventPs1);
    write_if_changed(&hooks_subdir.join("send-event.ps1"), &script_text)?;

    Ok(())
}

/// Build the `plugin.json` manifest written into
/// `<plugin-root>/.claude-plugin/plugin.json`.
///
/// Deliberately omits a `hooks` field — Copilot's loader auto-discovers
/// `<plugin-root>/hooks/hooks.json` by convention (matches the
/// `superpowers` plugin), and the embedded reference manifest's `"hooks":
/// "hooks/hooks.json"` field has caused at least one reported parse warning
/// in the wild.
fn plugin_json_value() -> Value {
    json!({
        "name":        COPILOT_PLUGIN_NAME,
        "description": COPILOT_PLUGIN_DESCRIPTION,
        "version":     COPILOT_PLUGIN_VERSION,
        "author":      { "name": "Agentic Terminal" },
        "license":     "MIT",
        "keywords":    ["windows-terminal", "agent-hooks", "wta"],
    })
}

/// Build the `hooks.json` document the plugin loader will read.
/// Generated programmatically from `HOOK_EVENTS` so we don't ship stale
/// event names. `cli_source` (e.g. `"copilot"`, `"claude"`) is forwarded
/// to the bridge script via `-CliSource <name>` so emitted events are
/// tagged with the originating CLI.
fn plugin_hooks_json_value(cli_source: &str) -> Value {
    let mut hooks_map = serde_json::Map::new();
    for (event_name, event_id) in HOOK_EVENTS {
        hooks_map.insert(
            (*event_name).to_string(),
            json!([{
                "matcher": ".*",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "powershell -ExecutionPolicy Bypass -File \"${{CLAUDE_PLUGIN_ROOT}}/hooks/send-event.ps1\" -CliSource {} {}",
                        cli_source, event_id,
                    ),
                }]
            }]),
        );
    }
    json!({ "hooks": Value::Object(hooks_map) })
}

/// Write `contents` to `path` only when the on-disk content differs. Skips
/// the write when unchanged so repeated startups don't churn mtimes.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    let needs_write = match fs::read_to_string(path) {
        Ok(existing) => existing != contents,
        Err(_) => true,
    };
    if needs_write {
        fs::write(path, contents)?;
        tracing::info!(
            target: "copilot_hooks",
            path = %path.display(),
            "wrote plugin file",
        );
    }
    Ok(())
}


/// Strip wta-tagged entries from the top-level `hooks` block of
/// `~/.claude/settings.json` (Round-8 cleanup). Pre-plugin-install wta
/// builds wrote our hook entries directly into settings.json; once the
/// plugin is installed via `claude plugin install`, leaving those
/// entries in place would fire each event twice. Idempotent: no-op if
/// there's nothing to clean.
fn cleanup_legacy_claude_hooks(settings_path: &Path) -> std::io::Result<()> {
    let text = match fs::read_to_string(settings_path) {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    let mut settings: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "agent_hooks",
                err = %e,
                path = %settings_path.display(),
                "settings.json malformed; leaving untouched",
            );
            return Ok(());
        }
    };

    let Some(root) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(());
    };
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Ok(());
    };

    let mut changed = false;
    let event_names: Vec<String> = hooks_obj.keys().cloned().collect();
    for event_name in event_names {
        let Some(arr) = hooks_obj.get_mut(&event_name).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let before = arr.len();
        arr.retain(|entry| !entry_is_wta_tagged(entry));
        if arr.len() != before {
            changed = true;
        }
        if arr.is_empty() {
            hooks_obj.remove(&event_name);
        }
    }

    // If the hooks object is now empty, remove it entirely so we don't
    // leave behind a `"hooks": {}` artifact in the user's settings.
    if hooks_obj.is_empty() {
        root.remove("hooks");
        changed = true;
    }

    if !changed {
        return Ok(());
    }

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(settings_path, serialized)?;
    tracing::info!(
        target: "agent_hooks",
        path = %settings_path.display(),
        "stripped legacy wta hooks block",
    );
    Ok(())
}

/// True iff the entry was inserted by us (any nested `command` string
/// references our bridge script or carries the WTA_TAG marker). Used by
/// `cleanup_legacy_claude_hooks` to identify our own entries during
/// migration off the direct-settings.json path.
fn entry_is_wta_tagged(entry: &Value) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
        return false;
    };
    for h in hooks {
        let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else { continue; };
        if cmd.contains(WTA_TAG) || cmd.contains("send-event.ps1") {
            return true;
        }
    }
    false
}


// ---------------------------------------------------------------------------
// Track 2: `wta hooks status` / `wta hooks uninstall`
//
// Public, side-effect-free inspection (`status`) plus best-effort teardown
// (`uninstall`) so the Settings UI and `Verify-AgentHooks.ps1` (Track 3)
// can rely on a single source of truth instead of re-implementing
// detection in C++/PowerShell.
// ---------------------------------------------------------------------------

/// One of the supported agent CLIs. Used as both a routing key (which
/// per-CLI helper to invoke) and as the `name` field in the JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliKind {
    Copilot,
    Claude,
    Gemini,
}

impl CliKind {
    pub const ALL: &'static [CliKind] = &[CliKind::Copilot, CliKind::Claude, CliKind::Gemini];

    pub fn name(self) -> &'static str {
        match self {
            Self::Copilot => "copilot",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "copilot" => Some(Self::Copilot),
            "claude" => Some(Self::Claude),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

/// Filter for `wta hooks uninstall --cli=...`.
#[derive(Debug, Clone, Copy)]
pub enum CliScope {
    All,
    One(CliKind),
}

impl CliScope {
    fn includes(self, k: CliKind) -> bool {
        match self {
            Self::All => true,
            Self::One(x) => x == k,
        }
    }
}

/// Per-CLI install state surfaced by `wta hooks status`.
///
/// `binary_on_path`/`binary_path` say whether the CLI itself is
/// installed. The remaining flags describe whether *our* plugin is
/// registered with that CLI. `detection_fallback` is set to `Some("fs")`
/// when the CLI command failed to spawn or returned unparseable output
/// and we used filesystem heuristics instead.
#[derive(Debug, Clone, Serialize)]
pub struct CliStatus {
    pub name: &'static str,
    pub binary_on_path: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
    pub marketplace_registered: bool,
    pub plugin_installed: bool,
    pub plugin_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection_fallback: Option<&'static str>,
}

/// Top-level shape of `wta hooks status --json`. `bundle_source`
/// reports which entry in the bundle lookup chain (env override / exe
/// sibling / dev tree / embedded) supplied the hook files for the
/// running `wta` process — useful when debugging "why is this machine
/// running an old `send-event.ps1`?" support tickets.
#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub clis: Vec<CliStatus>,
    pub bundle_source: BundleSourceInfo,
}

/// Resolved location of the `wt-agent-hooks/` bundle the running `wta`
/// is using. `kind` is one of `"env" | "exe-sibling" | "dev-tree" |
/// "embedded"` (mirrors `bundle::resolve_source`).
#[derive(Debug, Clone, Serialize)]
pub struct BundleSourceInfo {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Per-CLI outcome of `wta hooks uninstall`. Each of the optional
/// booleans is `Some(true)` when the matching CLI command succeeded,
/// `Some(false)` when it ran but failed, and `None` when we skipped
/// it (e.g. CLI not on PATH so we can't invoke `<cli> plugin
/// uninstall`).
#[derive(Debug, Clone, Serialize)]
pub struct CliUninstallResult {
    pub name: &'static str,
    pub attempted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_uninstalled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace_removed: Option<bool>,
    pub staging_dir_removed: bool,
    pub messages: Vec<String>,
}

/// Top-level shape of `wta hooks uninstall --json`.
#[derive(Debug, Clone, Serialize)]
pub struct UninstallReport {
    pub schema_version: u32,
    pub clis: Vec<CliUninstallResult>,
}

const STATUS_SCHEMA_VERSION: u32 = 1;
const UNINSTALL_SCHEMA_VERSION: u32 = 1;

// ---- public entry points ---------------------------------------------------

/// Build a `StatusReport` describing the current install state for
/// every supported CLI under the given home directory. Side-effect
/// free: spawns CLIs in read-only mode and stats files; never writes.
pub fn status() -> StatusReport {
    let home = home_dir();
    StatusReport {
        schema_version: STATUS_SCHEMA_VERSION,
        clis: CliKind::ALL
            .iter()
            .map(|k| status_for(*k, home.as_deref()))
            .collect(),
        bundle_source: bundle::resolve_source(),
    }
}

/// Run uninstall against `scope`. Best-effort: every step is logged but
/// failures never abort the run. CLIs not on PATH are recorded with
/// `attempted: false` and a message; the staging dir is still removed
/// where present so we don't leave behind orphan files.
pub fn uninstall(scope: CliScope) -> UninstallReport {
    let home = home_dir();
    UninstallReport {
        schema_version: UNINSTALL_SCHEMA_VERSION,
        clis: CliKind::ALL
            .iter()
            .copied()
            .filter(|k| scope.includes(*k))
            .map(|k| uninstall_for(k, home.as_deref()))
            .collect(),
    }
}

// ---- status detection ------------------------------------------------------

fn status_for(cli: CliKind, home: Option<&Path>) -> CliStatus {
    let (on_path, bin_path) = locate_binary(cli);
    match cli {
        CliKind::Copilot => copilot_status(on_path, bin_path, home),
        CliKind::Claude => claude_status(on_path, bin_path, home),
        CliKind::Gemini => gemini_status(on_path, bin_path, home),
    }
}

fn locate_binary(cli: CliKind) -> (bool, Option<String>) {
    match which::which(cli.name()) {
        Ok(p) => (true, Some(p.display().to_string())),
        Err(_) => (false, None),
    }
}

fn copilot_status(on_path: bool, bin_path: Option<String>, home: Option<&Path>) -> CliStatus {
    let mut out = CliStatus {
        name: CliKind::Copilot.name(),
        binary_on_path: on_path,
        binary_path: bin_path,
        marketplace_registered: false,
        plugin_installed: false,
        plugin_enabled: false,
        detection_fallback: None,
    };
    if !on_path {
        // CLI not present — fall back to fs check so we still report
        // install state from a prior run.
        copilot_fs_fallback(&mut out, home);
        return out;
    }

    // 1. plugin list (text — Copilot 1.0.44-2 has no --json).
    let plugin_ok = match run_plugin_cli_capture("copilot", &["plugin", "list"]) {
        Ok(o) if o.success => Some(parse_copilot_plugin_list(&o.stdout)),
        Ok(_) | Err(_) => None,
    };
    // 2. marketplace list (text).
    let mkt_ok = match run_plugin_cli_capture("copilot", &["plugin", "marketplace", "list"]) {
        Ok(o) if o.success => Some(parse_copilot_marketplace_list(&o.stdout)),
        Ok(_) | Err(_) => None,
    };

    if let (Some(p), Some(m)) = (plugin_ok, mkt_ok) {
        out.plugin_installed = p;
        // Copilot's `plugin list` doesn't expose enabled/disabled, so
        // "listed" implies enabled. Disabling a plugin removes it.
        out.plugin_enabled = p;
        out.marketplace_registered = m;
    } else {
        copilot_fs_fallback(&mut out, home);
    }

    out
}

fn copilot_fs_fallback(out: &mut CliStatus, home: Option<&Path>) {
    out.detection_fallback = Some("fs");
    let Some(home) = home else { return };

    // Source of truth is `~/.copilot/config.json`. The
    // `installed-plugins/<marketplace>/<plugin>/` directory may exist
    // empty (Copilot lazy-populates the cache_path), so a pure
    // file-existence check there gives false negatives. Parse the
    // JSON (Copilot writes JSONC with leading `//` banner comments —
    // strip those before handing to serde_json) and look for our
    // entry.
    let config_path = home.join(".copilot").join("config.json");
    if let Ok(text) = fs::read_to_string(&config_path) {
        let stripped = strip_jsonc_line_comments(&text);
        if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
            if let Some(present) = copilot_config_lookup(&v) {
                out.plugin_installed = present.installed;
                out.plugin_enabled = present.enabled;
                out.marketplace_registered = present.marketplace_registered;
                return;
            }
        }
    }

    // Last-resort heuristic for very old layouts: just check the
    // marketplace folder exists. Not as accurate as the JSON path,
    // but better than reporting a clean "not installed" when the
    // config file is unreadable.
    let marketplace_dir = home
        .join(".copilot")
        .join("installed-plugins")
        .join(COPILOT_MARKETPLACE_NAME);
    let any = marketplace_dir.is_dir();
    out.plugin_installed = any;
    out.plugin_enabled = any;
    out.marketplace_registered = any;
}

/// Inspect `~/.copilot/config.json` for our plugin / marketplace.
///
/// Real shape (Copilot CLI 1.0.44-2):
/// ```jsonc
/// {
///   "installedPlugins": [
///     { "name": "wt-agent-hooks", "marketplace": "wt-local",
///       "version": "0.1.0", "enabled": true,
///       "cache_path": "..." }
///   ],
///   "extraKnownMarketplaces": { "wt-local": { ... } }
/// }
/// ```
///
/// `extraKnownMarketplaces` may be an object keyed by marketplace name
/// or an array — accept either shape so we don't fall over on a future
/// schema change.
fn copilot_config_lookup(v: &Value) -> Option<CopilotConfigState> {
    let plugin = v
        .get("installedPlugins")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .find(|e| {
            e.get("name").and_then(|n| n.as_str()) == Some(COPILOT_PLUGIN_NAME)
                && e.get("marketplace").and_then(|n| n.as_str())
                    == Some(COPILOT_MARKETPLACE_NAME)
        });

    let marketplace_registered = match v.get("extraKnownMarketplaces") {
        Some(Value::Object(map)) => map.contains_key(COPILOT_MARKETPLACE_NAME),
        Some(Value::Array(arr)) => arr.iter().any(|e| {
            e.get("name").and_then(|n| n.as_str()) == Some(COPILOT_MARKETPLACE_NAME)
        }),
        _ => false,
    };

    Some(CopilotConfigState {
        installed: plugin.is_some() || marketplace_registered,
        enabled: plugin
            .and_then(|p| p.get("enabled"))
            .and_then(|x| x.as_bool())
            .unwrap_or(plugin.is_some()),
        marketplace_registered: marketplace_registered || plugin.is_some(),
    })
}

#[derive(Debug, Clone, Copy)]
struct CopilotConfigState {
    installed: bool,
    enabled: bool,
    marketplace_registered: bool,
}

/// Strip `//` line comments outside of strings. Copilot CLI's
/// `config.json` is JSONC — it carries a "// User settings belong in
/// settings.json." banner that strict serde_json refuses. This is the
/// minimum normalization needed; we don't try to handle `/* ... */`
/// block comments because Copilot doesn't emit them.
///
/// Tracks an in-string flag so a `//` literal inside a JSON string
/// (e.g. a `"https://..."` URL) isn't accidentally treated as the
/// start of a comment.
fn strip_jsonc_line_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            // Skip until newline.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn claude_status(on_path: bool, bin_path: Option<String>, home: Option<&Path>) -> CliStatus {
    let mut out = CliStatus {
        name: CliKind::Claude.name(),
        binary_on_path: on_path,
        binary_path: bin_path,
        marketplace_registered: false,
        plugin_installed: false,
        plugin_enabled: false,
        detection_fallback: None,
    };
    if !on_path {
        claude_fs_fallback(&mut out, home);
        return out;
    }

    let plugin_json = match run_plugin_cli_capture("claude", &["plugin", "list", "--json"]) {
        Ok(o) if o.success => parse_claude_plugin_list_json(&o.stdout),
        Ok(_) | Err(_) => None,
    };
    let mkt_json =
        match run_plugin_cli_capture("claude", &["plugin", "marketplace", "list", "--json"]) {
            Ok(o) if o.success => parse_claude_marketplace_list_json(&o.stdout),
            Ok(_) | Err(_) => None,
        };

    if let (Some(p), Some(m)) = (plugin_json, mkt_json) {
        out.plugin_installed = p.installed;
        out.plugin_enabled = p.enabled;
        out.marketplace_registered = m;
    } else {
        claude_fs_fallback(&mut out, home);
    }
    out
}

fn claude_fs_fallback(out: &mut CliStatus, home: Option<&Path>) {
    out.detection_fallback = Some("fs");
    let Some(home) = home else { return };
    // Mirrors AIAgentsViewModel.cpp _IsClaudeHookInstalled: marketplace
    // entry recorded by Claude AND staged source files still on disk.
    let known_path = home
        .join(".claude")
        .join("plugins")
        .join("known_marketplaces.json");
    let marketplace_known = fs::read_to_string(&known_path)
        .map(|t| t.contains("\"wt-local\""))
        .unwrap_or(false);
    let staged_manifest_exists = claude_plugin_source_dir()
        .map(|p| p.join(".claude-plugin").join("marketplace.json").is_file())
        .unwrap_or(false);
    let installed = marketplace_known && staged_manifest_exists;
    out.plugin_installed = installed;
    out.plugin_enabled = installed;
    out.marketplace_registered = marketplace_known;
}

fn gemini_status(on_path: bool, bin_path: Option<String>, home: Option<&Path>) -> CliStatus {
    let mut out = CliStatus {
        name: CliKind::Gemini.name(),
        binary_on_path: on_path,
        binary_path: bin_path,
        // Gemini has no marketplace concept — extensions install from
        // path/git directly. Report `true` whenever the extension is
        // installed so the Settings UI can render a uniform row.
        marketplace_registered: false,
        plugin_installed: false,
        plugin_enabled: false,
        detection_fallback: None,
    };
    if !on_path {
        gemini_fs_fallback(&mut out, home);
        return out;
    }

    match run_plugin_cli_capture("gemini", &["extensions", "list", "-o", "json"]) {
        Ok(o) if o.success => {
            // Gemini CLI 0.41.2 emits the JSON payload to **stderr**
            // (with stdout empty). Try stdout first, then stderr — be
            // defensive in case future versions move it back.
            let payload = if !o.stdout.trim().is_empty() {
                &o.stdout
            } else {
                &o.stderr
            };
            if let Some(p) = parse_gemini_extensions_list_json(payload) {
                out.plugin_installed = p.installed;
                out.plugin_enabled = p.enabled;
                out.marketplace_registered = p.installed;
                return out;
            }
            gemini_fs_fallback(&mut out, home);
        }
        Ok(_) | Err(_) => gemini_fs_fallback(&mut out, home),
    }
    out
}

fn gemini_fs_fallback(out: &mut CliStatus, home: Option<&Path>) {
    out.detection_fallback = Some("fs");
    let Some(home) = home else { return };
    let ext_dir = gemini_extension_dir(home);
    let installed = ext_dir.is_dir() && ext_dir.join("gemini-extension.json").is_file();
    out.plugin_installed = installed;
    out.plugin_enabled = installed;
    out.marketplace_registered = installed;
}

// ---- output parsers --------------------------------------------------------

/// Search Copilot's `plugin list` output for our entry. Looks for the
/// substring `wt-agent-hooks@wt-local` anywhere in the output —
/// deliberately ignores the leading bullet character because Node-based
/// CLIs on Windows often emit UTF-8 bytes that get reinterpreted as
/// cp850/cp1252 when stdout is not connected to a TTY (so the real `•`
/// can show up as garbage like `ΓÇó`).
fn parse_copilot_plugin_list(stdout: &str) -> bool {
    let needle = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    stdout.contains(&needle)
}

/// Search for our marketplace name in the `Registered marketplaces:`
/// section. We only consider lines after the section header so the
/// "Included with GitHub Copilot:" preamble (built-in marketplaces we
/// don't own) doesn't produce false positives.
///
/// Encoding-agnostic: matches on `<MARKETPLACE> ` (with a trailing
/// space) so we don't depend on the rendered bullet character.
fn parse_copilot_marketplace_list(stdout: &str) -> bool {
    let mut in_registered = false;
    for l in stdout.lines() {
        let trimmed = l.trim_end();
        if trimmed.contains("Registered marketplaces") {
            in_registered = true;
            continue;
        }
        if !in_registered {
            continue;
        }
        // Look for `<marketplace> (` or `<marketplace>` at end-of-line,
        // anywhere on the line. Avoids depending on the leading bullet.
        let needle_paren = format!("{} (", COPILOT_MARKETPLACE_NAME);
        if trimmed.contains(&needle_paren) || trimmed.ends_with(COPILOT_MARKETPLACE_NAME) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy)]
struct PluginPresence {
    installed: bool,
    enabled: bool,
}

/// Parse `claude plugin list --json` output. Returns `None` if the JSON
/// doesn't conform — caller falls back to fs heuristics.
///
/// Sample (Claude 2.1.133):
/// `[{"id":"wt-agent-hooks@wt-local","version":"0.1.0","scope":"user",
///    "enabled":true,"installPath":"...","installedAt":"...",...}]`
fn parse_claude_plugin_list_json(stdout: &str) -> Option<PluginPresence> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    let arr = v.as_array()?;
    let id_target = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    for entry in arr {
        let id = entry.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id == id_target {
            let enabled = entry
                .get("enabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
            return Some(PluginPresence {
                installed: true,
                enabled,
            });
        }
    }
    Some(PluginPresence {
        installed: false,
        enabled: false,
    })
}

/// Parse `claude plugin marketplace list --json`. Looks for any entry
/// with `name == "wt-local"`.
fn parse_claude_marketplace_list_json(stdout: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    let arr = v.as_array()?;
    Some(arr.iter().any(|e| {
        e.get("name").and_then(|x| x.as_str()) == Some(COPILOT_MARKETPLACE_NAME)
    }))
}

/// Parse `gemini extensions list -o json`. Looks for our extension by
/// `name`. `enabled` derives from `isActive` (the field gemini surfaces
/// for "is this extension active in the current scope?").
fn parse_gemini_extensions_list_json(stdout: &str) -> Option<PluginPresence> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    let arr = v.as_array()?;
    for entry in arr {
        let name = entry.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if name == GEMINI_EXTENSION_DIR_NAME {
            let enabled = entry
                .get("isActive")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
            return Some(PluginPresence {
                installed: true,
                enabled,
            });
        }
    }
    Some(PluginPresence {
        installed: false,
        enabled: false,
    })
}

// ---- uninstall -------------------------------------------------------------

fn uninstall_for(cli: CliKind, home: Option<&Path>) -> CliUninstallResult {
    match cli {
        CliKind::Copilot => copilot_uninstall(home),
        CliKind::Claude => claude_uninstall(home),
        CliKind::Gemini => gemini_uninstall(home),
    }
}

fn copilot_uninstall(_home: Option<&Path>) -> CliUninstallResult {
    let mut out = CliUninstallResult {
        name: CliKind::Copilot.name(),
        attempted: false,
        plugin_uninstalled: None,
        marketplace_removed: None,
        staging_dir_removed: false,
        messages: Vec::new(),
    };
    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);

    if which::which("copilot").is_ok() {
        out.attempted = true;
        out.plugin_uninstalled = Some(spawn_step(
            &mut out.messages,
            "copilot",
            &["plugin", "uninstall", &plugin_ref],
        ));
        // `--force`: marketplace removal would otherwise refuse if
        // anything is still installed under it (e.g. previous step
        // failed). Belt-and-braces.
        out.marketplace_removed = Some(spawn_step(
            &mut out.messages,
            "copilot",
            &["plugin", "marketplace", "remove", COPILOT_MARKETPLACE_NAME, "--force"],
        ));
    } else {
        out.messages.push("copilot CLI not on PATH; skipped CLI steps".into());
    }

    out.staging_dir_removed =
        remove_staging_dir(&mut out.messages, copilot_plugin_source_dir());
    out
}

fn claude_uninstall(home: Option<&Path>) -> CliUninstallResult {
    let mut out = CliUninstallResult {
        name: CliKind::Claude.name(),
        attempted: false,
        plugin_uninstalled: None,
        marketplace_removed: None,
        staging_dir_removed: false,
        messages: Vec::new(),
    };
    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);

    if which::which("claude").is_ok() {
        out.attempted = true;
        out.plugin_uninstalled = Some(spawn_step(
            &mut out.messages,
            "claude",
            &["plugin", "uninstall", &plugin_ref],
        ));
        out.marketplace_removed = Some(spawn_step(
            &mut out.messages,
            "claude",
            &["plugin", "marketplace", "remove", COPILOT_MARKETPLACE_NAME],
        ));
    } else {
        out.messages.push("claude CLI not on PATH; skipped CLI steps".into());
    }

    out.staging_dir_removed =
        remove_staging_dir(&mut out.messages, claude_plugin_source_dir());

    // Belt-and-braces: clean up the legacy hooks block we may have
    // written in pre-plugin-install builds. install_for_claude already
    // does this on every startup, but uninstall should leave nothing
    // behind either way.
    if let Some(home) = home {
        let settings_path = home.join(".claude").join("settings.json");
        if let Err(e) = cleanup_legacy_claude_hooks(&settings_path) {
            out.messages.push(format!(
                "legacy hooks cleanup failed at {}: {}",
                settings_path.display(),
                e,
            ));
        }
    }

    out
}

fn gemini_uninstall(home: Option<&Path>) -> CliUninstallResult {
    let mut out = CliUninstallResult {
        name: CliKind::Gemini.name(),
        attempted: false,
        // Gemini has no marketplace surface.
        plugin_uninstalled: None,
        marketplace_removed: None,
        staging_dir_removed: false,
        messages: Vec::new(),
    };

    let cli_ok = which::which("gemini").is_ok();
    if cli_ok {
        out.attempted = true;
        out.plugin_uninstalled = Some(spawn_step(
            &mut out.messages,
            "gemini",
            &["extensions", "uninstall", GEMINI_EXTENSION_DIR_NAME],
        ));
    } else {
        out.messages.push("gemini CLI not on PATH; will remove extension dir directly".into());
    }

    // Whether or not the CLI step succeeded, remove the on-disk dir so
    // we leave no orphan files. Gemini's own uninstall normally does
    // this, so the second sweep is a no-op when the CLI succeeded.
    if let Some(home) = home {
        let ext_dir = gemini_extension_dir(home);
        if ext_dir.exists() {
            match fs::remove_dir_all(&ext_dir) {
                Ok(_) => {
                    out.staging_dir_removed = true;
                    out.messages
                        .push(format!("removed {}", ext_dir.display()));
                }
                Err(e) => out.messages.push(format!(
                    "failed to remove {}: {}",
                    ext_dir.display(),
                    e,
                )),
            }
        } else {
            out.staging_dir_removed = true;
        }
    }
    out
}

/// Spawn `<exe>` with `args` and append a one-line summary to
/// `messages`. Returns true on success. Never propagates errors —
/// uninstall is best-effort by design.
fn spawn_step(messages: &mut Vec<String>, exe: &str, args: &[&str]) -> bool {
    match run_plugin_cli_capture(exe, args) {
        Ok(o) if o.success => {
            messages.push(format!("ok: {} {}", exe, args.join(" ")));
            true
        }
        Ok(o) => {
            let combined = if o.stderr.trim().is_empty() {
                o.stdout.trim().to_string()
            } else {
                o.stderr.trim().to_string()
            };
            messages.push(format!(
                "fail ({}): {} {} :: {}",
                o.status_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".into()),
                exe,
                args.join(" "),
                combined,
            ));
            false
        }
        Err(e) => {
            messages.push(format!("error: {} {} :: {}", exe, args.join(" "), e));
            false
        }
    }
}

/// Delete the `LOCALAPPDATA\IntelligentTerminal\<cli>-plugin-src\wt-local\`
/// staging dir. Returns true when it didn't exist *or* was removed
/// successfully, false on a real removal error.
fn remove_staging_dir(messages: &mut Vec<String>, dir: Option<PathBuf>) -> bool {
    let Some(dir) = dir else {
        messages.push("could not resolve LOCALAPPDATA; staging dir untouched".into());
        return false;
    };
    if !dir.exists() {
        return true;
    }
    match fs::remove_dir_all(&dir) {
        Ok(_) => {
            messages.push(format!("removed staging dir {}", dir.display()));
            true
        }
        Err(e) => {
            messages.push(format!(
                "failed to remove staging dir {}: {}",
                dir.display(),
                e,
            ));
            false
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_dir(label: &str) -> PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("wta-hooks-{}-{}-{}", label, pid, n));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // ---- helper / generator tests ---------------------------------------

    // ---- matches_idempotency_substring (Track 1 #17 task 2) -------------

    /// Captured stderr strings from real CLI runs (see PR description /
    /// session SQL `probe_findings` table for the raw probe output).
    /// These are the strings the matcher is built to recognize.
    const COPILOT_MARKETPLACE_ALREADY: &str =
        "Failed to add marketplace: Marketplace \"wt-local\" already registered";
    const GEMINI_INSTALL_ALREADY: &str =
        "Extension \"wt-agent-hooks\" is already installed. Please uninstall it first.";

    #[test]
    fn matches_idempotency_substring_matches_real_copilot_output() {
        assert!(matches_idempotency_substring(
            "",
            COPILOT_MARKETPLACE_ALREADY,
            &["already registered"]
        ));
    }

    #[test]
    fn matches_idempotency_substring_matches_real_gemini_output() {
        assert!(matches_idempotency_substring(
            "",
            GEMINI_INSTALL_ALREADY,
            &["already installed"]
        ));
    }

    #[test]
    fn matches_idempotency_substring_is_case_insensitive() {
        assert!(matches_idempotency_substring(
            "",
            "ALREADY REGISTERED",
            &["already registered"]
        ));
        assert!(matches_idempotency_substring(
            "Already Installed",
            "",
            &["already installed"]
        ));
    }

    #[test]
    fn matches_idempotency_substring_checks_both_streams() {
        assert!(matches_idempotency_substring(
            "Plugin already registered",
            "",
            &["already registered"]
        ));
        assert!(matches_idempotency_substring(
            "",
            "Plugin already registered",
            &["already registered"]
        ));
    }

    #[test]
    fn matches_idempotency_substring_does_not_match_unrelated_failures() {
        // Generic failure stderr (path not found) must NOT be
        // misclassified as "already installed".
        let unrelated = "Failed to add marketplace: source path does not exist";
        assert!(!matches_idempotency_substring(
            "",
            unrelated,
            &["already registered"]
        ));
    }

    #[test]
    fn matches_idempotency_substring_empty_needles_never_matches() {
        // Call sites that pass &[] (claude marketplace add, claude
        // install, copilot install) must always treat non-zero exit
        // as Err — the empty-needles short-circuit is the contract.
        assert!(!matches_idempotency_substring(
            "anything goes here",
            "and here",
            &[]
        ));
    }

    #[test]
    fn matches_idempotency_substring_multi_needle_any_match_succeeds() {
        // Future-proofing: a single call site can register multiple
        // substrings (e.g. if a CLI changes wording across versions).
        assert!(matches_idempotency_substring(
            "",
            "Plugin already exists in marketplace",
            &["already registered", "already exists"]
        ));
    }

    // ---- Gemini staging-dir source resolution (Track 1 #17 task 1) ------

    /// Verify the Gemini staging path lives under the expected
    /// `gemini-plugin-src` subfolder of `intelligent_terminal_root()`.
    /// We can't easily probe `gemini extensions install` from a unit
    /// test, but pinning the directory layout catches a future change
    /// that breaks the path immediately.
    #[test]
    fn gemini_plugin_source_dir_lives_under_intelligent_terminal_root() {
        let Some(p) = gemini_plugin_source_dir() else {
            // intelligent_terminal_root() may legitimately return None
            // in CI without LOCALAPPDATA — skip silently.
            return;
        };
        let s = p.to_string_lossy();
        assert!(
            s.contains("gemini-plugin-src"),
            "expected gemini-plugin-src in path: {}",
            s
        );
        assert!(
            s.ends_with(GEMINI_EXTENSION_DIR_NAME),
            "expected path to end with {}: {}",
            GEMINI_EXTENSION_DIR_NAME,
            s
        );
    }

    /// All three `<cli>_plugin_source_dir()` helpers must return
    /// distinct paths so the staged content for one CLI never
    /// overwrites another's. Pin this so a future copy/paste typo is
    /// caught at test time.
    #[test]
    fn source_dir_helpers_return_distinct_paths() {
        let claude = claude_plugin_source_dir();
        let copilot = copilot_plugin_source_dir();
        let gemini = gemini_plugin_source_dir();
        match (claude, copilot, gemini) {
            (Some(a), Some(b), Some(c)) => {
                assert_ne!(a, b, "claude and copilot staging paths collide");
                assert_ne!(a, c, "claude and gemini staging paths collide");
                assert_ne!(b, c, "copilot and gemini staging paths collide");
            }
            _ => {
                // intelligent_terminal_root() returned None — skip.
            }
        }
    }

    #[test]
    fn write_gemini_extension_files_creates_layout() {
        let dir = unique_dir("gemini-stage");
        write_gemini_extension_files(&dir).unwrap();

        assert!(dir.join("gemini-extension.json").is_file());
        assert!(dir.join("hooks").join("hooks.json").is_file());
        assert!(dir.join("hooks").join("send-event.ps1").is_file());

        // Idempotent: running again is a no-op (no panic, no error).
        write_gemini_extension_files(&dir).unwrap();
    }

    /// `bundle::read_with_roots` resolves loose copies in priority
    /// order and falls back to the embedded blob when nothing matches.
    /// We exercise the inner helper directly (rather than via
    /// [`bundle::read`]) so we don't have to mutate process-wide env
    /// state in a parallel test runner.
    #[test]
    fn bundle_read_resolves_loose_then_falls_back_to_embedded() {
        let dir = unique_dir("bundle");
        let loose_script_dir = dir.join("agent-hooks-plugin").join("hooks");
        fs::create_dir_all(&loose_script_dir).unwrap();
        let loose_marker = "# LOOSE OVERRIDE FOR TEST\n";
        fs::write(loose_script_dir.join("send-event.ps1"), loose_marker).unwrap();

        let roots = vec![dir.clone()];

        // Loose copy wins for the file present in the override dir.
        let resolved = bundle::read_with_roots(BundleFile::SendEventPs1, &roots);
        assert_eq!(
            resolved.as_ref(),
            loose_marker,
            "expected loose copy to win over embedded fallback",
        );

        // Files NOT present in the override dir fall through to embedded.
        let manifest = bundle::read_with_roots(BundleFile::GeminiExtensionJson, &roots);
        let parsed: serde_json::Value =
            serde_json::from_str(manifest.as_ref()).expect("embedded gemini manifest parses");
        assert_eq!(
            parsed.get("name").and_then(|v| v.as_str()),
            Some(GEMINI_EXTENSION_DIR_NAME),
            "embedded gemini-extension.json missing expected name",
        );

        // With an empty root list we always get the embedded fallback.
        let embedded = bundle::read_with_roots(BundleFile::SendEventPs1, &[]);
        assert!(
            embedded.as_ref().contains("send-event.ps1"),
            "embedded send-event.ps1 should contain its own banner comment",
        );
    }

    #[test]
    fn plugin_hooks_json_uses_plugin_root_variable() {
        let v = plugin_hooks_json_value("copilot");
        let s = v.to_string();
        assert!(
            s.contains("${CLAUDE_PLUGIN_ROOT}/hooks/send-event.ps1"),
            "expected ${{CLAUDE_PLUGIN_ROOT}}-relative path: {}",
            s
        );
        for (event_name, event_id) in HOOK_EVENTS {
            assert!(s.contains(event_name), "missing event name: {}", event_name);
            assert!(s.contains(event_id), "missing event id: {}", event_id);
        }
        assert!(
            s.contains("-CliSource copilot"),
            "expected -CliSource copilot in command: {}",
            s
        );
    }

    #[test]
    fn plugin_hooks_json_threads_cli_source_through() {
        let v = plugin_hooks_json_value("claude");
        let s = v.to_string();
        assert!(
            s.contains("-CliSource claude"),
            "expected -CliSource claude in command: {}",
            s
        );
        assert!(
            !s.contains("-CliSource copilot"),
            "did not expect -CliSource copilot in claude output: {}",
            s
        );
    }

    #[test]
    fn write_plugin_files_creates_layout() {
        let dir = unique_dir("plugin-files");
        write_plugin_files(&dir, "copilot").unwrap();

        let manifest = dir.join(".claude-plugin").join("plugin.json");
        let hooks = dir.join("hooks").join("hooks.json");
        let script = dir.join("hooks").join("send-event.ps1");

        assert!(manifest.is_file(), "missing plugin.json: {}", manifest.display());
        assert!(hooks.is_file(), "missing hooks.json: {}", hooks.display());
        assert!(script.is_file(), "missing send-event.ps1: {}", script.display());

        let hooks_text = fs::read_to_string(&hooks).unwrap();
        assert!(hooks_text.contains("-CliSource copilot"));

        // Idempotent: running again is a no-op (no panic, no error).
        write_plugin_files(&dir, "copilot").unwrap();
    }

    #[test]
    fn write_plugin_files_threads_cli_source_into_hooks() {
        let dir = unique_dir("plugin-files-claude");
        write_plugin_files(&dir, "claude").unwrap();
        let hooks_text = fs::read_to_string(dir.join("hooks").join("hooks.json")).unwrap();
        assert!(hooks_text.contains("-CliSource claude"));
        assert!(!hooks_text.contains("-CliSource copilot"));
    }

    #[test]
    fn write_plugin_files_removes_legacy_root_manifest() {
        let dir = unique_dir("plugin-stale");
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("plugin.json");
        fs::write(&stale, "{\"name\":\"old\"}").unwrap();

        write_plugin_files(&dir, "copilot").unwrap();
        assert!(
            !stale.exists(),
            "expected stale root plugin.json to be removed: {}",
            stale.display()
        );
        assert!(dir.join(".claude-plugin").join("plugin.json").is_file());
    }

    #[test]
    fn write_marketplace_files_creates_catalog() {
        let dir = unique_dir("marketplace");
        write_marketplace_files(&dir).unwrap();
        let mkt = dir.join(".claude-plugin").join("marketplace.json");
        assert!(mkt.is_file(), "missing marketplace.json: {}", mkt.display());
        let v: Value = serde_json::from_str(&fs::read_to_string(&mkt).unwrap()).unwrap();
        assert_eq!(v.get("name").and_then(|x| x.as_str()), Some(COPILOT_MARKETPLACE_NAME));
        let plugins = v.get("plugins").and_then(|x| x.as_array()).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(
            plugins[0].get("name").and_then(|x| x.as_str()),
            Some(COPILOT_PLUGIN_NAME)
        );
    }

    // ---- cleanup_legacy_claude_hooks ------------------------------------

    #[test]
    fn cleanup_legacy_claude_hooks_noop_when_file_missing() {
        let dir = unique_dir("cleanup-missing");
        let path = dir.join("settings.json");
        cleanup_legacy_claude_hooks(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_legacy_claude_hooks_removes_wta_entries() {
        let dir = unique_dir("cleanup-removes");
        let path = dir.join("settings.json");
        let before = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "powershell -ExecutionPolicy Bypass -File \"C:\\\\foo\\\\send-event.ps1\" -CliSource claude agent.session.start"
                        }]
                    },
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "echo user-defined hook"
                        }]
                    }
                ]
            },
            "model": "sonnet"
        });
        fs::write(&path, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // Unrelated key preserved.
        assert_eq!(after.get("model").and_then(|v| v.as_str()), Some("sonnet"));
        // User-defined hook preserved.
        let arr = after
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]
            .get("hooks")
            .and_then(|h| h.as_array())
            .unwrap()[0]
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "echo user-defined hook");
    }

    #[test]
    fn cleanup_legacy_claude_hooks_strips_empty_hooks_object() {
        let dir = unique_dir("cleanup-empty");
        let path = dir.join("settings.json");
        let before = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "powershell -ExecutionPolicy Bypass -File \"C:\\\\foo\\\\send-event.ps1\" -CliSource claude agent.session.start"
                        }]
                    }
                ]
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            after.get("hooks").is_none(),
            "expected empty hooks object to be removed: {}",
            after
        );
    }

    #[test]
    fn cleanup_legacy_claude_hooks_idempotent_on_clean_file() {
        let dir = unique_dir("cleanup-clean");
        let path = dir.join("settings.json");
        let before = serde_json::json!({ "model": "sonnet" });
        let serialized = serde_json::to_string_pretty(&before).unwrap();
        fs::write(&path, &serialized).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        // File should not have been rewritten (content identical).
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, serialized);
    }

    #[test]
    fn cleanup_legacy_claude_hooks_skips_malformed_json() {
        let dir = unique_dir("cleanup-malformed");
        let path = dir.join("settings.json");
        fs::write(&path, "{ this is not valid json").unwrap();

        // Must not panic; must not rewrite the file.
        cleanup_legacy_claude_hooks(&path).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, "{ this is not valid json");
    }

    // ---- Gemini extension layout ----------------------------------------

    // (install_for_gemini_writes_full_extension_layout test deleted —
    //  Track 1 #17 task 1 moved Gemini install to the plugin-CLI flow,
    //  so install_for_gemini no longer writes into ~/.gemini/extensions/
    //  directly. Coverage is now: write_gemini_extension_files_creates_
    //  layout exercises the staging step, and the install spawn itself
    //  is verified end-to-end manually — see PR description.)

    #[test]
    fn install_for_gemini_is_noop_when_gemini_not_installed() {
        let home = unique_dir("gemini-absent");
        // .gemini deliberately missing.
        install_for_gemini(&home);
        assert!(!home.join(".gemini").exists());
    }

    // ---- status / uninstall parsers (Track 2) ---------------------------

    /// Real `copilot plugin list` output captured 2026-05-08 (Copilot
    /// CLI 1.0.44-2). Asserts our parser finds the wt-agent-hooks
    /// entry by `<plugin>@<marketplace>` prefix.
    #[test]
    fn copilot_plugin_list_parser_finds_our_entry() {
        let stdout = "\
Installed plugins:
  • superpowers@superpowers-marketplace (v5.1.0)
  • wt-agent-hooks@wt-local (v0.1.0)
";
        assert!(parse_copilot_plugin_list(stdout));
    }

    #[test]
    fn copilot_plugin_list_parser_returns_false_when_missing() {
        let stdout = "\
Installed plugins:
  • superpowers@superpowers-marketplace (v5.1.0)
";
        assert!(!parse_copilot_plugin_list(stdout));
    }

    #[test]
    fn copilot_plugin_list_parser_returns_false_when_empty() {
        assert!(!parse_copilot_plugin_list(""));
    }

    /// Real `copilot plugin marketplace list` output. Built-in
    /// marketplaces appear before the "Registered marketplaces:"
    /// header; only entries below that header should count.
    #[test]
    fn copilot_marketplace_list_parser_only_counts_registered() {
        let stdout = "\
✨ Included with GitHub Copilot:
  ◆ copilot-plugins (GitHub: github/copilot-plugins)
  ◆ awesome-copilot (GitHub: github/awesome-copilot)

Registered marketplaces:
  • superpowers-marketplace (GitHub: obra/superpowers-marketplace)
  • wt-local (Local: C:\\Users\\u\\.copilot\\installed-plugins\\wt-local)
";
        assert!(parse_copilot_marketplace_list(stdout));
    }

    #[test]
    fn copilot_marketplace_list_parser_ignores_builtin_only() {
        let stdout = "\
✨ Included with GitHub Copilot:
  ◆ wt-local (GitHub: bogus/wt-local)

Registered marketplaces:
  • superpowers-marketplace (GitHub: obra/superpowers-marketplace)
";
        // wt-local appears in the included list, NOT registered.
        // Parser should refuse to count it.
        assert!(!parse_copilot_marketplace_list(stdout));
    }

    /// Real `claude plugin list --json` output captured 2026-05-08
    /// (Claude Code 2.1.133).
    #[test]
    fn claude_plugin_list_json_parser_extracts_enabled_flag() {
        let stdout = r#"[{"id":"wt-agent-hooks@wt-local","version":"0.1.0","scope":"user","enabled":true,"installPath":"C:\\Users\\u\\.claude\\plugins\\cache\\wt-local\\wt-agent-hooks\\0.1.0","installedAt":"2026-05-08T11:29:58.295Z","lastUpdated":"2026-05-08T11:29:58.295Z"}]"#;
        let p = parse_claude_plugin_list_json(stdout).expect("parses");
        assert!(p.installed);
        assert!(p.enabled);
    }

    #[test]
    fn claude_plugin_list_json_parser_reports_disabled() {
        let stdout = r#"[{"id":"wt-agent-hooks@wt-local","version":"0.1.0","scope":"user","enabled":false}]"#;
        let p = parse_claude_plugin_list_json(stdout).expect("parses");
        assert!(p.installed);
        assert!(!p.enabled);
    }

    #[test]
    fn claude_plugin_list_json_parser_handles_empty_array() {
        let p = parse_claude_plugin_list_json("[]").expect("parses");
        assert!(!p.installed);
        assert!(!p.enabled);
    }

    #[test]
    fn claude_plugin_list_json_parser_returns_none_on_garbage() {
        assert!(parse_claude_plugin_list_json("not json").is_none());
    }

    /// Real `claude plugin marketplace list --json` output.
    #[test]
    fn claude_marketplace_list_json_parser_finds_wt_local() {
        let stdout = r#"[{"name":"claude-plugins-official","source":"github","repo":"anthropics/claude-plugins-official","installLocation":"x"},{"name":"wt-local","source":"directory","path":"y","installLocation":"z"}]"#;
        assert_eq!(parse_claude_marketplace_list_json(stdout), Some(true));
    }

    #[test]
    fn claude_marketplace_list_json_parser_missing_returns_false() {
        let stdout = r#"[{"name":"claude-plugins-official"}]"#;
        assert_eq!(parse_claude_marketplace_list_json(stdout), Some(false));
    }

    /// Real `gemini extensions list -o json` output. Asserts we read
    /// `isActive` for the enabled flag.
    #[test]
    fn gemini_extensions_list_json_parser_uses_is_active() {
        let stdout = r#"[{"name":"wt-agent-hooks","version":"0.1.0","path":"x","isActive":true,"hooks":{},"id":"abc"}]"#;
        let p = parse_gemini_extensions_list_json(stdout).expect("parses");
        assert!(p.installed);
        assert!(p.enabled);
    }

    #[test]
    fn gemini_extensions_list_json_parser_reports_inactive() {
        let stdout = r#"[{"name":"wt-agent-hooks","version":"0.1.0","path":"x","isActive":false}]"#;
        let p = parse_gemini_extensions_list_json(stdout).expect("parses");
        assert!(p.installed);
        assert!(!p.enabled);
    }

    #[test]
    fn gemini_extensions_list_json_parser_missing_extension() {
        let p = parse_gemini_extensions_list_json("[]").expect("parses");
        assert!(!p.installed);
    }

    /// Filesystem fallback: when the CLI step can't run, the
    /// per-CLI helpers should report state from on-disk artifacts and
    /// tag the row with `detection_fallback="fs"`. We exercise the
    /// fallback function directly so we don't depend on PATH state.
    #[test]
    fn copilot_fs_fallback_detects_installed_layout() {
        let home = unique_dir("copilot-fs");
        let copilot_dir = home.join(".copilot");
        fs::create_dir_all(&copilot_dir).unwrap();
        // Source of truth: ~/.copilot/config.json.
        let config = serde_json::json!({
            "installedPlugins": [
                { "name": "wt-agent-hooks", "marketplace": "wt-local",
                  "version": "0.1.0", "enabled": true,
                  "cache_path": "C:\\fake" }
            ],
            "extraKnownMarketplaces": {
                "wt-local": { "type": "local", "path": "C:\\fake" }
            }
        });
        fs::write(copilot_dir.join("config.json"), config.to_string()).unwrap();

        let mut out = CliStatus {
            name: CliKind::Copilot.name(),
            binary_on_path: false,
            binary_path: None,
            marketplace_registered: false,
            plugin_installed: false,
            plugin_enabled: false,
            detection_fallback: None,
        };
        copilot_fs_fallback(&mut out, Some(&home));
        assert_eq!(out.detection_fallback, Some("fs"));
        assert!(out.plugin_installed);
        assert!(out.plugin_enabled);
        assert!(out.marketplace_registered);
    }

    /// The cache_path subdirectory may exist empty (Copilot
    /// lazy-populates it). config.json is the truth — when it lists
    /// our plugin we report installed even if the cache dir is empty.
    #[test]
    fn copilot_fs_fallback_uses_config_not_dir_contents() {
        let home = unique_dir("copilot-fs-empty-cache");
        let copilot_dir = home.join(".copilot");
        let cache_dir = copilot_dir
            .join("installed-plugins")
            .join(COPILOT_MARKETPLACE_NAME)
            .join(COPILOT_PLUGIN_DIR_NAME);
        fs::create_dir_all(&cache_dir).unwrap();
        // Cache dir exists but is empty — would fail the old
        // hooks/hooks.json existence check.
        let config = serde_json::json!({
            "installedPlugins": [
                { "name": "wt-agent-hooks", "marketplace": "wt-local",
                  "enabled": true }
            ]
        });
        fs::write(copilot_dir.join("config.json"), config.to_string()).unwrap();

        let mut out = CliStatus {
            name: CliKind::Copilot.name(),
            binary_on_path: false,
            binary_path: None,
            marketplace_registered: false,
            plugin_installed: false,
            plugin_enabled: false,
            detection_fallback: None,
        };
        copilot_fs_fallback(&mut out, Some(&home));
        assert!(out.plugin_installed);
        assert!(out.plugin_enabled);
    }

    #[test]
    fn copilot_fs_fallback_reports_disabled_plugin() {
        let home = unique_dir("copilot-fs-disabled");
        let copilot_dir = home.join(".copilot");
        fs::create_dir_all(&copilot_dir).unwrap();
        let config = serde_json::json!({
            "installedPlugins": [
                { "name": "wt-agent-hooks", "marketplace": "wt-local",
                  "enabled": false }
            ]
        });
        fs::write(copilot_dir.join("config.json"), config.to_string()).unwrap();

        let mut out = CliStatus {
            name: CliKind::Copilot.name(),
            binary_on_path: false,
            binary_path: None,
            marketplace_registered: false,
            plugin_installed: false,
            plugin_enabled: false,
            detection_fallback: None,
        };
        copilot_fs_fallback(&mut out, Some(&home));
        assert!(out.plugin_installed);
        assert!(!out.plugin_enabled);
    }

    #[test]
    fn gemini_fs_fallback_detects_extension_dir() {
        let home = unique_dir("gemini-fs");
        let ext_dir = gemini_extension_dir(&home);
        fs::create_dir_all(&ext_dir).unwrap();
        fs::write(ext_dir.join("gemini-extension.json"), "{}").unwrap();

        let mut out = CliStatus {
            name: CliKind::Gemini.name(),
            binary_on_path: false,
            binary_path: None,
            marketplace_registered: false,
            plugin_installed: false,
            plugin_enabled: false,
            detection_fallback: None,
        };
        gemini_fs_fallback(&mut out, Some(&home));
        assert_eq!(out.detection_fallback, Some("fs"));
        assert!(out.plugin_installed);
    }

    #[test]
    fn gemini_fs_fallback_reports_absent_extension() {
        let home = unique_dir("gemini-fs-absent");
        let mut out = CliStatus {
            name: CliKind::Gemini.name(),
            binary_on_path: false,
            binary_path: None,
            marketplace_registered: false,
            plugin_installed: false,
            plugin_enabled: false,
            detection_fallback: None,
        };
        gemini_fs_fallback(&mut out, Some(&home));
        assert!(!out.plugin_installed);
    }

    /// `CliKind::from_name` parses the canonical names (case-insensitive).
    #[test]
    fn cli_kind_from_name_parses_canonical_strings() {
        assert_eq!(CliKind::from_name("copilot"), Some(CliKind::Copilot));
        assert_eq!(CliKind::from_name("CLAUDE"), Some(CliKind::Claude));
        assert_eq!(CliKind::from_name("Gemini"), Some(CliKind::Gemini));
        assert_eq!(CliKind::from_name("nope"), None);
    }

    /// `CliScope::All` includes every CLI; `CliScope::One` only the named one.
    #[test]
    fn cli_scope_filters_correctly() {
        assert!(CliScope::All.includes(CliKind::Copilot));
        assert!(CliScope::All.includes(CliKind::Claude));
        assert!(CliScope::All.includes(CliKind::Gemini));

        let one = CliScope::One(CliKind::Gemini);
        assert!(!one.includes(CliKind::Copilot));
        assert!(!one.includes(CliKind::Claude));
        assert!(one.includes(CliKind::Gemini));
    }

    #[test]
    fn jsonc_stripper_removes_line_comments_outside_strings() {
        let input = "// banner\n{\n  \"k\": \"v\", // trailing\n  \"u\": \"https://x\"\n}\n";
        let stripped = strip_jsonc_line_comments(input);
        // Strict serde_json now accepts the result.
        let v: Value = serde_json::from_str(&stripped).expect("parses after stripping");
        assert_eq!(v.get("k").and_then(|x| x.as_str()), Some("v"));
        // The `//` inside the URL string was NOT stripped.
        assert_eq!(v.get("u").and_then(|x| x.as_str()), Some("https://x"));
    }
}
