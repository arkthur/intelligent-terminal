//! In-memory registry of currently-alive ACP sessions.
//!
//! Used by both the master (truth source) and each helper (a push-updated
//! mirror). Master maintains it as the authoritative view of "which sessions
//! are connected right now"; helpers receive `intellterm.wta/session_added`
//! and `session_removed` ext-notifications and apply them locally so the
//! F2 session-manager Enter routing can decide focus vs. resume with zero
//! IPC round-trip.
//!
//! The trait surface is intentionally tiny and async (matching the master's
//! existing `tokio::sync::Mutex` convention on `session_to_helper`). The
//! interior of `InMemoryRegistry` is a plain HashMap behind a tokio mutex —
//! operations are sub-µs CPU work, no awaits held across the lock. Switching
//! to a sync lock model is tracked as a follow-up PR; it stays out of scope
//! here to avoid mixing a lock refactor into the routing change.

use agent_client_protocol as acp;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// One row in the registry. Mirrors the fields the F2 view needs:
///
/// * `session_id` — the ACP session GUID (truth-source key).
/// * `cwd`        — required by ACP `SessionInfo` for `session/list`
///                  responses; populated from `NewSessionRequest.cwd` at
///                  insertion time.
/// * `title`      — optional human-friendly label; `None` until we wire a
///                  title source (e.g. derived from the first user prompt).
/// * `updated_at` — optional ISO-8601 timestamp of the last activity, kept
///                  here so `session/list` responses match agents that
///                  populate it; we leave it `None` for now (history sort
///                  uses local `agent-pane-sessions.jsonl` provenance).
/// * `pane_session_id` — the WT pane GUID (`WT_SESSION`) that owns this
///                  ACP session. Some sessions have no pane attached
///                  (e.g. legacy entries replayed from history before the
///                  field was introduced) so this is `Option`. Serialized
///                  into `acp::SessionInfo._meta.wta.pane_session_id` on
///                  the wire so we don't pollute the standard ACP schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: acp::SessionId,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub pane_session_id: Option<String>,
}

impl SessionInfo {
    /// Convenience constructor for tests and call sites that only have the
    /// mandatory fields. Optional fields default to `None`.
    pub fn new(session_id: acp::SessionId, cwd: PathBuf) -> Self {
        Self {
            session_id,
            cwd,
            title: None,
            updated_at: None,
            pane_session_id: None,
        }
    }

    /// Builder-style setter for `pane_session_id`, useful in tests and at
    /// `new_session` time when the helper hands us a `_meta.wta` payload.
    pub fn with_pane_session_id(mut self, pane_session_id: impl Into<String>) -> Self {
        self.pane_session_id = Some(pane_session_id.into());
        self
    }
}

/// Read/write surface over the live-session set. Both master and helper
/// hold an `Arc<dyn SessionRegistry>` so unit tests can swap in mocks
/// without spinning up a real pipe. In production both sides use
/// `InMemoryRegistry`.
#[async_trait::async_trait]
pub trait SessionRegistry: Send + Sync {
    /// Insert-or-replace the row for `info.session_id`. Idempotent — calling
    /// twice with the same `session_id` keeps only the latest copy.
    async fn upsert(&self, info: SessionInfo);

    /// Remove the row for `sid`. Returns the prior value if any (the master
    /// uses this both for routing teardown and to know what to broadcast
    /// in `session_removed` ext-notifications).
    async fn remove(&self, sid: &acp::SessionId) -> Option<SessionInfo>;

    /// Fetch a clone of the current entry for `sid`. Returns `None` if the
    /// session isn't alive (or hasn't been mirrored yet on the helper side).
    async fn lookup(&self, sid: &acp::SessionId) -> Option<SessionInfo>;

    /// Snapshot the full set. Order is unspecified — callers that need a
    /// stable order should sort by `session_id` themselves. The clone is
    /// cheap because `SessionInfo` is small (`Arc<str>` for the id).
    async fn snapshot(&self) -> Vec<SessionInfo>;
}

/// Production implementation. Uses `tokio::sync::Mutex` for parity with the
/// existing master state; the critical sections are all sync HashMap ops
/// so a future sync-lock conversion is a mechanical swap.
#[derive(Default)]
pub struct InMemoryRegistry {
    inner: Mutex<HashMap<acp::SessionId, SessionInfo>>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<dyn SessionRegistry> {
        Arc::new(Self::new())
    }
}

