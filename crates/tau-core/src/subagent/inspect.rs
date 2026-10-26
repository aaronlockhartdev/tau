use super::{ChildState, SessionStore, SubagentInfo, Supervisor, TurnUsage, WaitingOn};
use crate::om;
use crate::om_integration::{self, OmState};
use crate::session::Entry;
use tau_protocol::EntryKind;
use tau_protocol::payload::{ActivityEntry, ActivityGroup, ChildActivity};

/// The activity window's tail cap: a 10k-entry child must stay cheap
/// (#203) — the cap applies unconditionally, including on the cursor-miss
/// and empty-record fallbacks (a miss otherwise falls back to the entire
/// branch).
const ACTIVITY_TAIL: usize = 20;
/// The observation tail's group cap (the last N groups).
const ACTIVITY_GROUPS: usize = 4;
/// Per-item text truncation for the display projection.
const ACTIVITY_TEXT_LIMIT: usize = 400;

impl Supervisor {
    /// All registered children (the snapshot's live-state handles).
    pub fn handles(&self) -> Vec<String> {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
            .keys()
            .cloned()
            .collect()
    }

    /// A child's structured inspection (the `subagent_state` tool and the
    /// protocol's `subagent_state` command).
    pub fn state_info(&self, name_or_id: &str) -> Option<SubagentInfo> {
        let child = self.resolve(name_or_id)?.clone();
        let (state, waiting_on) = match child.state() {
            ChildState::Idle { waiting_on } => (ChildState::Idle { waiting_on }, Some(waiting_on)),
            // The `QUESTION:` marker: the child is asking the user.
            ChildState::Awaiting => (ChildState::Awaiting, Some(WaitingOn::User)),
            other => (other, None),
        };
        let usage = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|()| store.leaf().ok().flatten())
                .and_then(|leaf| {
                    let entries = store.entries_range(0, usize::MAX).ok()?;
                    let branch = om_integration::branch_entries(&entries, Some(&leaf.id));
                    branch
                        .iter()
                        .rev()
                        .find(|e| e.kind == EntryKind::Assistant)
                        .and_then(|e| e.payload.get("usage"))
                        .and_then(|u| serde_json::from_value::<TurnUsage>(u.clone()).ok())
                })
        };
        // The child's plan file (spec §5.3): its open-step resume contract.
        let resume_contract =
            std::fs::read_to_string(crate::plan::path(&self.cwd, &child.session_id))
                .ok()
                .and_then(|raw| crate::plan::resume_contract(&crate::plan::parse(&raw)));
        Some(SubagentInfo {
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            agent_type: child.agent_type.clone(),
            context_mode: child.context_mode,
            state,
            waiting_on,
            last_message: child
                .last_message
                .lock()
                .expect("child last message: no panic while the lock is held")
                .clone(),
            created: child.agent.store_created(),
            usage,
            resume_contract,
        })
    }

    /// The child's activity window (the `subagent_activity` command, #203):
    /// a handle-scoped read of the free function below.
    pub fn activity_window(&self, name_or_id: &str) -> Option<ChildActivity> {
        let child = self.resolve(name_or_id)?.clone();
        read_activity(&self.cwd, &child.session_id, &self.om)
    }
}

/// One child's activity window: the tail of its observation log plus its
/// un-observed raw tail, assembled on a **throwaway** `OmState` built from
/// the child's record (research `subagent-transcript-sourcing.md`): the
/// seam is pure, and the one-shot flips in `assemble_context` land on the
/// throwaway, never the child's live state. Never mutates the child's
/// persistent OM state.
pub(crate) fn read_activity(
    cwd: &std::path::Path,
    session_id: &str,
    om: &crate::config::Om,
) -> Option<ChildActivity> {
    let mut store = SessionStore::for_workspace(cwd, session_id);
    store.open().ok()?;
    // The entries are read BEFORE the record (research §3): a cursor newer
    // than the entry list misses the position lookup and the raw window
    // falls back to the entire branch. Reading the list first biases the
    // mid-turn skew toward the harmless direction (a re-included fact).
    let entries = store.entries_range(0, usize::MAX).ok()?;
    let leaf = store.leaf().ok().and_then(|l| l)?;
    let record = OmState::load_record(&mut store).ok()?;
    // A cursor that is not on the branch means the entry list is older
    // than the record (or the record is corrupt): the window is the whole
    // branch, tail-capped. A missing cursor (young child, empty record)
    // is by-design, not degraded.
    let degraded = record
        .cursor
        .as_ref()
        .is_some_and(|c| !entries.iter().any(|e| e.id == c.entry_id));
    // The observation groups are the child's actual log text (boundary
    // markers stripped) — the same source `assemble_context` projects them
    // from, so the GUI's window stays byte-faithful to what the child's
    // model sees. `assemble_context` itself is NOT run here: its one-shot
    // flips must stay on the child's live state, and nothing downstream
    // needs them.
    let groups = om::parse_observation_groups(&record.agent_observations());
    let state = OmState::from_config(om, record);
    let observations: Vec<ActivityGroup> = groups
        .iter()
        .skip(groups.len().saturating_sub(ACTIVITY_GROUPS))
        .map(|g| ActivityGroup {
            id: g.id.clone(),
            range: g.range.clone(),
            kind: g.kind.clone(),
            content: truncate_at(&g.content, ACTIVITY_TEXT_LIMIT),
        })
        .collect();
    let window = state.raw_window_from(&entries, Some(&leaf.id));
    // The window is in branch order (newest last): keep the last N.
    let live: Vec<ActivityEntry> = window
        .iter()
        .rev()
        .take(ACTIVITY_TAIL)
        .map(project_entry)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(ChildActivity {
        observations,
        live,
        degraded,
    })
}

