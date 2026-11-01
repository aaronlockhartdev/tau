//! The snapshot's `subagents` projection (ticket #221): the parent's child
//! list and each child's `own_state`, rebuilt from the session files alone.
//!
//! A child's true lifecycle state survives a restart in its own session file
//! (the last `subagent` state entry); the supervisor's in-memory registry is
//! empty on a fresh boot. These functions read that ground truth — the disk
//! half of the snapshot — so the GUI's badges reflect the on-disk state after
//! a restart. The `ChildStateRef` mirror is the deserialization target: it
//! keeps the `Done` output out of the record, which the full `ChildState`
//! serde would require.

use super::ContextMode;
use super::states::{StoppedBy, WaitingOn};
use crate::session::{Entry, SessionStore, list_workspace};
use std::path::Path;
use tau_protocol::EntryKind;

/// The parent's children from disk: every on-disk session whose header names
/// `parent` (and is not archived), each carrying its last recorded lifecycle
/// state, projected to the protocol's `SubagentInfo`. A child with no state
/// entry (spawned, then restarted before any rest) reads `idle` — the
/// post-restart ground truth (a `running` turn does not survive a restart;
/// the child process is gone).
#[must_use]
pub fn restore_children_from_disk(cwd: &Path, parent: &str) -> Vec<tau_protocol::SubagentInfo> {
    let mut out: Vec<tau_protocol::SubagentInfo> = list_workspace(cwd)
        .into_iter()
        .filter(|m| !m.archived && m.parent.as_deref() == Some(parent))
        .map(|m| {
            let mut store = SessionStore::for_workspace(cwd, &m.id);
            // Two bounded reads (#302): the spawn record sits at the child's
            // log head, its last state record at the tail — the fleet read
            // never parses a child's whole file.
            let head = store
                .open()
                .and_then(|()| store.entries_range(0, super::restore::SPAWN_HEAD))
                .unwrap_or_default();
            let tail = store
                .entries_tail(super::restore::STATE_TAIL)
                .unwrap_or_default();
            // The child's post-restart state: a `running` record is a
            // restart artifact (the process is gone) and reads back as a
            // parked child (ticket #221's negative bar).
            let state = last_state(&tail).map_or(
                ChildStateRef::Idle {
                    waiting_on: WaitingOn::Parent,
                },
                settled,
            );
            // The spawn record is the child's provenance (ADR-0001):
            // agent type + context mode. A fresh/compacted child writes it at
            // its log's head, a fork child appends it to the copied parent
            // tree, so both windows are searched. Absent (a hand-made file) →
            // the `general`/`fresh` defaults.
            let spawn = head
                .iter()
                .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "spawn")
                .or_else(|| {
                    tail.iter()
                        .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "spawn")
                });
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
                state: state.to_protocol(),
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
/// projected to the protocol's tagged state. `None` when no state was
/// recorded (a spawn with no rest) — the GUI then falls back to the
/// turn-derived `idle`. A `running` record reads back as a parked child.
#[must_use]
pub fn own_state_for(entries: &[Entry]) -> Option<tau_protocol::SubagentState> {
    let state = last_state(entries).map(settled)?;
    Some(state.to_protocol())
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
        .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "state")
        .and_then(|e| serde_json::from_value::<ChildStateRef>(e.payload.clone()).ok())
}

/// The child's persisted state (the `subagent` state entry's shape): a
/// `ChildState` mirror that keeps the done output out of the record —
/// `set_state` writes the discriminant + non-output fields, and the full
/// `ChildState` serde would require the `Done` output.
/// The record has no `other` arm, unlike the wire's decoder: a record that
/// cannot state its detail decodes to nothing and the restore reads no state,
/// because the file was written by this same build — there is no newer name to
/// keep, only a fact the writer failed to record.
#[derive(serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ChildStateRef {
    Running,
    Idle {
        // The target is what `set_state` writes; a frame that omits it
        // states no wait, and the restore declines the record rather than
        // picking a target (the wire decoder's rule for a truncated frame,
        // applied to the record).
        waiting_on: WaitingOn,
    },
    Awaiting,
    Done,
    Failed {
        reason: String,
    },
    Stopped {
        by: StoppedBy,
    },
}

