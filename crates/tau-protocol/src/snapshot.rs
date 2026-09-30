//! The snapshot projection (spec §8, ADR-0006): the ephemeral state the GUI
//! rebuilds from — session metadata + entry *metadata* (no payloads) + the
//! bounded OM observation log + live state + a durable cursor. Built on
//! demand by the core (app launch, tab open, reconnect); never a file.

use crate::MessageLane;
use crate::payload::Task;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A workspace as identity (spec §8: no local paths as identity; `cwd` is
/// the on-host root the tools resolve relative paths against, §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub cwd: String,
}

/// Session metadata (the snapshot's (1) piece, spec §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    /// The owning workspace's id.
    pub workspace: String,
    /// The display name (the file header's title; adjective-noun for fresh
    /// top-level sessions, "Sub-agent: …" for children).
    pub title: Option<String>,
    /// The creator session (a sub-agent's parent); absent for top-level.
    pub parent: Option<String>,
    /// The header's created timestamp (epoch ms).
    pub created: u64,
    /// The active branch's leaf entry id.
    pub leaf: Option<String>,
    pub model: Option<String>,
    /// The last assistant entry's usage.
    pub usage: Option<crate::Usage>,
    /// The session is archived (ADR-0005): its file lives in the
    /// workspace's `archive/` dir; the GUI's archive folder lists it.
    #[serde(default)]
    pub archived: bool,
}

/// One entry's metadata: everything the entry tree needs, no payload
/// (spec §8: the snapshot carries no entry payloads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryMeta {
    pub id: String,
    pub parent: Option<String>,
    pub kind: String,
    pub timestamp: u64,
    /// The serialized line size in bytes (the GUI's virtualization sizes).
    pub size: u64,
    /// The first line of the entry's text, truncated (the tree's preview).
    pub preview: String,
    /// Set on compaction records (spec §3).
    pub first_kept: Option<String>,
    /// `Ok` is omitted on the wire — it is the near-universal case and the
    /// type's default; the spec's field stays present via the type.
    #[serde(default, skip_serializing_if = "is_ok_status")]
    pub status: EntryStatus,
}
/// Per-entry status (spec §8 entry metadata): in the v0 format the only
/// per-entry state is a cut partial — an assistant entry recorded as
/// `interrupted` (spec §6/§7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    #[default]
    Ok,
    Interrupted,
}

fn is_ok_status(s: &EntryStatus) -> bool {
    *s == EntryStatus::Ok
}

/// A paged-read window (spec §8: `entries {range}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRange {
    pub start: usize,
    pub count: usize,
}
/// A sidecar-blob pointer (ADR-0005 hardening 2): the payload lives in a
/// zstd-compressed file, the entry carries this. Kept protocol-owned so the
/// page-read types never import core types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub id: String,
    pub size: u64,
    pub hash: String,
}

/// One entry for a page read — unlike `EntryMeta`, the payload is included
/// (the GUI renders the visible page's content; blobs stream on demand).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewEntry {
    pub id: String,
    pub parent: Option<String>,
    pub kind: String,
    pub timestamp: u64,
    pub payload: Value,
    /// A sidecar pointer (ADR-0005) when the payload lives out-of-band.
    pub blob: Option<BlobRef>,
    pub first_kept: Option<String>,
}

/// A message sitting on a lane, as the GUI's vertical queue shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedItem {
    pub text: String,
    pub lane: MessageLane,
    /// Child provenance (a sub-agent's report, ticket #23): `None` for the
    /// user's own messages.
    #[serde(default)]
    pub source: Option<String>,
}

/// The session's turn state (spec §8 live state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Idle,
    Running,
}

/// The live, non-persisted part of the snapshot (spec §8 (4)): the pending
/// lane, the turn state, and the sub-agent set (empty until #23 wires it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveState {
    pub queue: Vec<QueuedItem>,
    pub turn: TurnState,
    /// The session's children (structured state — the GUI's sub-agent
    /// panel); empty for child sessions (the depth cap is structural).
    pub subagents: Vec<crate::SubagentInfo>,
    /// The session's tasks (per-session store, spec §5.3) — the GUI's
    /// tasks panel.
    pub tasks: Vec<Task>,
}

