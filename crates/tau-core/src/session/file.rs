use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{BlobRef, DEFAULT_BLOB_THRESHOLD, Entry, Error, Header, SessionStore, ZSTD_LEVEL};

impl SessionStore {
    pub(super) fn blob_path(&self, id: &str) -> PathBuf {
        self.root.join("blobs").join(id)
    }

    #[must_use]
    pub fn archive_path(&self) -> PathBuf {
        self.root
            .join("archive")
            .join(format!("{}.jsonl.zst", self.id))
    }

    /// Decode a sidecar blob from `root` (the workspace's `.tau` dir),
    /// verifying its hash (ADR-0005).
    pub fn read_blob(root: &Path, ref_: &BlobRef) -> Result<Vec<u8>, Error> {
        let compressed = fs::read(root.join("blobs").join(&ref_.id))?;
        let bytes = zstd::decode_all(&compressed[..]).map_err(|e| Error::Other(e.to_string()))?;
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
        if hash != ref_.hash {
            return Err(Error::Other(format!("blob {} hash mismatch", ref_.id)));
        }
        Ok(bytes)
    }

    pub fn resolve_blob(&self, ref_: &BlobRef) -> Result<Vec<u8>, Error> {
        Self::read_blob(&self.root, ref_)
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
        const CAP: usize = 4096;
        let file = fs::File::open(self.archive_path()).map_err(|e| Error::Other(e.to_string()))?;
        let mut buf = Vec::with_capacity(CAP);
        zstd::Decoder::new(file)?
            .take(u64::try_from(CAP).expect("4 KiB cap"))
            .read_to_end(&mut buf)?;
        let text = std::str::from_utf8(&buf).map_err(|e| Error::Other(e.to_string()))?;
        match text.find('\n') {
            // The header is the first line: a complete line inside the cap
            // is a short header, even when the window is full (a big
            // archive whose body was never decoded).
            Some(pos) => Ok(text
                .get(..pos)
                .expect("offset from find() is a char boundary")
                .to_owned()),
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

    /// Split the raw file into lines, dropping a torn tail: a final line
    /// without its terminating newline that parses as neither an entry
    /// nor the header is a kill-interrupted write — not an entry, and the
    /// next append settles it. A complete final line missing only its
    /// terminator is kept (as is an unterminated header). The read path
    /// never rewrites the file; it only tolerates the torn tail in memory.
    pub(super) fn entry_lines(raw: &str) -> Vec<&str> {
        let mut lines: Vec<&str> = raw.lines().collect();
        if !raw.ends_with('\n')
            && lines.last().is_some_and(|l| {
                l.parse::<Entry>().is_err() && serde_json::from_str::<Header>(l).is_err()
            })
        {
            lines.pop();
        }
        lines
    }

    fn append_line(&mut self, mut entry: Entry, parent: Option<&str>) -> Result<Entry, Error> {
        self.ensure_open()?;
        // A loaded store whose file is gone (deleted, or archived out
        // from under it) is in an inconsistent state: refuse the append
        // — never resurrect a file headerless (unopenable, invisible to
        // the list, orphaned on disk; ADR-0005).
        if !self.path().exists() {
            return Err(Error::Other(format!(
                "session {} file is gone — the append was refused",
                self.id
            )));
        }
        if let Some(p) = parent
            && !self.ids.contains(p)
        {
            return Err(Error::Other(format!("unknown parent {p}")));
        }
        entry.parent = parent.map(str::to_owned);
        self.extract_blob_if_needed(&mut entry)?;
        entry.crc = Some(entry.compute_crc());

        // The writer — never the reader — settles a tail left unterminated
        // by a kill mid-append, so no mangled line lands mid-file.
        self.settle_tail()?;

        let mut file = OpenOptions::new()
            // create(false): a file deleted after open is a refusal (the
            // check above), never a silent resurrection.
            .create(false)
            .append(true)
            .open(self.path())?;
        let mut line = entry.canonical_line();
        let line_len = line.len();
        line.push('\n');
        file.write_all(line.as_bytes())?;
        // The writer's own growth: keep the bookkeeping `refresh_if_grown`
        // relies on (a settle may also have touched the tail).
        if let Ok(m) = fs::metadata(self.path()) {
            self.file_len = m.len();
        }

        self.ids.insert(entry.id.clone());
        self.entries.push(Some(entry.clone()));
        self.entry_len.push(line_len);
        // The id was reserved up front (mint_id): advance the counter past
        // it, never double-advance.
        self.next = self.next.max(entry.id.parse::<u64>().unwrap_or(0) + 1);
        self.loaded = true;
        Ok(entry)
    }

    /// The writer-side tail settlement (the read path never rewrites, so
    /// it cannot race the writer): a kill mid-append leaves the file's
    /// last line unterminated — a complete line missing its newline, or a
    /// torn partial. The next append settles it: the complete line is
    /// terminated, the torn one truncated, and the new line starts clean.
    fn settle_tail(&self) -> Result<(), Error> {
        let mut file = fs::File::open(self.path())?;
        let len = file.metadata()?.len();
        if len == 0 {
            return Ok(());
        }
        file.seek(io::SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        file.read_exact(&mut last)?;
        if last[0] == b'\n' {
            return Ok(());
        }
        let raw = fs::read_to_string(self.path())?;
        let idx = raw.rfind('\n').map_or(0, |i| i + 1);
        if raw
            .get(idx..)
            .expect("offset from rfind() is a char boundary")
            .parse::<Entry>()
            .is_ok()
        {
            // A complete line missing its newline: terminate it.
            let mut tail = OpenOptions::new().append(true).open(self.path())?;
            tail.write_all(b"\n")?;
        } else {
            // A torn line: truncate it; the append below starts clean.
            fs::write(
                self.path(),
                raw.get(..idx)
                    .expect("offset from rfind() is a char boundary"),
            )?;
        }
        Ok(())
    }

    /// Move an oversized inline payload out to a zstd sidecar blob
    /// (ADR-0005 hardening 2).
    fn extract_blob_if_needed(&self, entry: &mut Entry) -> Result<(), Error> {
        if entry.blob.is_some() || entry.payload.is_null() {
            return Ok(());
        }
        let bytes = entry.payload.to_string().into_bytes();
        if u64::try_from(bytes.len()).expect("payload size, well under u64::MAX")
            <= DEFAULT_BLOB_THRESHOLD
        {
            return Ok(());
        }
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
        let compressed = zstd::encode_all(&bytes[..], ZSTD_LEVEL)?;
        fs::create_dir_all(self.root.join("blobs"))?;
        fs::write(self.blob_path(&entry.id), compressed)?;
        entry.payload = Value::Null;
        entry.blob = Some(BlobRef {
            id: entry.id.clone(),
            size: u64::try_from(bytes.len()).expect("payload size, well under u64::MAX"),
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
        let id = self.mint_id();
        self.append_entry(&id, kind, payload, parent)
    }

    /// Append under a pre-minted id (ADR-0008): a re-emitted item (a
    /// streaming assistant snapshot, a tool's call→result) keeps the id it
    /// was first issued, so the wire's upsert and the file's line are one
    /// object.
    pub fn append_entry(
        &mut self,
        id: &str,
        kind: &str,
        payload: Value,
        parent: Option<&str>,
    ) -> Result<Entry, Error> {
        let entry = self.append_line(self.new_entry(id, kind, payload, None), parent)?;
        if let Some(hook) = &self.entry_event_hook {
            hook(&entry);
        }
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
        // Line 1 is the header; the leaf is the last entry line after it
        // (a torn tail is not an entry — `entry_lines` drops it).
        let lines: Vec<&str> = Self::entry_lines(&raw);
        let Some(line) = lines.iter().skip(1).rev().find(|l| !l.is_empty()) else {
            return Ok(None);
        };
        let entry: Entry = line.parse::<Entry>()?;
        entry.verify(lines.len())?;
        Ok(Some(entry))
    }

    /// A unique temp path for an atomic rewrite (note 3): a shared fixed
    /// name lets two concurrently dispatched commands interleave on one
    /// file; the nanosecond clock keeps the names apart.
    fn rewrite_tmp(&self) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        self.path().with_file_name(format!(
            "{}.tmp-{nanos:016x}",
            self.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    }

    /// Shared body of the `set_*` header mutators: read the header line,
    /// apply `f`, atomically rewrite just that line (temp-file rename).
    fn update_header(&mut self, f: impl FnOnce(&mut Header)) -> Result<(), Error> {
        self.ensure_open()?;
        let raw = fs::read_to_string(self.path())?;
        let mut lines: Vec<&str> = raw.split('\n').collect();
        let mut header: Header = serde_json::from_str(lines[0])?;
        f(&mut header);
        let new_header = serde_json::to_string(&header)?;
        lines[0] = &new_header;
        let joined = lines.join("\n");
        let tmp = self.rewrite_tmp();
        fs::write(&tmp, &joined)?;
        fs::rename(&tmp, self.path())?;
        Ok(())
    }

    /// Persist an explicit branch choice (the GUI switches the active
    /// branch); only the header line's content changes, written atomically
    /// via temp-file rename.
    pub fn set_leaf(&mut self, leaf_id: &str) -> Result<(), Error> {
        self.ensure_open()?;
        if !self.ids.contains(leaf_id) {
            return Err(Error::Other(format!("unknown leaf {leaf_id}")));
        }
        self.update_header(|h| h.leaf = Some(leaf_id.to_owned()))?;
        self.leaf = Some(leaf_id.to_owned());
        Ok(())
    }

    /// Set the session's readable name; like `set_leaf` this rewrites only
    /// the header line, atomically via temp-file rename.
    pub fn set_title(&mut self, title: &str) -> Result<(), Error> {
        self.update_header(|h| h.title = Some(title.to_owned()))?;
        self.title = Some(title.to_owned());
        Ok(())
    }

    /// The creator session (a sub-agent's parent, ADR-0001); written into
    /// the header so the link survives restarts.
    pub fn set_parent(&mut self, parent: &str) -> Result<(), Error> {
        self.update_header(|h| h.parent = Some(parent.to_owned()))?;
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
        id.clone_into(&mut header.id);
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
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{seeded, store};
    use crate::session::{FILE_VERSION, Header, Value};

    #[test]
    fn oversized_payload_becomes_a_sidecar_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(tmp.path(), "s1");
        s.create().unwrap();
        let payload = Value::String("x".repeat(120_000));
        let e = s.append("tool_result", payload.clone(), None).unwrap();
        assert!(e.payload.is_null());
        let blob = e.blob.as_ref().unwrap();
        assert_eq!(blob.size, 120_002);
        assert!(s.blob_path(&e.id).exists());
        let decoded = s.resolve_blob(blob).unwrap();
        assert_eq!(decoded, serde_json::to_string(&payload).unwrap().as_bytes());
        // The on-disk line carries a pointer, not the payload.
        let content = fs::read_to_string(s.path()).unwrap();
        let line = content.lines().last().unwrap();
        assert!(!line.contains(&"x".repeat(50)));
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
}
