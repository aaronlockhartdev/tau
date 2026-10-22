//! The restart restore for a session's sub-agent children (ticket #221).
//!
//! A child's true lifecycle state survives a restart in its own session
//! file (the last `subagent` state entry), and the child→parent link in
//! the file header — not in the supervisor's in-memory registry, which
//! is empty on a fresh boot. This rebuilds the parent's child list from
//! those records, so the snapshot's `subagents` (and the GUI's badges)
//! reflect the on-disk ground truth after a restart.
use super::states::WaitingOn;
use super::{ContextMode, KIND_SUBAGENT};
use crate::session::{Entry, SessionStore, list_workspace};
use std::path::Path;

/// The parent's children from disk: every on-disk session whose header
/// names `parent` (and is not archived), each carrying its last recorded
/// lifecycle state, projected to the protocol's `SubagentInfo`. A child
/// with no state entry (spawned, then restarted before any rest) reads
/// `idle` — the post-restart ground truth (a `running` turn does not
/// survive a restart; the child process is gone).
#[must_use]
pub fn restore_children_from_disk(cwd: &Path, parent: &str) -> Vec<tau_protocol::SubagentInfo> {
    let mut out: Vec<tau_protocol::SubagentInfo> = list_workspace(cwd)
        .into_iter()
        .filter(|m| !m.archived && m.parent.as_deref() == Some(parent))
        .map(|m| {
            let mut store = SessionStore::for_workspace(cwd, &m.id);
            let entries = store
                .open()
                .and_then(|()| store.entries_range(0, usize::MAX))
                .unwrap_or_default();
            // The child's post-restart state: a `running` record is a
            // restart artifact (the process is gone) and reads back as a
            // parked child (ticket #221's negative bar).
            let state = last_state(&entries).map_or(
                ChildStateRef::Idle {
                    waiting_on: WaitingOn::Parent,
                },
                settled,
            );
            // waiting_on mirrors the live projection (inspect.rs): `idle`
            // carries its declared target, `awaiting` is a user question.
            let waiting_on = match state {
                ChildStateRef::Idle { waiting_on } => Some(waiting_on),
                ChildStateRef::Awaiting => Some(WaitingOn::User),
                _ => None,
            };
            // The spawn record is the child's provenance (ADR-0001):
            // agent type + context mode. Absent (a hand-made file) →
            // the `general`/`fresh` defaults.
            let spawn = entries
                .iter()
                .find(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == "spawn");
            let agent_type = spawn
                .and_then(|e| e.payload["type"].as_str())
                .unwrap_or("general")
                .to_owned();
            let context_mode = spawn
                .and_then(|e| e.payload["context_mode"].as_str())
                .and_then(|s| match s {
                    "compacted" => Some(ContextMode::Compacted),
                    "fork" => Some(ContextMode::Fork),
                    _ => None,
                })
                .unwrap_or(ContextMode::Fresh);
            tau_protocol::SubagentInfo {
                // No live bridge to address a dead child by handle: the
                // child's session id is the stable key.
                handle: m.id.clone(),
                child: m.id.clone(),
                agent_type,
                context_mode: match context_mode {
                    ContextMode::Fresh => tau_protocol::ContextMode::Fresh,
                    ContextMode::Compacted => tau_protocol::ContextMode::Compacted,
                    ContextMode::Fork => tau_protocol::ContextMode::Fork,
                },
                state: state.kind().to_owned(),
                waiting_on: waiting_on.map(|w| w.as_str().to_owned()),
                last_message: None,
                usage: None,
                created: m.created,
                resume_contract: None,
            }
        })
        .collect();
    // Spawn order (#226 variant): the transcript's Nth spawn point claims
    // the Nth list entry, so the projection sorts by the child session's
    // created timestamp, the handle as a stable tie-break.
    out.sort_by_key(|i: &tau_protocol::SubagentInfo| (i.created, i.child.clone()));
    out
}

/// A child session's own lifecycle state for its snapshot's `own_state`
/// (ticket #221): the last `subagent` state entry in the child's own file,
/// projected to the protocol's `OwnState`. `None` when no state was
/// recorded (a spawn with no rest) — the GUI then falls back to the
/// turn-derived `idle`. A `running` record reads back as a parked child.
#[must_use]
pub fn own_state_for(entries: &[Entry]) -> Option<tau_protocol::snapshot::OwnState> {
    let state = last_state(entries).map(settled)?;
    Some(tau_protocol::snapshot::OwnState {
        state: state.kind().to_owned(),
        waiting_on: match state {
            ChildStateRef::Idle { waiting_on } => Some(waiting_on.as_str().to_owned()),
            ChildStateRef::Awaiting => Some(WaitingOn::User.as_str().to_owned()),
            _ => None,
        },
    })
}