/// The raw tail's compact projection: role, the tool's name and verdict,
/// and a truncated text slice — the GUI renders exactly this.
fn project_entry(entry: &Entry) -> ActivityEntry {
    match &entry.kind {
        EntryKind::Tool => {
            let text = entry
                .payload
                .get("output")
                .or_else(|| entry.payload.get("args"))
                .map(ToString::to_string)
                .unwrap_or_default();
            ActivityEntry {
                role: "tool".to_owned(),
                tool: entry
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                status: entry
                    .payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                text: truncate_at(&text, ACTIVITY_TEXT_LIMIT),
            }
        }
        other => {
            let text = entry
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            ActivityEntry {
                role: other.as_str().to_owned(),
                tool: None,
                status: None,
                text: truncate_at(&text, ACTIVITY_TEXT_LIMIT),
            }
        }
    }
}

/// A char-boundary-safe, line-boundary truncation (#219): over-limit text is
/// cut to the last complete line that fits, with an ellipsis appended. A
/// single line longer than the whole limit yields an empty prefix + ellipsis
/// (the oversized-line case).
fn truncate_at(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= limit {
        return trimmed.to_owned();
    }
    let mut out = trimmed.to_owned();
    let mut count = 0usize;
    for (i, _) in out.char_indices() {
        count += 1;
        if count > limit {
            let prefix = out.get(..i).expect("char_indices yields a boundary");
            let cut = prefix.rfind('\n').map_or(0, |j| j + 1);
            out.truncate(cut);
            let _ = out.pop();
            out.push('…');
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Entry;
    use crate::subagent::testkit::harness;

    /// A user entry, an assistant entry, and a settled tool entry: the
    /// three raw kinds the projection distinguishes.
    fn seed_child_session(dir: &std::path::Path, session: &str) -> Entry {
        let mut store = SessionStore::for_workspace(dir, session);
        store.create().unwrap();
        let user = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "work the thing" }),
                None,
            )
            .unwrap();
        let assistant = store
            .append(
                EntryKind::Assistant,
                serde_json::json!({ "text": "on it" }),
                Some(&user.id),
            )
            .unwrap();
        store
            .append(
                EntryKind::Tool,
                serde_json::json!({
                    "call_id": "c1",
                    "name": "bash",
                    "args": { "command": "ls" },
                    "output": "ok",
                    "status": "success",
                }),
                Some(&assistant.id),
            )
            .unwrap()
    }

    #[test]
    fn the_window_projects_the_raw_tail_and_is_not_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let (sup, _bridge, parent) = harness(
            tmp.path(),
            vec![],
            vec![],
            crate::subagent::SubAgents::default(),
        );
        seed_child_session(tmp.path(), "child-1");
        // The supervisor's registry is empty here, so the read goes through
        // the free function the method wraps — same code path.
        drop(parent);
        let activity = read_activity(tmp.path(), "child-1", &crate::config::Om::default())
            .expect("a created session yields a window");
        assert!(
            !activity.degraded,
            "no cursor: a young child is not degraded"
        );
        assert!(activity.observations.is_empty());
        assert_eq!(activity.live.len(), 3);
        // Newest first is not promised — the tail is the newest entries in
        // branch order.
        assert_eq!(activity.live[0].role, "user");
        assert_eq!(activity.live[0].text, "work the thing");
        assert_eq!(activity.live[1].role, "assistant");
        assert_eq!(activity.live[1].text, "on it");
        assert_eq!(activity.live[2].role, "tool");
        assert_eq!(activity.live[2].tool.as_deref(), Some("bash"));
        assert_eq!(activity.live[2].status.as_deref(), Some("success"));
        assert_eq!(activity.live[2].text, "\"ok\"");
        // The method resolves through the registry: an unknown handle is
        // None, not a panic.
        assert!(sup.activity_window("nope").is_none());
    }

    #[test]
    fn a_missing_session_is_none_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_activity(tmp.path(), "ghost", &crate::config::Om::default()).is_none());
    }

    #[test]
    fn a_cursor_miss_degrades_and_caps_the_whole_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(tmp.path(), "child-2");
        store.create().unwrap();
        let one = store
            .append(EntryKind::User, serde_json::json!({ "text": "one" }), None)
            .unwrap();
        let two = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "two" }),
                Some(&one.id),
            )
            .unwrap();
        // The record's cursor points at an entry the branch does not carry
        // (the skew direction: the record read is newer than the entry
        // list). The window falls back to the whole branch — and stays
        // capped. The log's groups still project.
        let record = crate::om::OmRecord {
            cursor: Some(crate::om::Cursor {
                entry_id: "0000000000000000".into(),
                timestamp: 0,
            }),
            active_observations: "<observation-group id=\"1111111111111111\" range=\"a:b\">\n* 🔴 (14:30) a fact\n</observation-group>"
                .into(),
            ..Default::default()
        };
        let om_entry = store
            .append(
                EntryKind::Om,
                serde_json::to_value(&record).unwrap(),
                Some(&two.id),
            )
            .unwrap();
        // A raw entry after the `om` record: the branch's leaf is past the
        // record, so `load_record` walks to it and finds the record
        // (the degraded path needs a leaf newer than the record).
        store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "three" }),
                Some(&om_entry.id),
            )
            .unwrap();
        let activity = read_activity(tmp.path(), "child-2", &crate::config::Om::default())
            .expect("a created session yields a window");
        assert!(activity.degraded, "a cursor off the branch degrades");
        assert_eq!(activity.live.len(), 3);
        assert_eq!(activity.live[0].text, "one");
        assert_eq!(activity.live[2].text, "three");
        assert_eq!(activity.observations.len(), 1);
        assert_eq!(activity.observations[0].id, "1111111111111111");
        assert_eq!(activity.observations[0].content, "* 🔴 (14:30) a fact");
    }

    #[test]
    fn the_caps_bound_a_long_child() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(tmp.path(), "child-3");
        store.create().unwrap();
        // 25 raw entries: the window is the newest ACTIVITY_TAIL of them,
        // so it starts at entry {total - T}.
        let total = ACTIVITY_TAIL + 5;
        let mut parent_id: Option<String> = None;
        for i in 0..total {
            let e = store
                .append(
                    EntryKind::User,
                    serde_json::json!({ "text": format!("entry {i}") }),
                    parent_id.as_deref(),
                )
                .unwrap();
            parent_id = Some(e.id);
        }
        // Six groups: the newest four survive.
        let record = crate::om::OmRecord {
            active_observations: (0..6)
                .map(|i| format!(
                    "<observation-group id=\"{i:016x}\" range=\"{i:08x}:{i}\">\n* 🔴 (14:30) g{i}\n</observation-group>",
                ))
                .collect::<Vec<_>>()
                .join("\n"),
            ..Default::default()
        };
        store
            .append(
                EntryKind::Om,
                serde_json::to_value(&record).unwrap(),
                parent_id.as_deref(),
            )
            .unwrap();
        let activity = read_activity(tmp.path(), "child-3", &crate::config::Om::default())
            .expect("a created session yields a window");
        assert_eq!(activity.live.len(), ACTIVITY_TAIL);
        // The window is the newest ACTIVITY_TAIL entries of the
        // {total}-entry branch (the `om` record is not raw).
        assert_eq!(
            activity.live[0].text,
            format!("entry {}", total - ACTIVITY_TAIL)
        );
        assert_eq!(activity.observations.len(), ACTIVITY_GROUPS);
        // Newest group last.
        assert_eq!(activity.observations[0].content, "* 🔴 (14:30) g2");
        assert_eq!(
            activity.observations[ACTIVITY_GROUPS - 1].content,
            "* 🔴 (14:30) g5"
        );
    }

    #[test]
    fn long_text_is_truncated_with_an_ellipsis() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(tmp.path(), "child-4");
        store.create().unwrap();
        store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "x".repeat(ACTIVITY_TEXT_LIMIT + 10) }),
                None,
            )
            .unwrap();
        let activity = read_activity(tmp.path(), "child-4", &crate::config::Om::default()).unwrap();
        assert_eq!(activity.live.len(), 1);
        let text = &activity.live[0].text;
        assert!(text.ends_with('…'));
        assert!(text.len() <= ACTIVITY_TEXT_LIMIT + 3);
    }

    #[test]
    fn truncated_text_stops_at_a_line_boundary() {
        // #219: the cut lands on a line boundary — the last visible line is
        // complete, and the ellipsis follows on its own line.
        let big = (0..100)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(big.len() > ACTIVITY_TEXT_LIMIT);
        let tmp = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(tmp.path(), "child-5");
        store.create().unwrap();
        store
            .append(EntryKind::User, serde_json::json!({ "text": big }), None)
            .unwrap();
        let activity = read_activity(tmp.path(), "child-5", &crate::config::Om::default()).unwrap();
        let text = &activity.live[0].text;
        assert!(text.ends_with('…'), "got: {text:?}");
        let body = text.strip_suffix('…').expect("the ellipsis");
        assert!(!body.ends_with('\n'));
        let last = body.lines().last().expect("a visible line");
        let n: usize = last.trim_start_matches("line ").parse().unwrap();
        assert_eq!(
            last,
            format!("line {n:03}"),
            "last visible line must be complete"
        );
    }
}
