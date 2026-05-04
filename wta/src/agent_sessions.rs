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
                // If we're rebinding to a different pane, drop the old pane's mapping first.
                if let Some(old_pane) = entry.pane_session_id.take() {
                    if old_pane != pane_session_id {
                        self.active_by_pane.remove(&old_pane);
                    }
                }
                entry.cli_source       = cli_source;
                // Preserve an existing title (e.g. one loaded from disk by the
                // history loader) when the new event carries no replacement.
                // Live synth titles are sent only for genuinely new sessions
                // (route_agent_event_to_registry passes "" for resumed ones).
                if !title.is_empty() {
                    entry.title        = title;
                }
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

            SessionEvent::Notification { key, message } => {
                if let Some(entry) = self.sessions.get_mut(&key) {
                    entry.status            = AgentStatus::Attention;
                    entry.attention_reason  = Some(message);
                    entry.last_activity_at  = now;
                    self.dirty = true;
                }
            }

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
        }
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

    pub fn remove(&mut self, key: &AgentKey) {
        if let Some(s) = self.sessions.remove(key) {
            if let Some(pane) = s.pane_session_id {
                self.active_by_pane.remove(&pane);
            }
            self.dirty = true;
        }
    }

    /// Drop any synthetic `pane:<guid>` session bound to the given pane.
    /// Used when a real `agent.session.started` arrives to clean up the
    /// placeholder created by an earlier tool event with no agent_session_id.
    pub fn drop_synthetic_for_pane(&mut self, pane_session_id: &str) {
        if let Some(key) = self.active_by_pane.get(pane_session_id).cloned() {
            if key.starts_with("pane:") {
                self.sessions.remove(&key);
                self.active_by_pane.remove(pane_session_id);
                self.dirty = true;
            }
        }
    }

    /// Insert historical entries loaded from disk, skipping any whose key
    /// is already present (the live registry wins). Idempotent — safe to
    /// call multiple times. Used at startup.
    pub fn merge_historical(&mut self, loaded: Vec<AgentSession>) {
        for s in loaded {
            if self.sessions.contains_key(&s.key) {
                continue;
            }
            self.sessions.insert(s.key.clone(), s);
        }
        self.dirty = true;
    }

    /// Populate the registry with synthetic data covering all 6 statuses.
    /// Triggered by the `WTA_DEMO_AGENTS=1` env var on App startup so the
    /// Agents view (F2) can be exercised without running any real CLI.
    ///
    /// Layout (sorted by last_activity_at desc, newest first):
    ///   1. copilot  WORKING    — currently running a tool
    ///   2. claude   ATTENTION  — needs user approval
    ///   3. gemini   IDLE       — sitting waiting for input
    ///   4. copilot  ERROR      — connection failed
    ///   5. claude   ENDED      — exited normally a moment ago
    ///   6. gemini   HISTORICAL — loaded from an old log (no live pane)
    pub fn populate_demo_data(&mut self) {
        use std::time::Duration;

        let now = SystemTime::now();
        let cwd = PathBuf::from("C:/GitRepo/agentic-terminal");

        // 1. Working — copilot running a tool right now
        self.apply(SessionEvent::SessionStarted {
            key:             "demo-copilot-working".to_string(),
            cli_source:      CliSource::Copilot,
            pane_session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            cwd:             cwd.clone(),
            title:           "copilot — refactor agent_sessions".to_string(),
        });
        self.apply(SessionEvent::ToolStarting {
            key:       "demo-copilot-working".to_string(),
            tool_name: "shell".to_string(),
        });

        // 2. Attention — claude waiting for tool approval
        self.apply(SessionEvent::SessionStarted {
            key:             "demo-claude-attention".to_string(),
            cli_source:      CliSource::Claude,
            pane_session_id: "22222222-2222-2222-2222-222222222222".to_string(),
            cwd:             cwd.clone(),
            title:           "claude — write tests for registry".to_string(),
        });
        self.apply(SessionEvent::Notification {
            key:     "demo-claude-attention".to_string(),
            message: "Allow tool: write_file ./src/lib.rs?".to_string(),
        });

        // 3. Idle — gemini waiting for next prompt
        self.apply(SessionEvent::SessionStarted {
            key:             "demo-gemini-idle".to_string(),
            cli_source:      CliSource::Gemini,
            pane_session_id: "33333333-3333-3333-3333-333333333333".to_string(),
            cwd:             cwd.clone(),
            title:           "gemini — explain build system".to_string(),
        });

        // 4. Error — copilot lost network
        self.apply(SessionEvent::SessionStarted {
            key:             "demo-copilot-error".to_string(),
            cli_source:      CliSource::Copilot,
            pane_session_id: "44444444-4444-4444-4444-444444444444".to_string(),
            cwd:             cwd.clone(),
            title:           "copilot — fix CI failure".to_string(),
        });
        self.apply(SessionEvent::ConnectionFailed {
            pane_session_id: "44444444-4444-4444-4444-444444444444".to_string(),
            reason:          "API request failed: 503 Service Unavailable".to_string(),
        });

        // 5. Ended — claude finished cleanly a moment ago
        self.apply(SessionEvent::SessionStarted {
            key:             "demo-claude-ended".to_string(),
            cli_source:      CliSource::Claude,
            pane_session_id: "55555555-5555-5555-5555-555555555555".to_string(),
            cwd:             cwd.clone(),
            title:           "claude — review PR diff".to_string(),
        });
        self.apply(SessionEvent::SessionStopped {
            key:    "demo-claude-ended".to_string(),
            reason: "end_turn".to_string(),
        });

        // 6. Historical — loaded from old log, no live pane
        let two_hours_ago = now - Duration::from_secs(2 * 60 * 60);
        let key = "demo-gemini-historical".to_string();
        self.sessions.insert(key.clone(), AgentSession {
            key:               key,
            cli_source:        CliSource::Gemini,
            pane_session_id:   None,
            window_id:         None,
            tab_id:            None,
            title:             "gemini — earlier debug session".to_string(),
            cwd:               cwd.clone(),
            started_at:        two_hours_ago - Duration::from_secs(60 * 30),
            last_activity_at:  two_hours_ago,
            status:            AgentStatus::Historical,
            last_error:        None,
            current_tool:      None,
            attention_reason:  None,
            log_path:          Some(PathBuf::from("~/.gemini/logs/2026-05-03-1530.log")),
        });

        // Stagger last_activity_at so the order in the UI matches the
        // narrative (working newest, historical oldest).
        let stagger = |secs: u64| now - Duration::from_secs(secs);
        if let Some(s) = self.sessions.get_mut("demo-copilot-working")  { s.last_activity_at = stagger(2); }
        if let Some(s) = self.sessions.get_mut("demo-claude-attention") { s.last_activity_at = stagger(15); }
        if let Some(s) = self.sessions.get_mut("demo-gemini-idle")      { s.last_activity_at = stagger(45); }
        if let Some(s) = self.sessions.get_mut("demo-copilot-error")    { s.last_activity_at = stagger(120); }
        if let Some(s) = self.sessions.get_mut("demo-claude-ended")     { s.last_activity_at = stagger(300); }

        self.dirty = true;
    }
}

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

    // ─── Issue #2: SessionStarted rebinding pane leak ────────────────────────

    #[test]
    fn session_started_rebinding_to_new_pane_drops_old_pane_mapping() {
        let mut reg = AgentSessionRegistry::new();
        reg.apply(SessionEvent::SessionStarted {
            key: k("s"), cli_source: CliSource::Claude,
            pane_session_id: pane("old"), cwd: PathBuf::from("/x"), title: "t".into(),
        });
        reg.apply(SessionEvent::SessionStarted {
            key: k("s"), cli_source: CliSource::Claude,
            pane_session_id: pane("new"), cwd: PathBuf::from("/x"), title: "t".into(),
        });
        assert_eq!(reg.active_by_pane.get("new"), Some(&k("s")));
        assert!(reg.active_by_pane.get("old").is_none(), "old pane mapping must be dropped");

        // Closing the OLD pane must NOT mark the session ended.
        reg.apply(SessionEvent::PaneClosed { pane_session_id: pane("old") });
        assert_eq!(reg.sessions.get("s").unwrap().status, AgentStatus::Idle);
    }

    #[test]
    fn populate_demo_data_creates_one_session_per_status() {
        let mut reg = AgentSessionRegistry::new();
        reg.populate_demo_data();
        let sessions = reg.iter_sorted();
        assert_eq!(sessions.len(), 6, "demo data should yield exactly 6 sessions");

        // Verify each status appears exactly once.
        let statuses: Vec<AgentStatus> = sessions.iter().map(|s| s.status.clone()).collect();
        for st in [
            AgentStatus::Working,
            AgentStatus::Attention,
            AgentStatus::Idle,
            AgentStatus::Error,
            AgentStatus::Ended,
            AgentStatus::Historical,
        ] {
            assert_eq!(statuses.iter().filter(|s| **s == st).count(), 1, "expected exactly one {:?}", st);
        }

        // Working session must come first (most recent activity).
        assert_eq!(sessions[0].status, AgentStatus::Working);
        // Historical session must be last and have no live pane binding.
        assert_eq!(sessions[5].status, AgentStatus::Historical);
        assert!(sessions[5].pane_session_id.is_none());

        // Error session must carry the failure reason.
        let err = sessions.iter().find(|s| s.status == AgentStatus::Error).unwrap();
        assert!(err.last_error.is_some());

        // Attention session must carry an attention reason.
        let att = sessions.iter().find(|s| s.status == AgentStatus::Attention).unwrap();
        assert!(att.attention_reason.is_some());
    }

    #[test]
    fn merge_historical_inserts_only_new_keys() {
        let mut reg = AgentSessionRegistry::new();
        // Pre-existing live session.
        reg.apply(SessionEvent::SessionStarted {
            key:             "live-1".into(),
            cli_source:      CliSource::Copilot,
            pane_session_id: "p".into(),
            cwd:             PathBuf::from("/x"),
            title:           "live".into(),
        });

        let now = SystemTime::now();
        let mk_hist = |key: &str| AgentSession {
            key:               key.to_string(),
            cli_source:        CliSource::Claude,
            pane_session_id:   None,
            window_id:         None, tab_id: None,
            title:             format!("hist {}", key),
            cwd:               PathBuf::from("/y"),
            started_at:        now,
            last_activity_at:  now,
            status:            AgentStatus::Historical,
            last_error:        None,
            current_tool:      None,
            attention_reason:  None,
            log_path:          None,
        };

        // Loaded set tries to overwrite live-1 + add hist-1.
        reg.merge_historical(vec![
            mk_hist("live-1"),
            mk_hist("hist-1"),
        ]);

        // live-1 must remain Working/Idle (Copilot, with pane), NOT Historical.
        let live = reg.sessions.get("live-1").unwrap();
        assert_eq!(live.cli_source, CliSource::Copilot);
        assert_ne!(live.status, AgentStatus::Historical);
        assert!(live.pane_session_id.is_some());

        // hist-1 must be added as Historical.
        let hist = reg.sessions.get("hist-1").unwrap();
        assert_eq!(hist.status, AgentStatus::Historical);
    }
}
