//! Session storage: one JSONL file per session (ADR-0005, spec §3) — a header
//! line plus an append-only `id`/`parentId` entry tree, per-line CRC,
//! zstd-compressed sidecar blobs for oversized payloads, and manual zstd
//! archives.
//!
//! The active leaf is the persisted branch choice (`set_leaf`) if any, else
//! the last entry line; the choice lives in the header, which is rewritten
//! atomically (entry lines are never touched — branching is always an append).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Session file format version — a versioned surface separate from the
/// protocol (spec §3, pi-style).
pub const FILE_VERSION: u32 = 1;

/// zstd level for blobs and archives — the level measured in ADR-0005.
const ZSTD_LEVEL: i32 = 3;

/// The sidecar-blob threshold (spec §3: 100 KB — the research
pub const DEFAULT_BLOB_THRESHOLD: u64 = 100_000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    version: u32,
    id: String,
    created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leaf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// The creator session (sub-agent provenance, ADR-0001); the GUI nests
    /// the session under its parent in the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
}

/// A session file: header + append-only entry lines (ADR-0005).
///
/// One writer per session; reads are paged (`entries_range`, `entries_since`)
/// — there is deliberately no full-dump read API (spec §3 protocol rule).
pub(crate) type EntryEventHook = Arc<dyn Fn(&Entry) + Send + Sync>;

pub struct SessionStore {
    id: String,
    root: PathBuf,
    fixed_time: Option<u64>,
    leaf: Option<String>,
    created: u64,
    title: Option<String>,
    parent: Option<String>,
    loaded: bool,
    ids: HashSet<String>,
    next: u64,
    /// The loaded entry lines in file order (empty lines kept as `None`
    /// so the in-memory paging indexes match the file's), plus this store's
    /// own appends: the live session's paged reads are served from here
    /// without re-opening the file. The file-backed reads (`entries_range`,
    /// `entries_since`, `entry`) stay the verification path and are
    /// untouched.
    entries: Vec<Option<Entry>>,
    /// Each entry line's byte length (a snapshot's `size` field, served
    /// without re-serializing the entry).
    entry_len: Vec<usize>,
    /// The file length the in-memory log is current to: the freshness
    /// check (`refresh_if_grown`) re-opens when the file has grown past it
    /// (an external writer — the 10k fixture test), so a live read never
    /// serves a stale log.
    file_len: u64,
    /// A live entry sink (the app maps each appended entry to a protocol
    /// event). Set by the owning session; `None` for stores with no live
    /// surface. Fired at the append choke point so every write path — the
    /// turn loop, the task tools, the OM — emits.
    entry_event_hook: Option<EntryEventHook>,
}