#[async_trait::async_trait]
impl SessionRegistry for InMemoryRegistry {
    async fn upsert(&self, info: SessionInfo) {
        let mut guard = self.inner.lock().await;
        guard.insert(info.session_id.clone(), info);
    }

    async fn remove(&self, sid: &acp::SessionId) -> Option<SessionInfo> {
        let mut guard = self.inner.lock().await;
        guard.remove(sid)
    }

    async fn lookup(&self, sid: &acp::SessionId) -> Option<SessionInfo> {
        let guard = self.inner.lock().await;
        guard.get(sid).cloned()
    }

    async fn snapshot(&self) -> Vec<SessionInfo> {
        let guard = self.inner.lock().await;
        guard.values().cloned().collect()
    }
}

/// Bulk-load the result of an ACP `session/list` response into a registry
/// and mark the helper as having seen its first authoritative snapshot.
///
/// Semantics: the snapshot is *authoritative* — any row not present in
/// `items` is removed. We achieve this by issuing per-key removes against
/// the current snapshot (so we honor the registry's existing locking
/// surface without adding a `clear()` method just for one bootstrap call
/// site) and then upserting each item from `items`.
///
/// Setting `loaded` to `true` flips the helper from "we haven't heard
/// from master yet, fall back to legacy behavior" to "registry is
/// authoritative". The F2 routing layer reads this flag to avoid
/// misclassifying an actually-Live row as Ended during the startup
/// window between helper boot and the first `session/list` response.
///
/// This is intentionally a free function rather than a method on
/// `SessionRegistry`: bootstrap-vs-incremental is a *caller* concern,
/// not a property of the storage, and keeping the trait minimal keeps
/// the mock surface small for unit tests of higher layers.
pub async fn apply_snapshot(
    reg: &dyn SessionRegistry,
    loaded: &AtomicBool,
    items: impl IntoIterator<Item = SessionInfo>,
) {
    // Drop every row currently in the registry. We snapshot first and
    // then remove by id rather than holding a write lock across the
    // whole reload, because (a) the trait surface only offers per-key
    // mutations, (b) bootstrap snapshots are tiny (<<100 rows) so the
    // double-pass is cheap, and (c) the only concurrent writer at
    // bootstrap is the ext-notification listener, which we *want* to
    // win against this routine — see comment on `alive_loaded` for
    // why we tolerate the small race window.
    for old in reg.snapshot().await {
        reg.remove(&old.session_id).await;
    }
    for item in items {
        reg.upsert(item).await;
    }
    loaded.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, pane: Option<&str>) -> SessionInfo {
        let mut s = SessionInfo::new(acp::SessionId::new(id.to_string()), PathBuf::from("/tmp"));
        if let Some(p) = pane {
            s = s.with_pane_session_id(p.to_string());
        }
        s
    }

    #[tokio::test]
    async fn upsert_then_lookup_returns_clone() {
        let reg = InMemoryRegistry::new();
        let original = info("sess-1", Some("pane-A"));
        reg.upsert(original.clone()).await;
        let found = reg
            .lookup(&acp::SessionId::new("sess-1".to_string()))
            .await
            .expect("session present");
        assert_eq!(found, original);
    }

    #[tokio::test]
    async fn lookup_miss_returns_none() {
        let reg = InMemoryRegistry::new();
        assert!(reg
            .lookup(&acp::SessionId::new("missing".to_string()))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn upsert_is_idempotent_and_replaces() {
        let reg = InMemoryRegistry::new();
        reg.upsert(info("sess-1", Some("pane-A"))).await;
        reg.upsert(info("sess-1", Some("pane-B"))).await;
        let found = reg
            .lookup(&acp::SessionId::new("sess-1".to_string()))
            .await
            .unwrap();
        assert_eq!(found.pane_session_id.as_deref(), Some("pane-B"));
        assert_eq!(reg.snapshot().await.len(), 1, "no duplicate rows");
    }

    #[tokio::test]
    async fn remove_returns_prior_and_subsequent_lookup_is_none() {
        let reg = InMemoryRegistry::new();
        reg.upsert(info("sess-1", Some("pane-A"))).await;
        let removed = reg
            .remove(&acp::SessionId::new("sess-1".to_string()))
            .await
            .expect("entry removed");
        assert_eq!(removed.pane_session_id.as_deref(), Some("pane-A"));
        assert!(reg
            .lookup(&acp::SessionId::new("sess-1".to_string()))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn remove_miss_returns_none() {
        let reg = InMemoryRegistry::new();
        assert!(reg
            .remove(&acp::SessionId::new("nope".to_string()))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn snapshot_contains_all_inserted_rows_in_any_order() {
        let reg = InMemoryRegistry::new();
        reg.upsert(info("a", Some("pa"))).await;
        reg.upsert(info("b", None)).await;
        reg.upsert(info("c", Some("pc"))).await;
        let mut snap = reg.snapshot().await;
        snap.sort_by(|l, r| l.session_id.0.cmp(&r.session_id.0));
        let ids: Vec<&str> = snap.iter().map(|s| &*s.session_id.0).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn shared_constructor_returns_trait_object_that_works() {
        let reg: Arc<dyn SessionRegistry> = InMemoryRegistry::shared();
        reg.upsert(info("sess-1", None)).await;
        assert_eq!(reg.snapshot().await.len(), 1);
    }

    // ── apply_snapshot ──────────────────────────────────────────────

    #[tokio::test]
    async fn apply_snapshot_seeds_empty_registry() {
        let reg = InMemoryRegistry::new();
        let loaded = AtomicBool::new(false);
        apply_snapshot(&reg, &loaded, vec![info("a", Some("pa")), info("b", None)]).await;
        let mut snap = reg.snapshot().await;
        snap.sort_by(|l, r| l.session_id.0.cmp(&r.session_id.0));
        let ids: Vec<&str> = snap.iter().map(|s| &*s.session_id.0).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert!(loaded.load(Ordering::Acquire), "loaded flag flipped");
    }

    #[tokio::test]
    async fn apply_snapshot_drops_rows_absent_from_new_snapshot() {
        let reg = InMemoryRegistry::new();
        let loaded = AtomicBool::new(false);
        reg.upsert(info("stale", Some("pa"))).await;
        reg.upsert(info("keep", Some("pb"))).await;
        apply_snapshot(&reg, &loaded, vec![info("keep", Some("pb")), info("fresh", None)]).await;
        let mut snap = reg.snapshot().await;
        snap.sort_by(|l, r| l.session_id.0.cmp(&r.session_id.0));
        let ids: Vec<&str> = snap.iter().map(|s| &*s.session_id.0).collect();
        assert_eq!(ids, vec!["fresh", "keep"], "stale row evicted");
    }

    #[tokio::test]
    async fn apply_snapshot_replaces_existing_row_contents() {
        let reg = InMemoryRegistry::new();
        let loaded = AtomicBool::new(false);
        reg.upsert(info("sess-1", Some("old-pane"))).await;
        apply_snapshot(&reg, &loaded, vec![info("sess-1", Some("new-pane"))]).await;
        let found = reg
            .lookup(&acp::SessionId::new("sess-1".to_string()))
            .await
            .unwrap();
        assert_eq!(found.pane_session_id.as_deref(), Some("new-pane"));
        assert_eq!(reg.snapshot().await.len(), 1, "no duplicates");
    }

    #[tokio::test]
    async fn apply_snapshot_with_empty_iter_clears_registry() {
        let reg = InMemoryRegistry::new();
        let loaded = AtomicBool::new(false);
        reg.upsert(info("a", None)).await;
        reg.upsert(info("b", None)).await;
        apply_snapshot(&reg, &loaded, std::iter::empty()).await;
        assert!(reg.snapshot().await.is_empty(), "registry cleared");
        assert!(
            loaded.load(Ordering::Acquire),
            "loaded still flips on empty snapshot"
        );
    }

    #[tokio::test]
    async fn apply_snapshot_is_idempotent() {
        let reg = InMemoryRegistry::new();
        let loaded = AtomicBool::new(false);
        let items = vec![info("a", Some("pa")), info("b", None)];
        apply_snapshot(&reg, &loaded, items.clone()).await;
        apply_snapshot(&reg, &loaded, items).await;
        assert_eq!(reg.snapshot().await.len(), 2, "second apply matches first");
    }
}