/// The session's OM gauge (ticket #22): observation tokens against the
/// session's configured Reflector threshold, plus the unobserved pending
/// tokens. The status bar renders the ratio when idle and the activity
/// (the `om_status` event) while a run is in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmSnapshot {
    pub observation_tokens: u32,
    pub pending_tokens: u32,
    /// The configured Reflector threshold (the gauge's denominator).
    pub reflector_threshold: u32,
}

/// The ephemeral snapshot (spec §8): metadata skeleton + bounded OM + live
/// state + cursor. `om` is the session's current OM gauge (ticket #22).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub workspace: Workspace,
    pub session: SessionMeta,
    pub entries: Vec<EntryMeta>,
    pub om: OmSnapshot,
    pub live: LiveState,
    /// The last entry id: a durable cursor for `entries-since` reads.
    pub cursor: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The snapshot's size contract (spec §8: no payloads, < 2 MB) rides on
    /// this: the near-universal `Ok` status is omitted on the wire, so the
    /// 10k-entry skeleton stays a skeleton.
    #[test]
    fn entry_status_wire_contract() {
        let meta = EntryMeta {
            id: "e1".into(),
            parent: None,
            kind: "message".into(),
            timestamp: 1,
            size: 10,
            preview: "hi".into(),
            first_kept: None,
            status: EntryStatus::Ok,
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("status").is_none(), "Ok must be off the wire");

        let interrupted = EntryMeta {
            status: EntryStatus::Interrupted,
            ..meta.clone()
        };
        let v = serde_json::to_value(&interrupted).unwrap();
        assert_eq!(v["status"], "interrupted");

        // And both decode back: a missing field lands on the default.
        let back: EntryMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back.status, EntryStatus::Interrupted);
        let v = serde_json::to_value(&meta).unwrap();
        let back: EntryMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back, meta);
    }

    /// Archived is a spec-defaulted flag (ADR-0005): session files written
    /// before the flag existed must decode as unarchived, not fail.
    #[test]
    fn session_meta_archived_defaults_false() {
        let meta: SessionMeta =
            serde_json::from_str(r#"{"id":"s1","workspace":"w1","created":1}"#).unwrap();
        assert!(!meta.archived);
        assert!(meta.title.is_none());
        assert!(meta.leaf.is_none());

        let v = json!({ "id": "s1", "workspace": "w1", "created": 1, "archived": true });
        assert!(serde_json::from_value::<SessionMeta>(v).unwrap().archived);
    }

    #[test]
    fn snapshot_types_roundtrip() {
        let ws = Workspace {
            id: "w1".into(),
            name: "tau".into(),
            cwd: "/tmp/tau".into(),
        };
        let ws_back: Workspace =
            serde_json::from_str(&serde_json::to_string(&ws).unwrap()).unwrap();
        assert_eq!(ws_back, ws);

        let range = EntryRange {
            start: 5,
            count: 50,
        };
        let range_back: EntryRange =
            serde_json::from_str(&serde_json::to_string(&range).unwrap()).unwrap();
        assert_eq!(range_back, range);

        let blob = BlobRef {
            id: "e9".into(),
            size: 4096,
            hash: "0011".into(),
        };
        let blob_back: BlobRef =
            serde_json::from_str(&serde_json::to_string(&blob).unwrap()).unwrap();
        assert_eq!(blob_back, blob);

        let entry = ViewEntry {
            id: "e1".into(),
            parent: Some("e0".into()),
            kind: "assistant".into(),
            timestamp: 7,
            payload: json!({ "text": "hi" }),
            blob: Some(blob),
            first_kept: Some("e2".into()),
        };
        let entry_back: ViewEntry =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(entry_back, entry);

        // A payload-less entry (sidecar blob) is the common page-read shape.
        let bare = ViewEntry {
            blob: None,
            first_kept: None,
            parent: None,
            ..entry
        };
        let bare_back: ViewEntry =
            serde_json::from_str(&serde_json::to_string(&bare).unwrap()).unwrap();
        assert_eq!(bare_back, bare);

        // source is default-None: a user's own queued message carries no tag.
        let item: QueuedItem =
            serde_json::from_str(r#"{"text":"next","lane":"steering"}"#).unwrap();
        assert!(item.source.is_none());
        let item_back: QueuedItem =
            serde_json::from_str(&serde_json::to_string(&item).unwrap()).unwrap();
        assert_eq!(item_back, item);

        assert_eq!(
            serde_json::to_string(&TurnState::Running).unwrap(),
            "\"running\""
        );
        let turn: TurnState = serde_json::from_str("\"idle\"").unwrap();
        assert_eq!(turn, TurnState::Idle);
    }

    #[test]
    fn live_state_and_full_snapshot_roundtrip() {
        let live = LiveState {
            queue: vec![QueuedItem {
                text: "next".into(),
                lane: MessageLane::FollowUp,
                source: Some("s2".into()),
            }],
            turn: TurnState::Running,
            subagents: vec![crate::SubagentInfo {
                handle: "h1".into(),
                child: "s2".into(),
                agent_type: "general".into(),
                context_mode: crate::ContextMode::Fresh,
                state: "running".into(),
                waiting_on: None,
                last_message: None,
                usage: None,
                task: None,
                resume_contract: None,
            }],
            tasks: vec![],
        };
        let live_back: LiveState =
            serde_json::from_str(&serde_json::to_string(&live).unwrap()).unwrap();
        assert_eq!(live_back, live);

        let om = OmSnapshot {
            observation_tokens: 100,
            pending_tokens: 5,
            reflector_threshold: 40_000,
        };
        let om_back: OmSnapshot =
            serde_json::from_str(&serde_json::to_string(&om).unwrap()).unwrap();
        assert_eq!(om_back, om);

        let snap = Snapshot {
            workspace: Workspace {
                id: "w1".into(),
                name: "w".into(),
                cwd: "/tmp/w".into(),
            },
            session: SessionMeta {
                id: "s1".into(),
                workspace: "w1".into(),
                title: None,
                parent: None,
                created: 1,
                leaf: Some("e9".into()),
                model: Some("m".into()),
                usage: None,
                archived: false,
            },
            entries: vec![EntryMeta {
                id: "e1".into(),
                parent: None,
                kind: "user".into(),
                timestamp: 1,
                size: 4,
                preview: "hi".into(),
                first_kept: None,
                status: EntryStatus::Ok,
            }],
            om,
            live,
            cursor: "e9".into(),
        };
        let snap_back: Snapshot =
            serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(snap_back, snap);
    }

    #[test]
    fn snapshot_parse_negatives() {
        // An unknown turn state and a cursorless snapshot are data errors —
        // the GUI rebuilds its whole projection from this, so a half-decoded
        // snapshot is worse than none.
        let err = serde_json::from_str::<TurnState>("\"paused\"").unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Data);
        let err = serde_json::from_str::<ViewEntry>(r#"{"id":"e1"}"#).unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Data);
        let mut v = serde_json::to_value(Snapshot {
            workspace: Workspace {
                id: "w".into(),
                name: "w".into(),
                cwd: "/w".into(),
            },
            session: SessionMeta {
                id: "s".into(),
                workspace: "w".into(),
                title: None,
                parent: None,
                created: 0,
                leaf: None,
                model: None,
                usage: None,
                archived: false,
            },
            entries: vec![],
            om: OmSnapshot::default(),
            live: LiveState {
                queue: vec![],
                turn: TurnState::Idle,
                subagents: vec![],
                tasks: vec![],
            },
            cursor: "e1".into(),
        })
        .unwrap();
        v.as_object_mut().unwrap().remove("cursor");
        let err = serde_json::from_value::<Snapshot>(v).unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Data);
    }
}
