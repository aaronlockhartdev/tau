use super::*;

impl SessionStore {
    pub(super) fn blob_path(&self, id: &str) -> PathBuf {
        self.root.join("blobs").join(id)
    }

    pub fn archive_path(&self) -> PathBuf {
        self.root
            .join("archive")
            .join(format!("{}.jsonl.zst", self.id))
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{seeded, store};

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