/// Storage errors.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    /// A line failed its CRC check; `line` is the 1-based file line number
    /// (the header is line 1).
    Corrupt {
        line: usize,
        expected: String,
        actual: String,
    },
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::Corrupt {
                line,
                expected,
                actual,
            } => write!(
                f,
                "corrupt session line {line}: expected crc {expected}, found {actual}"
            ),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl SessionStore {
    /// Workspace-scoped store: `{project root}/.tau/` (spec §3 placement).
    #[must_use]
    pub fn for_workspace(project_root: &Path, id: &str) -> Self {
        Self::new(project_root.join(".tau"), id)
    }

    fn new(root: PathBuf, id: &str) -> Self {
        Self {
            id: id.to_owned(),
            root,
            fixed_time: None,
            leaf: None,
            created: 0,
            title: None,
            parent: None,
            loaded: false,
            ids: HashSet::new(),
            // 1-based, matching open()'s empty-file baseline and the id
            // series real sessions carry: a fresh session's first entry is
            // 00000001 whether or not the store reloads before minting.
            next: 1,
            entries: Vec::new(),
            entry_len: Vec::new(),
            file_len: 0,
            entry_event_hook: None,
        }
    }

    /// Set the live entry sink (the app maps each appended entry to a
    /// protocol event); `None` clears it.
    pub fn set_entry_event_hook(&mut self, hook: Option<EntryEventHook>) {
        self.entry_event_hook = hook;
    }
    /// A fixed timestamp for every write (the test seam that makes the
    /// shared fixture byte-deterministic, roadmap G handoff 1).
    #[must_use]
    pub const fn with_fixed_time(mut self, ms: u64) -> Self {
        self.fixed_time = Some(ms);
        self
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The header's created timestamp (epoch ms).
    #[must_use]
    pub const fn created(&self) -> u64 {
        self.created
    }

    /// The header's title (a readable session name; absent on old files).
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root
            .join("sessions")
            .join(format!("{}.jsonl", self.id))
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| {
                u64::try_from(d.as_millis())
                    .expect("epoch milliseconds fit u64 with ~584 million years to spare")
            })
    }
    pub(crate) fn now(&self) -> u64 {
        self.fixed_time.unwrap_or_else(Self::now_ms)
    }

    /// The next entry id, reserved now (the single id counter, ADR-0008):
    /// a wire-only re-emission can be minted before its file line lands, and
    /// the later append reuses the same id.
    pub fn mint_id(&mut self) -> String {
        // Mint only after the file is known: an unloaded store starts at
        // 0 and would re-issue ids an existing session already holds
        // (duplicate id -> cyclic parent chain; see the live-om 80GB
        // regression). A failed open surfaces on the following append.
        let _ = self.ensure_open();
        let id = format!("{:08}", self.next);
        self.next += 1;
        id
    }

    fn new_entry(
        &self,
        id: &str,
        kind: &str,
        payload: Value,
        first_kept_entry_id: Option<String>,
    ) -> Entry {
        Entry {
            id: id.to_owned(),
            parent: None,
            timestamp: self.now(),
            kind: kind.to_owned(),
            payload,
            blob: None,
            first_kept_entry_id,
            crc: None,
        }
    }

    /// Create a new session file with its header line. Fails if it exists.
    pub fn create(&mut self) -> Result<(), Error> {
        if self.path().exists() {
            return Err(Error::Other(format!("session {} already exists", self.id)));
        }
        fs::create_dir_all(self.root.join("sessions"))?;
        let created = self.now();
        let header = Header {
            kind: "session".into(),
            version: FILE_VERSION,
            id: self.id.clone(),
            created,
            leaf: None,
            title: None,
            parent: None,
        };
        self.created = created;
        let mut line = serde_json::to_string(&header)?;
        line.push('\n');
        fs::write(self.path(), &line)?;
        self.file_len = u64::try_from(line.len()).expect("file length, well under u64::MAX");
        Ok(())
    }

    /// Load the header and all entry ids, verifying every line's CRC. A
    /// torn last line (process killed mid-append) is skipped in memory —
    /// the read path never rewrites the file (the writer settles the tail
    /// at the next append); any other bad line is corruption.
    pub fn open(&mut self) -> Result<(), Error> {
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        self.file_len = u64::try_from(raw.len()).expect("file length, well under u64::MAX");
        let lines = Self::entry_lines(&raw);
        let Some(header_line) = lines.first() else {
            return Err(Error::Other("session file has no header".into()));
        };
        let header: Header = serde_json::from_str(header_line)?;
        if header.kind != "session" {
            return Err(Error::Other("first line is not a session header".into()));
        }
        if header.version != FILE_VERSION {
            return Err(Error::Other(format!(
                "unsupported session file version {}",
                header.version
            )));
        }
        if header.id != self.id {
            return Err(Error::Other(format!(
                "header id {} does not match file id {}",
                header.id, self.id
            )));
        }
        self.leaf = header.leaf.clone();
        self.created = header.created;
        self.title = header.title;
        self.parent = header.parent;

        self.ids.clear();
        self.entries.clear();
        self.entry_len.clear();
        self.next = 1;
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                self.entries.push(None);
                self.entry_len.push(0);
                continue;
            }
            let entry: Entry = match line.parse::<Entry>() {
                Ok(e) => e,
                Err(_) => return Err(Error::Other(format!("malformed entry on line {}", i + 1))),
            };
            entry.verify(i + 1)?;
            self.next = self.next.max(entry.id.parse::<u64>().unwrap_or(0) + 1);
            self.ids.insert(entry.id.clone());
            self.entries.push(Some(entry));
            self.entry_len.push(line.len());
        }
        self.loaded = true;
        Ok(())
    }

    fn ensure_open(&mut self) -> Result<(), Error> {
        if !self.loaded {
            self.open()?;
        }
        Ok(())
    }

    /// Paged read by 0-based entry index (the header is not an entry).
    pub fn entries_range(&self, start: usize, end: usize) -> Result<Vec<Entry>, Error> {
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        let mut out = Vec::new();
        for (i, line) in Self::entry_lines(&raw).iter().skip(1).enumerate() {
            if line.is_empty() {
                continue;
            }
            if i >= end {
                break;
            }
            if i >= start {
                let entry: Entry = line.parse::<Entry>()?;
                entry.verify(i + 2)?;
                out.push(entry);
            }
        }
        Ok(out)
    }

    /// Paged read of the entries after `cursor` (exclusive). The cursor line
    /// is located by `"id"` prefix scan (entry ids serialize first, in
    /// append order) so tail reads never parse the entries before the
    /// cursor (spec §3: tail reads stay cheap); entries after it are parsed
    /// and CRC-verified as usual.
    pub fn entries_since(&self, cursor: &str) -> Result<Vec<Entry>, Error> {
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        let mut out = Vec::new();
        let prefix = format!("{{\"id\":\"{cursor}\"");
        let mut past = false;
        for (i, line) in Self::entry_lines(&raw).iter().skip(1).enumerate() {
            if line.is_empty() {
                continue;
            }
            if past {
                let entry: Entry = line.parse::<Entry>()?;
                entry.verify(i + 2)?;
                out.push(entry);
            } else if line.starts_with(&prefix) {
                past = true;
            }
        }
        Ok(out)
    }

    /// Read one entry by id.
    pub fn entry(&self, id: &str) -> Result<Entry, Error> {
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        for (i, line) in Self::entry_lines(&raw).iter().skip(1).enumerate() {
            if line.is_empty() {
                continue;
            }
            let entry: Entry = line.parse::<Entry>()?;
            if entry.id == id {
                entry.verify(i + 2)?;
                return Ok(entry);
            }
        }
        Err(Error::Other(format!("no entry {id}")))
    }
}

mod entry;
mod file;
mod list;
mod log;

pub use entry::{BlobRef, Entry};
pub use list::list_workspace;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_log;
