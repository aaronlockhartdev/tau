//! Session storage: one JSONL file per session (ADR-0005, spec §3) — a header
//! line plus an append-only `id`/`parentId` entry tree, per-line CRC,
//! zstd-compressed sidecar blobs for oversized payloads, and manual zstd
//! archives.
//!
//! The active leaf is the persisted branch choice (`set_leaf`) if any, else
//! the last entry line; the choice lives in the header, which is rewritten
//! atomically (entry lines are never touched — branching is always an append).

use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Session file format version — a versioned surface separate from the
/// protocol (spec §3, pi-style).
pub const FILE_VERSION: u32 = 1;

/// zstd level for blobs and archives — the level measured in ADR-0005.
const ZSTD_LEVEL: i32 = 3;

/// Default sidecar-blob threshold; the config's `[sessions]
/// `blob_threshold_bytes` overrides it (spec §3: 100 KB — the research
/// corpus measured 83 lines >100 KB, max 1 MB, mostly base64 images).
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

/// Out-of-band sidecar blob (ADR-0005 hardening 2): raw bytes stored
/// zstd-compressed in `blobs/`, referenced from the owning entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    /// Blob file name (the owning entry's id).
    pub id: String,
    /// Uncompressed payload size in bytes.
    pub size: u64,
    /// hex xxh3-64 of the uncompressed payload.
    pub hash: String,
}

/// One session entry: one JSONL line (spec §3). `kind` is free-form — the
/// agent-loop and OM tickets define the concrete kinds on top of this storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    /// Previous entry in the branch; `None` for the first entry.
    #[serde(rename = "parentId", default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Epoch milliseconds.
    pub timestamp: u64,
    #[serde(rename = "type")]
    pub kind: String,
    /// Inline payload; `Value::Null` when the data lives in a sidecar blob.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobRef>,
    /// Compaction span reference (spec §3, pi's `firstKeptEntryId` pattern):
    /// the first entry kept after this record's compressed span.
    #[serde(
        default,
        rename = "firstKeptEntryId",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_kept_entry_id: Option<String>,
    /// CRC32 (hex) of the canonical line with this field empty.
    #[serde(
        default,
        serialize_with = "serialize_crc",
        deserialize_with = "deserialize_crc"
    )]
    pub crc: Option<String>,
}

fn serialize_crc<S: serde::Serializer>(crc: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crc.clone().unwrap_or_default())
}

fn deserialize_crc<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = String::deserialize(d)?;
    Ok(if v.is_empty() { None } else { Some(v) })
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & ((crc & 1) * 0xEDB8_8320));
        }
    }
    !crc
}

impl Entry {
    fn canonical_line(&self) -> String {
        serde_json::to_string(self).expect("Entry is always serializable")
    }

    fn compute_crc(&self) -> String {
        let mut without = self.clone();
        without.crc = None;
        format!("{:08x}", crc32(without.canonical_line().as_bytes()))
    }

    /// A CRC mismatch means the line was corrupted after the write
    /// (spec §3 hardening 1).
    fn verify(&self, line: usize) -> Result<(), Error> {
        let expected = self.compute_crc();
        match &self.crc {
            Some(c) if *c == expected => Ok(()),
            _ => Err(Error::Corrupt {
                line,
                expected,
                actual: self.crc.clone().unwrap_or_default(),
            }),
        }
    }
}

/// A session file: header + append-only entry lines (ADR-0005).
///
/// One writer per session; reads are paged (`entries_range`, `entries_since`)
/// — there is deliberately no full-dump read API (spec §3 protocol rule).
pub struct SessionStore {
    id: String,
    root: PathBuf,
    blob_threshold: u64,
    leaf: Option<String>,
    created: u64,
    title: Option<String>,
    parent: Option<String>,
    loaded: bool,
    ids: HashSet<String>,
    next: u64,
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
    pub fn for_workspace(project_root: &Path, id: &str) -> Self {
        Self::new(project_root.join(".tau"), id)
    }

    /// No-project store: `~/.config/tau/` (spec §3 placement).
    pub fn for_no_project(id: &str) -> Self {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .expect("XDG_CONFIG_HOME or HOME must be set");
        Self::new(config_home.join("tau"), id)
    }