/// A `running` state is a restart artifact: the child process is gone, so
/// a record that ends in `running` (no rest was observed before the crash)
/// reads back as a parked child, never a live turn (ticket #221: the
/// reconcile must not resurrect a `running` that no longer exists).
fn settled(state: ChildStateRef) -> ChildStateRef {
    match state {
        ChildStateRef::Running => ChildStateRef::Idle {
            waiting_on: WaitingOn::Parent,
        },
        other => other,
    }
}

/// The child's last lifecycle state: the last `subagent` state entry,
/// deserialized with the exact serde the live path writes (child.rs
/// `set_state`), so a restart reads back what a run recorded.
fn last_state(entries: &[Entry]) -> Option<ChildStateRef> {
    entries
        .iter()
        .rev()
        .find(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == "state")
        .and_then(|e| serde_json::from_value::<ChildStateRef>(e.payload.clone()).ok())
}

/// The child's persisted state (the `subagent` state entry's shape): a
/// `ChildState` mirror that keeps the done output out of the record —
/// `set_state` writes only the discriminant + non-output fields, and the
/// full `ChildState` serde would require the `Done` output. The restore
/// only needs the state kind (the `reason`/`by` payload fields are not
/// projected, so they are dropped from the mirror).
#[derive(serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ChildStateRef {
    Running,
    Idle {
        // A state entry written without a target (none are, today) still
        // decodes: the declared wait defaults to the delegating parent.
        #[serde(default = "waiting_parent")]
        waiting_on: WaitingOn,
    },
    Awaiting,
    Done,
    Failed,
    Stopped,
}

fn waiting_parent() -> WaitingOn {
    WaitingOn::Parent
}