impl ChildStateRef {
    /// The persisted state as the protocol's tagged enum (#346): the record
    /// carries the reason and the stopper, so the restart's badge says the
    /// same thing the live event did. Done's output is the notify record's,
    /// never the state entry's, so it reads back absent.
    #[must_use]
    fn to_protocol(&self) -> tau_protocol::SubagentState {
        match self {
            ChildStateRef::Running => tau_protocol::SubagentState::Running,
            ChildStateRef::Idle { waiting_on } => tau_protocol::SubagentState::Idle {
                waiting_on: (*waiting_on).into(),
            },
            ChildStateRef::Awaiting => tau_protocol::SubagentState::Awaiting,
            ChildStateRef::Done => tau_protocol::SubagentState::Done { output: None },
            ChildStateRef::Failed { reason } => tau_protocol::SubagentState::Failed {
                reason: reason.clone(),
            },
            ChildStateRef::Stopped { by } => tau_protocol::SubagentState::Stopped {
                by: (*by).into(),
                resume_contract: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Entry;

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
            kind: EntryKind::Subagent,
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
        assert_eq!(own.tag(), "done");
        assert!(
            matches!(own, tau_protocol::SubagentState::Done { .. }),
            "the tag carries the state and its fields: {own:?}"
        );
    }

    #[test]
    fn an_idle_child_carries_its_declared_wait() {
        let entries = vec![state_entry(
            "idle",
            &serde_json::json!({ "waiting_on": "user" }),
        )];
        let own = own_state_for(&entries).expect("an idle child has an own_state");
        assert_eq!(own.tag(), "idle");
        assert!(
            matches!(&own, tau_protocol::SubagentState::Idle { waiting_on }
                if waiting_on.as_str() == "user"),
            "an `idle` tag carries the declared wait: {own:?}"
        );
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
            kind: EntryKind::Subagent,
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
        assert!(
            matches!(&own, tau_protocol::SubagentState::Idle { waiting_on }
                if waiting_on.as_str() == "parent"),
            "a `running` record reads back as parked on its parent, never live: {own:?}"
        );
    }

    #[test]
    fn an_awaiting_child_reads_as_its_own_tag() {
        let entries = vec![state_entry("awaiting", &serde_json::json!({}))];
        let own = own_state_for(&entries).expect("awaiting yields an own_state");
        assert!(
            matches!(own, tau_protocol::SubagentState::Awaiting),
            "the `awaiting` tag says a child is asking with no field: {own:?}"
        );
    }

    #[test]
    fn a_recorded_failure_and_stop_read_back_their_detail() {
        let failed = own_state_for(&[state_entry(
            "failed",
            &serde_json::json!({ "reason": "the provider refused" }),
        )])
        .expect("a recorded failure states its reason");
        assert!(
            matches!(&failed, tau_protocol::SubagentState::Failed { reason }
                if reason == "the provider refused"),
            "the reason rides the restart's tag: {failed:?}"
        );
        let stopped =
            own_state_for(&[state_entry("stopped", &serde_json::json!({ "by": "user" }))])
                .expect("a recorded stop names its storer");
        assert!(
            matches!(&stopped, tau_protocol::SubagentState::Stopped { by, .. }
                if *by == tau_protocol::StoppedBy::User),
            "the storer rides the restart's tag: {stopped:?}"
        );
    }

    #[test]
    fn a_truncated_record_is_no_state_not_a_defaulted_one() {
        // A record that cannot state its fact is declined: the badge falls to
        // the no-state path, rather than an empty reason, a parent stopper, or
        // a picked wait target (the wire decoder's rule, applied to the record).
        for state in ["failed", "stopped", "idle"] {
            let entries = vec![state_entry(state, &serde_json::json!({}))];
            assert!(
                own_state_for(&entries).is_none(),
                "a `{state}` record with no detail states no lifecycle fact"
            );
        }
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
                    .append(
                        EntryKind::Subagent,
                        state_entry(state, &extra).payload,
                        None,
                    )
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
        assert_eq!(c1.state.tag(), "done");
        assert!(
            matches!(c1.state, tau_protocol::SubagentState::Done { .. }),
            "a restored `done` row carries its tag: {:?}",
            c1.state
        );
        let c2 = children
            .iter()
            .find(|c| c.child == "c2")
            .expect("c2 is listed");
        assert_eq!(c2.state.tag(), "idle");
        assert!(
            matches!(&c2.state, tau_protocol::SubagentState::Idle { waiting_on }
                if waiting_on.as_str() == "user"),
            "a restored `idle` row carries the declared wait: {:?}",
            c2.state
        );
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
            .append(
                EntryKind::Subagent,
                serde_json::json!({ "event": "spawn" }),
                None,
            )
            .unwrap();
        let children = restore_children_from_disk(tmp.path(), "p");
        assert_eq!(children.len(), 1);
        assert!(
            matches!(&children[0].state, tau_protocol::SubagentState::Idle { waiting_on }
                if waiting_on.as_str() == "parent"),
            "a child with no state reads as parked on its parent: {:?}",
            children[0].state
        );
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