    fn new(root: PathBuf, id: &str) -> Self {
        Self {
            id: id.to_owned(),
            root,
            blob_threshold: DEFAULT_BLOB_THRESHOLD,
            leaf: None,
            created: 0,
            title: None,
            parent: None,
            loaded: false,
            ids: HashSet::new(),
            next: 0,
        }
    }

    /// Override the sidecar-blob threshold (normally the config's
    /// `[sessions] blob_threshold_bytes`).
    pub fn with_blob_threshold(mut self, bytes: u64) -> Self {
        self.blob_threshold = bytes;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The header's created timestamp (epoch ms).
    pub fn created(&self) -> u64 {
        self.created
    }

    /// The header's title (a readable session name; absent on old files).
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub fn path(&self) -> PathBuf {
        self.root
            .join("sessions")
            .join(format!("{}.jsonl", self.id))
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.root.join("blobs").join(id)
    }

    pub fn archive_path(&self) -> PathBuf {
        self.root
            .join("archive")
            .join(format!("{}.jsonl.zst", self.id))
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn new_entry(&self, kind: &str, payload: Value, first_kept_entry_id: Option<String>) -> Entry {
        Entry {
            id: format!("{:08}", self.next),
            parent: None,
            timestamp: Self::now_ms(),
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
        let created = Self::now_ms();
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
        fs::write(self.path(), line)?;
        Ok(())
    }

    /// Load the header and all entry ids, verifying every line's CRC. A torn
    /// last line (process killed mid-append) is terminated or truncated —
    /// kill-safe by construction (ADR-0005); any other bad line is corruption.
    pub fn open(&mut self) -> Result<(), Error> {
        let mut raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        if !raw.ends_with('\n') {
            match raw.rfind('\n') {
                Some(idx) => {
                    let partial = &raw[idx + 1..];
                    if partial.parse::<Entry>().is_ok() {
                        raw.push('\n');
                    } else {
                        raw.truncate(idx + 1);
                    }
                    fs::write(self.path(), &raw)?;
                }
                None => {
                    if serde_json::from_str::<Header>(&raw).is_err() {
                        return Err(Error::Other("session file has no header".into()));
                    }
                    raw.push('\n');
                    fs::write(self.path(), &raw)?;
                }
            }
        }
        let lines: Vec<&str> = raw.split('\n').collect();
        let header: Header = serde_json::from_str(lines[0])?;
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
        self.title = header.title.clone();
        self.parent = header.parent.clone();

        self.ids.clear();
        self.next = 1;
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            let entry: Entry = match line.parse::<Entry>() {
                Ok(e) => e,
                Err(_) => return Err(Error::Other(format!("malformed entry on line {}", i + 1))),
            };
            entry.verify(i + 1)?;
            self.next = self.next.max(entry.id.parse::<u64>().unwrap_or(0) + 1);
            self.ids.insert(entry.id);
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

    fn append_line(&mut self, mut entry: Entry, parent: Option<&str>) -> Result<Entry, Error> {
        self.ensure_open()?;
        if let Some(p) = parent
            && !self.ids.contains(p)
        {
            return Err(Error::Other(format!("unknown parent {p}")));
        }
        entry.parent = parent.map(str::to_owned);
        self.extract_blob_if_needed(&mut entry)?;
        entry.crc = Some(entry.compute_crc());

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())?;
        let mut line = entry.canonical_line();
        line.push('\n');
        file.write_all(line.as_bytes())?;

        self.ids.insert(entry.id.clone());
        self.next += 1;
        self.loaded = true;
        Ok(entry)
    }

    /// Move an oversized inline payload out to a zstd sidecar blob
    /// (ADR-0005 hardening 2).
    fn extract_blob_if_needed(&self, entry: &mut Entry) -> Result<(), Error> {
        if entry.blob.is_some() || entry.payload.is_null() {
            return Ok(());
        }
        let bytes = entry.payload.to_string().into_bytes();
        if (bytes.len() as u64) <= self.blob_threshold {
            return Ok(());
        }
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
        let compressed = zstd::encode_all(&bytes[..], ZSTD_LEVEL)?;
        fs::create_dir_all(self.root.join("blobs"))?;
        fs::write(self.blob_path(&entry.id), compressed)?;
        entry.payload = Value::Null;
        entry.blob = Some(BlobRef {
            id: entry.id.clone(),
            size: bytes.len() as u64,
            hash,
        });
        Ok(())
    }

    /// Append an entry; payloads above the blob threshold go to a sidecar
    /// blob automatically.
    pub fn append(
        &mut self,
        kind: &str,
        payload: Value,
        parent: Option<&str>,
    ) -> Result<Entry, Error> {
        self.append_line(self.new_entry(kind, payload, None), parent)
    }

    /// Append a compaction record referencing the first kept entry (spec §3).
    pub fn append_compaction(
        &mut self,
        first_kept_entry_id: &str,
        payload: Value,
        parent: Option<&str>,
    ) -> Result<Entry, Error> {
        self.append_line(
            self.new_entry("compaction", payload, Some(first_kept_entry_id.to_owned())),
            parent,
        )
    }

    /// Append a raw-byte payload (e.g. decoded image bytes) — always a
    /// sidecar blob, keeping base64 out of the log (ADR-0005).
    pub fn append_raw(
        &mut self,
        kind: &str,
        bytes: Vec<u8>,
        parent: Option<&str>,
    ) -> Result<Entry, Error> {
        self.ensure_open()?;
        if let Some(p) = parent
            && !self.ids.contains(p)
        {
            return Err(Error::Other(format!("unknown parent {p}")));
        }
        let id = format!("{:08}", self.next);
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
        let compressed = zstd::encode_all(&bytes[..], ZSTD_LEVEL)?;
        fs::create_dir_all(self.root.join("blobs"))?;
        fs::write(self.blob_path(&id), compressed)?;
        let mut entry = self.new_entry(kind, Value::Null, None);
        entry.id = id;
        entry.parent = parent.map(str::to_owned);
        entry.blob = Some(BlobRef {
            id: entry.id.clone(),
            size: bytes.len() as u64,
            hash,
        });
        entry.crc = Some(entry.compute_crc());
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())?;
        let mut line = entry.canonical_line();
        line.push('\n');
        file.write_all(line.as_bytes())?;
        self.ids.insert(entry.id.clone());
        self.next += 1;
        Ok(entry)
    }

    /// The active leaf: the persisted branch choice if any, else the last
    /// entry line in the file; `None` = a fresh session with no entries (not
    /// an error — the first entry starts a root branch).
    pub fn leaf(&mut self) -> Result<Option<Entry>, Error> {
        self.ensure_open()?;
        if let Some(l) = &self.leaf {
            return self.entry(l).map(Some);
        }
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        // Line 1 is the header; the leaf is the last entry line after it.
        let lines: Vec<&str> = raw.lines().skip(1).collect();
        let Some(line) = lines.iter().rev().find(|l| !l.is_empty()) else {
            return Ok(None);
        };
        let entry: Entry = line.parse::<Entry>()?;
        entry.verify(raw.lines().count())?;
        Ok(Some(entry))
    }

    /// A unique temp path for an atomic rewrite (note 3): a shared fixed
    /// name lets two concurrently dispatched commands interleave on one
    /// file; the nanosecond clock keeps the names apart.
    fn rewrite_tmp(&self) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.path().with_file_name(format!(
            "{}.tmp-{nanos:016x}",
            self.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    }

    /// Persist an explicit branch choice (the GUI switches the active
    /// branch); only the header line's content changes, written atomically
    /// via temp-file rename.
    pub fn set_leaf(&mut self, leaf_id: &str) -> Result<(), Error> {
        self.ensure_open()?;
        if !self.ids.contains(leaf_id) {
            return Err(Error::Other(format!("unknown leaf {leaf_id}")));
        }
        let raw = fs::read_to_string(self.path())?;
        let mut lines: Vec<&str> = raw.split('\n').collect();
        let mut header: Header = serde_json::from_str(lines[0])?;
        header.leaf = Some(leaf_id.to_owned());
        let new_header = serde_json::to_string(&header)?;
        lines[0] = &new_header;
        let joined = lines.join("\n");
        let tmp = self.rewrite_tmp();
        fs::write(&tmp, &joined)?;
        fs::rename(&tmp, self.path())?;
        self.leaf = Some(leaf_id.to_owned());
        Ok(())
    }

    /// Set the session's readable name; like set_leaf this rewrites only
    /// the header line, atomically via temp-file rename.
    pub fn set_title(&mut self, title: &str) -> Result<(), Error> {
        self.ensure_open()?;
        let raw = fs::read_to_string(self.path())?;
        let mut lines: Vec<&str> = raw.split('\n').collect();
        let mut header: Header = serde_json::from_str(lines[0])?;
        header.title = Some(title.to_owned());
        let new_header = serde_json::to_string(&header)?;
        lines[0] = &new_header;
        let joined = lines.join("\n");
        let tmp = self.rewrite_tmp();
        fs::write(&tmp, &joined)?;
        fs::rename(&tmp, self.path())?;
        self.title = Some(title.to_owned());
        Ok(())
    }

    /// The creator session (a sub-agent's parent, ADR-0001); written into
    /// the header so the link survives restarts.
    pub fn set_parent(&mut self, parent: &str) -> Result<(), Error> {
        self.ensure_open()?;
        let raw = fs::read_to_string(self.path())?;
        let mut lines: Vec<&str> = raw.split('\n').collect();
        let mut header: Header = serde_json::from_str(lines[0])?;
        header.parent = Some(parent.to_owned());
        let new_header = serde_json::to_string(&header)?;
        lines[0] = &new_header;
        let joined = lines.join("\n");
        let tmp = self.rewrite_tmp();
        fs::write(&tmp, &joined)?;
        fs::rename(&tmp, self.path())?;
        self.parent = Some(parent.to_owned());
        Ok(())
    }

    /// A fork (spec §5.1): the source's entire entry tree copied into a new
    /// session file — identical entry ids and parent links (the per-line CRCs
    /// stay valid on the copied lines), a fresh header, and the source's
    /// active leaf inherited.
    pub fn fork_from(source: &Self, id: &str) -> Result<Self, Error> {
        if id == source.id {
            return Err(Error::Other(format!(
                "fork target {id} is the source session"
            )));
        }
        let raw = fs::read_to_string(source.path())?;
        let mut lines: Vec<&str> = raw.lines().collect();
        let Some(first) = lines.first() else {
            return Err(Error::Other("empty session file".into()));
        };
        let mut header: Header = serde_json::from_str(first)?;
        header.id = id.to_owned();
        let new_header = serde_json::to_string(&header)?;
        lines[0] = &new_header;
        let mut target = Self::new(source.root.clone(), id);
        if target.path().exists() {
            return Err(Error::Other(format!("fork target {id} already exists")));
        }
        fs::create_dir_all(target.root.join("sessions"))?;
        fs::write(target.path(), format!("{}\n", lines.join("\n")))?;
        target.open()?;
        Ok(target)
    }

    /// A new session id: 12 hex digits (48 bits) of random. The id only
    /// needs to be unique — the namespace is per machine (the file lives
    /// under the workspace's .tau/sessions/), and the session list sorts on
    /// the created timestamp, not the id. The birthday bound on 2^48 is
    /// negligible at machine-scale session counts.
    pub fn new_session_id() -> String {
        let bytes: [u8; 6] = rand::rng().random();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Paged read by 0-based entry index (the header is not an entry).
    pub fn entries_range(&self, start: usize, end: usize) -> Result<Vec<Entry>, Error> {
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::Other(e.to_string()))?;
        let mut out = Vec::new();
        for (i, line) in raw.lines().skip(1).enumerate() {
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
        for (i, line) in raw.lines().skip(1).enumerate() {
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
        for (i, line) in raw.lines().skip(1).enumerate() {
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

    /// Decode a sidecar blob, verifying its hash (ADR-0005).
    pub fn resolve_blob(&self, ref_: &BlobRef) -> Result<Vec<u8>, Error> {
        let compressed =
            fs::read(self.blob_path(&ref_.id)).map_err(|e| Error::Other(e.to_string()))?;
        let bytes = zstd::decode_all(&compressed[..]).map_err(|e| Error::Other(e.to_string()))?;
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
        if hash != ref_.hash {
            return Err(Error::Other(format!("blob {} hash mismatch", ref_.id)));
        }
        Ok(bytes)
    }

    /// Manual archive: zstd the session file into `archive/` and remove the
    /// live file. One-way, off the live read/write path (ADR-0005).
    pub fn archive(&self) -> Result<PathBuf, Error> {
        let live = self.path();
        let arc = self.archive_path();
        if !live.exists() {
            return Err(Error::Other(format!(
                "cannot archive {}: not present",
                live.display()
            )));
        }
        if arc.exists() {
            return Err(Error::Other(format!(
                "archive {} already exists",
                arc.display()
            )));
        }
        let bytes = fs::read(&live)?;
        fs::create_dir_all(self.root.join("archive"))?;
        let compressed = zstd::encode_all(&bytes[..], ZSTD_LEVEL)?;
        fs::write(&arc, compressed)?;
        fs::remove_file(live)?;
        Ok(arc)
    }

    /// Restore a manually archived session; removes the archive.
    pub fn unarchive(&self) -> Result<(), Error> {
        let live = self.path();
        let arc = self.archive_path();
        if live.exists() {
            return Err(Error::Other("live session already present".into()));
        }
        if !arc.exists() {
            return Err(Error::Other(format!(
                "archive {} not present",
                arc.display()
            )));
        }
        let compressed = fs::read(&arc)?;
        let bytes = zstd::decode_all(&compressed[..])?;
        fs::create_dir_all(self.root.join("sessions"))?;
        fs::write(live, bytes)?;
        fs::remove_file(arc)?;
        Ok(())
    }

    /// The archive file's header line, read bounded (ADR-0005): a listing
    /// must not decompress a whole transcript to read one short line, so
    /// the stream is decoded only up to the first newline, capped at 4 KiB
    /// (overflow is an error — a header is a single JSON line, well under
    /// the cap).
    pub fn archive_header_line(&self) -> Result<String, Error> {
        use std::io::Read as _;
        const CAP: usize = 4096;
        let file = fs::File::open(self.archive_path()).map_err(|e| Error::Other(e.to_string()))?;
        let mut buf = Vec::with_capacity(CAP);
        zstd::Decoder::new(file)?
            .take(CAP as u64)
            .read_to_end(&mut buf)?;
        let text = std::str::from_utf8(&buf).map_err(|e| Error::Other(e.to_string()))?;
        match text.find('\n') {
            // The header is the first line: a complete line inside the cap
            // is a short header, even when the window is full (a big
            // archive whose body was never decoded).
            Some(pos) => Ok(text[..pos].to_owned()),
            None if buf.len() == CAP => Err(Error::Other(format!(
                "archive {} header exceeds the {CAP}-byte read",
                self.id
            ))),
            None => Err(Error::Other(format!(
                "archive {} has no header line",
                self.id
            ))),
        }
    }
}

impl std::str::FromStr for Entry {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path, id: &str) -> SessionStore {
        SessionStore::for_workspace(dir, id)
    }

    fn seeded(dir: &Path) -> (SessionStore, Vec<Entry>) {
        let mut s = store(dir, "s1");
        s.create().unwrap();
        let e1 = s
            .append(
                "message",
                serde_json::json!({"role": "user", "text": "one"}),
                None,
            )
            .unwrap();
        let e2 = s
            .append(
                "message",
                serde_json::json!({"role": "assistant", "text": "two"}),
                Some(&e1.id),
            )
            .unwrap();
        let e3 = s
            .append(
                "message",
                serde_json::json!({"role": "user", "text": "three"}),
                Some(&e2.id),
            )
            .unwrap();
        (s, vec![e1, e2, e3])
    }

    #[test]
    fn create_writes_a_versioned_header() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1");
        s.create().unwrap();
        let line = fs::read_to_string(s.path()).unwrap();
        let h: Header = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(h.version, FILE_VERSION);
        assert_eq!(h.id, "s1");
        assert!(s.path().starts_with(tmp.path().join(".tau")));
    }

    #[test]
    fn append_round_trip_builds_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, entries) = seeded(tmp.path());
        assert_eq!(entries[0].parent, None);
        assert_eq!(entries[1].parent, Some(entries[0].id.clone()));
        assert_eq!(entries[2].parent, Some(entries[1].id.clone()));
        let leaf = s.leaf().unwrap().unwrap();
        assert_eq!(leaf.id, entries[2].id);
        let again = s.entry(&entries[1].id).unwrap();
        assert_eq!(again, entries[1]);
    }

    #[test]
    fn branch_appends_a_child_line() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, entries) = seeded(tmp.path());
        // Branch off the first entry — an append, no rewrite.
        let e4 = s
            .append(
                "message",
                serde_json::json!({"text": "branch"}),
                Some(&entries[0].id),
            )
            .unwrap();
        assert_eq!(e4.parent, Some(entries[0].id.clone()));
        assert_eq!(s.leaf().unwrap().unwrap().id, e4.id);
        let since = s.entries_since(&entries[0].id).unwrap();
        assert_eq!(
            since.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec![
                entries[1].id.as_str(),
                entries[2].id.as_str(),
                e4.id.as_str()
            ]
        );
    }

    #[test]
    fn set_leaf_rewrites_only_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, entries) = seeded(tmp.path());
        let before = fs::read_to_string(s.path()).unwrap();
        s.set_leaf(&entries[0].id).unwrap();
        let after = fs::read_to_string(s.path()).unwrap();
        let (_b_h, b_rest) = before.split_once('\n').unwrap();
        let (a_h, a_rest) = after.split_once('\n').unwrap();
        assert_eq!(b_rest, a_rest);
        let h: Header = serde_json::from_str(a_h).unwrap();
        assert_eq!(h.leaf, Some(entries[0].id.clone()));
        assert!(!s.path().with_extension("tmp").exists());
        assert_eq!(s.leaf().unwrap().unwrap().id, entries[0].id);
    }

    #[test]
    fn set_title_persists_in_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, _entries) = seeded(tmp.path());
        s.set_title("Brave Otter").unwrap();
        assert_eq!(s.title(), Some("Brave Otter"));
        let mut reopened = SessionStore::for_workspace(tmp.path(), s.id());
        reopened.open().unwrap();
        assert_eq!(reopened.title(), Some("Brave Otter"));
    }

    #[test]
    fn set_parent_persists_in_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, _entries) = seeded(tmp.path());
        s.set_parent("18d63f408cee5080").unwrap();
        assert_eq!(s.parent(), Some("18d63f408cee5080"));
        let mut reopened = SessionStore::for_workspace(tmp.path(), s.id());
        reopened.open().unwrap();
        assert_eq!(reopened.parent(), Some("18d63f408cee5080"));
    }

    #[test]
    fn crc_catches_a_flipped_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, entries) = seeded(tmp.path());
        let mut raw = fs::read_to_string(s.path()).unwrap().into_bytes();
        // Flip a byte inside a *value* on the second entry line (file line
        // 3) so the JSON stays valid and only the CRC can catch it.
        let text = String::from_utf8(raw.clone()).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        let start = lines[..2].iter().map(|l| l.len() + 1).sum::<usize>();
        let off = start + lines[2].find("\"two\"").unwrap() + 3;
        raw[off] = b'i';
        fs::write(s.path(), &raw).unwrap();
        let err = s.entries_range(1, 2).unwrap_err();
        match err {
            Error::Corrupt { line, .. } => assert_eq!(line, 3),
            other => panic!("expected Corrupt, got {other:?}"),
        }
        // The uncorrupted entry still reads.
        assert_eq!(s.entry(&entries[0].id).unwrap().id, entries[0].id);
    }

    #[test]
    fn torn_last_line_is_repaired_on_open() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, entries) = seeded(tmp.path());
        let mut raw = fs::read_to_string(s.path()).unwrap();
        raw.push_str(&format!("{{\"id\":\"{:08}\",\"parentId\":null", 99)); // no newline: a kill mid-append
        fs::write(s.path(), raw).unwrap();
        let mut reopened = store(tmp.path(), "s1");
        reopened.open().unwrap();
        assert_eq!(reopened.leaf().unwrap().unwrap().id, entries[2].id);
        let fixed = fs::read_to_string(s.path()).unwrap();
        assert!(fixed.ends_with('\n'));
        assert!(!fixed.contains("99999999"));
    }

    #[test]
    fn valid_last_line_without_newline_is_terminated_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, entries) = seeded(tmp.path());
        let raw = fs::read_to_string(s.path()).unwrap();
        fs::write(s.path(), raw.trim_end_matches('\n')).unwrap();
        let mut reopened = store(tmp.path(), "s1");
        reopened.open().unwrap();
        assert_eq!(reopened.leaf().unwrap().unwrap().id, entries[2].id);
        assert!(fs::read_to_string(s.path()).unwrap().ends_with('\n'));
    }

    #[test]
    fn oversized_payload_becomes_a_sidecar_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1").with_blob_threshold(100);
        s.create().unwrap();
        let payload = Value::String("x".repeat(500));
        let e = s.append("tool_result", payload.clone(), None).unwrap();
        assert!(e.payload.is_null());
        let blob = e.blob.as_ref().unwrap();
        assert_eq!(blob.size, 502);
        assert!(s.blob_path(&e.id).exists());
        let decoded = s.resolve_blob(blob).unwrap();
        assert_eq!(decoded, serde_json::to_string(&payload).unwrap().as_bytes());
        // The on-disk line carries a pointer, not the payload.
        let content = fs::read_to_string(s.path()).unwrap();
        let line = content.lines().last().unwrap();
        assert!(!line.contains(&"x".repeat(50)));
    }

    #[test]
    fn small_payload_stays_inline() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1").with_blob_threshold(100);
        s.create().unwrap();
        let e = s
            .append("message", Value::String("y".repeat(50)), None)
            .unwrap();
        assert!(e.blob.is_none());
        assert_eq!(e.payload, Value::String("y".repeat(50)));
    }

    #[test]
    fn raw_bytes_always_become_a_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1");
        s.create().unwrap();
        let bytes: Vec<u8> = (0..10).map(|i| i as u8).collect();
        let e = s.append_raw("image", bytes.clone(), None).unwrap();
        let blob = e.blob.as_ref().unwrap();
        assert_eq!(blob.size, 10);
        assert_eq!(s.resolve_blob(blob).unwrap(), bytes);
    }

    #[test]
    fn tampered_blob_fails_its_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1");
        s.create().unwrap();
        let e = s.append_raw("image", vec![1, 2, 3, 4], None).unwrap();
        let blob = s.blob_path(&e.id);
        let mut compressed = fs::read(&blob).unwrap();
        compressed[0] ^= 0xFF;
        fs::write(&blob, compressed).unwrap();
        assert!(s.resolve_blob(e.blob.as_ref().unwrap()).is_err());
    }

    #[test]
    fn archive_and_unarchive_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, _) = seeded(tmp.path());
        let original = fs::read(s.path()).unwrap();
        let arc = s.archive().unwrap();
        assert!(!s.path().exists());
        assert!(arc.exists());
        s.unarchive().unwrap();
        assert!(!arc.exists());
        assert_eq!(fs::read(s.path()).unwrap(), original);
    }

    #[test]
    fn archiving_twice_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, _) = seeded(tmp.path());
        s.archive().unwrap();
        assert!(s.archive().is_err());
    }

    #[test]
    fn an_archive_header_read_needs_no_full_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, _) = seeded(tmp.path());
        // A body big enough that a full decode would be the obvious way to
        // read the header — the bounded read must not need it.
        for i in 0..200 {
            s.append(
                "message",
                serde_json::json!({"text": format!("entry {i} {}", "x".repeat(512))}),
                None,
            )
            .unwrap();
        }
        s.archive().unwrap();
        let line = s.archive_header_line().unwrap();
        let h: Header = serde_json::from_str(&line).unwrap();
        assert_eq!(h.id, "s1");
        assert_eq!(h.kind, "session");
        assert_eq!(h.version, FILE_VERSION);
    }

    #[test]
    fn an_archive_header_overflow_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, _) = seeded(tmp.path());
        // A first line over the cap with no newline: the bounded read stops
        // at the cap and reports the overflow instead of decoding on.
        let header = format!(
            "{{\"type\":\"session\",\"version\":1,\"id\":\"s1\",\"created\":0,\"title\":\"{}\"}}{}",
            "x".repeat(4500),
            "y".repeat(1000)
        );
        fs::create_dir_all(s.root.join("archive")).unwrap();
        let compressed = zstd::encode_all(header.as_bytes(), ZSTD_LEVEL).unwrap();
        fs::write(s.archive_path(), compressed).unwrap();
        assert!(s.archive_header_line().is_err());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, _) = seeded(tmp.path());
        let mut raw = fs::read_to_string(s.path()).unwrap();
        raw = raw.replacen("\"version\":1", "\"version\":9", 1);
        fs::write(s.path(), raw).unwrap();
        let mut reopened = store(tmp.path(), "s1");
        assert!(reopened.open().is_err());
    }

    #[test]
    fn unknown_parent_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, _) = seeded(tmp.path());
        assert!(
            s.append("message", serde_json::json!({}), Some("99999999"))
                .is_err()
        );
    }

    #[test]
    fn compaction_entry_carries_the_span_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, entries) = seeded(tmp.path());
        let c = s
            .append_compaction(
                &entries[2].id,
                serde_json::json!({"summary": "earlier work"}),
                Some(&entries[2].id),
            )
            .unwrap();
        assert_eq!(c.kind, "compaction");
        assert_eq!(c.first_kept_entry_id, Some(entries[2].id.clone()));
        let on_disk = s.entry(&c.id).unwrap();
        assert_eq!(on_disk.first_kept_entry_id, Some(entries[2].id.clone()));
    }

    #[test]
    fn leaf_is_none_on_a_fresh_session_and_the_entry_on_a_used_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(dir.path(), "s1");
        store.create().unwrap();
        assert_eq!(store.leaf().unwrap(), None);
        let e = store
            .append("user", serde_json::json!({"text": "hi"}), None)
            .unwrap();
        assert_eq!(store.leaf().unwrap().unwrap().id, e.id);
    }
    #[test]
    fn a_fork_copies_the_entry_tree_with_stable_ids_and_crcs() {
        let dir = tempfile::tempdir().unwrap();
        let (s, entries) = seeded(dir.path());
        let _forked = SessionStore::fork_from(&s, "s2").unwrap();
        // A second independent open verifies every copied line's CRC and
        // resolves the inherited leaf.
        let mut re = store(dir.path(), "s2");
        re.open().unwrap();
        let copied = re.entries_range(0, usize::MAX).unwrap();
        assert_eq!(copied.len(), entries.len());
        for (a, b) in entries.iter().zip(&copied) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.parent, b.parent);
            assert_eq!(a.crc, b.crc);
        }
        // The fork appends against the inherited leaf, not a new root.
        let e = re
            .append(
                "user",
                serde_json::json!({"text": "after the fork"}),
                Some(&entries.last().unwrap().id),
            )
            .unwrap();
        assert_eq!(
            e.parent.as_deref(),
            Some(entries.last().unwrap().id.as_str())
        );
    }

    #[test]
    fn a_fork_cannot_target_the_source_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = seeded(dir.path());
        assert!(SessionStore::fork_from(&s, "s1").is_err());
    }

    #[test]
    fn a_fork_cannot_clobber_an_existing_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = seeded(dir.path());
        let mut taken = store(dir.path(), "s2");
        taken.create().unwrap();
        taken
            .append("user", serde_json::json!({"text": "the original"}), None)
            .unwrap();
        assert!(SessionStore::fork_from(&s, "s2").is_err());
        // The existing file is untouched.
        let mut re = store(dir.path(), "s2");
        re.open().unwrap();
        let entries = re.entries_range(0, usize::MAX).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload["text"], "the original");
    }

    #[test]
    fn session_ids_are_distinct_and_well_formed() {
        let a = SessionStore::new_session_id();
        let b = SessionStore::new_session_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