impl ChildStateRef {
    /// The state's kind string (the badge label): mirrors `ChildState::kind`.
    #[must_use]
    fn kind(&self) -> &'static str {
        match self {
            ChildStateRef::Running => "running",
            ChildStateRef::Idle { .. } => "idle",
            ChildStateRef::Awaiting => "awaiting",
            ChildStateRef::Done => "done",
            ChildStateRef::Failed => "failed",
            ChildStateRef::Stopped => "stopped",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `subagent` state entry in the exact shape the live path writes
    /// (child.rs `set_state`): the `event` tag plus the `state` tag.
    fn state_entry(state: &str, extra: &serde_json::Value) -> Entry {
        let mut payload = serde_json::json!({ "event": "state", "state": state });
        if let (Some(target), Some(obj)) = (extra.as_object(), payload.as_object_mut()) {
            for (k, v) in target {
                obj.insert(k.clone(), v.clone());
            }
        }
        Entry {
            id: "e1".into(),
            parent: None,
            timestamp: 0,
            kind: KIND_SUBAGENT.into(),
            payload,
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        }
    }

    #[test]
    fn own_state_reads_the_last_recorded_state() {
        let entries = vec![
            state_entry("running", &serde_json::json!({})),
            state_entry("idle", &serde_json::json!({ "waiting_on": "user" })),
            state_entry("done", &serde_json::json!({})),
        ];
        let own = own_state_for(&entries).expect("a state entry yields an own_state");
        assert_eq!(own.state, "done");
        assert_eq!(own.waiting_on, None);
    }

    #[test]
    fn an_idle_child_carries_its_declared_wait() {
        let entries = vec![state_entry(
            "idle",
            &serde_json::json!({ "waiting_on": "user" }),
        )];
        let own = own_state_for(&entries).expect("an idle child has an own_state");
        assert_eq!(own.state, "idle");
        assert_eq!(own.waiting_on.as_deref(), Some("user"));
    }

    #[test]
    fn no_state_entry_is_none_not_a_guess() {
        // A spawn with no rest: the child never recorded a state, so the
        // GUI falls back to the turn-derived idle. A guess here would
        // fabricate a lifecycle the child never had.
        let spawn = Entry {
            id: "e1".into(),
            parent: None,
            timestamp: 0,
            kind: KIND_SUBAGENT.into(),
            payload: serde_json::json!({ "event": "spawn" }),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        };
        assert!(own_state_for(&[spawn]).is_none());
    }

    #[test]
    fn a_running_record_settles_to_a_parked_child() {
        // Ticket #221's negative bar: a `running` record is a restart
        // artifact (the process is gone) — it reads back as a parked child,
        // never a resurrected live turn.
        let entries = vec![state_entry("running", &serde_json::json!({}))];
        let own = own_state_for(&entries).expect("a running record settles");
        assert_eq!(own.state, "idle");
        assert_eq!(own.waiting_on.as_deref(), Some("parent"));
    }

    #[test]
    fn an_awaiting_child_reads_as_a_user_question() {
        let entries = vec![state_entry("awaiting", &serde_json::json!({}))];
        let own = own_state_for(&entries).expect("awaiting yields an own_state");
        assert_eq!(own.state, "awaiting");
        assert_eq!(own.waiting_on.as_deref(), Some("user"));
    }

    #[test]
    fn restore_lists_the_parents_children_with_their_states() {
        // A parent, two children on disk: one done, one parked on the user.
        // The restore rebuilds the parent's child list from the files.
        let tmp = tempfile::tempdir().unwrap();
        for (id, parent, state) in [
            ("p", None, None),
            ("c1", Some("p"), Some(("done", serde_json::json!({})))),
            (
                "c2",
                Some("p"),
                Some(("idle", serde_json::json!({ "waiting_on": "user" }))),
            ),
        ] {
            let mut store = SessionStore::for_workspace(tmp.path(), id);
            store.create().unwrap();
            if let Some(parent) = parent {
                store.set_parent(parent).unwrap();
            }
            if let Some((state, extra)) = state {
                store
                    .append(KIND_SUBAGENT, state_entry(state, &extra).payload, None)
                    .unwrap();
            }
        }
        // A sibling of the parent (not a child) is not listed.
        let mut sibling = SessionStore::for_workspace(tmp.path(), "sib");
        sibling.create().unwrap();

        let children = restore_children_from_disk(tmp.path(), "p");
        assert_eq!(children.len(), 2, "only the parent's children list");
        let c1 = children
            .iter()
            .find(|c| c.child == "c1")
            .expect("c1 is listed");
        assert_eq!(c1.state, "done");
        assert_eq!(c1.waiting_on, None);
        let c2 = children
            .iter()
            .find(|c| c.child == "c2")
            .expect("c2 is listed");
        assert_eq!(c2.state, "idle");
        assert_eq!(c2.waiting_on.as_deref(), Some("user"));
        // The handle is the child's session id (no live bridge to address a
        // dead child by handle).
        assert_eq!(c1.handle, "c1");
    }

    #[test]
    fn a_child_with_no_state_reads_idle_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(tmp.path(), "p");
        store.create().unwrap();
        let mut child = SessionStore::for_workspace(tmp.path(), "c");
        child.create().unwrap();
        child.set_parent("p").unwrap();
        // A spawn record, no state: the post-restart ground truth is idle.
        child
            .append(KIND_SUBAGENT, serde_json::json!({ "event": "spawn" }), None)
            .unwrap();
        let children = restore_children_from_disk(tmp.path(), "p");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].state, "idle");
        assert_eq!(children[0].waiting_on.as_deref(), Some("parent"));
    }
    #[test]
    fn restore_lists_children_in_created_order() {
        // #226 variant: the restore's projection must be in spawn order,
        // sorted by the child sessions' created timestamps — `list_workspace`
        // yields an arbitrary (directory) order.
        let tmp = tempfile::tempdir().unwrap();
        let mut parent = SessionStore::for_workspace(tmp.path(), "p");
        parent.create().unwrap();
        for id in ["c1", "c2"] {
            let mut store = SessionStore::for_workspace(tmp.path(), id);
            store.create().unwrap();
            store.set_parent("p").unwrap();
        }
        // c2's header gets the EARLIER created timestamp: despite c1's id
        // sorting first, the restore must list c2 first.
        let p2 = tmp.path().join(".tau").join("sessions").join("c2.jsonl");
        let raw = std::fs::read_to_string(&p2).unwrap();
        let mut lines: Vec<String> = raw.lines().map(std::borrow::ToOwned::to_owned).collect();
        let mut h: serde_json::Value = serde_json::from_str(lines[0].as_str()).unwrap();
        h["created"] = serde_json::json!(1);
        lines[0] = serde_json::to_string(&h).unwrap();
        std::fs::write(&p2, lines.join("\n")).unwrap();
        let children = restore_children_from_disk(tmp.path(), "p");
        let order: Vec<String> = children.iter().map(|c| c.child.clone()).collect();
        assert_eq!(
            order,
            vec!["c2", "c1"],
            "the restore is not in created order"
        );
    }
}
