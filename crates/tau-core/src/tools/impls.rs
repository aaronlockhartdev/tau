use super::*;
use base64::Engine;

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

pub(super) async fn read(
    cwd: &Path,
    args: &serde_json::Value,
    image_max_bytes: Option<u64>,
) -> ToolOutput {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return "read: missing \"path\"".into();
    };
    let path = resolve(cwd, path);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return format!("read: cannot read {}: {e}", path.display()).into(),
    };
    // #34: an image is content for a vision endpoint, not UTF-8 text.
    if let Some(media_type) = image_media_type(&bytes) {
        // #36: a bounded read — an oversized image is a loud refusal, not a
        // silent drop (stateless, it would ride every request).
        if let Some(cap) = image_max_bytes
            && bytes.len() as u64 > cap
        {
            return format!(
                "read: {} is a {media_type} image of {} bytes, over the {cap}-byte cap — not returned",
                path.display(),
                bytes.len()
            )
            .into();
        }
        return ToolOutput::Image(tau_protocol::payload::ImageBlock {
            kind: "image".into(),
            media_type: media_type.to_owned(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        });
    }
    let content = match std::str::from_utf8(&bytes) {
        Ok(c) => hashline::normalize(c),
        // The same diagnostic `read_to_string` produced: the text path stays
        // byte-for-byte (#34).
        Err(_) => {
            return format!(
                "read: cannot read {}: stream did not contain valid UTF-8",
                path.display()
            )
            .into();
        }
    };
    let rows = match hashline::render(&content) {
        Ok(rows) => rows,
        Err(e) => return e.to_string().into(),
    };
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let total = rows.len();
    let shown: Vec<String> = rows
        .iter()
        .skip(offset - 1)
        .take(limit.unwrap_or(usize::MAX))
        .cloned()
        .collect();
    let mut out = shown.join("\n");
    if offset - 1 + shown.len() < total {
        out.push_str(&format!(
            "\n… truncated (showing lines {}–{} of {})",
            offset,
            offset - 1 + shown.len(),
            total
        ));
    }
    out.into()
}

/// The image type by magic bytes (#34): the extension is not authoritative —
/// a lying one would ship binary junk to the model as text.
pub(super) fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 14 && bytes.starts_with(b"BM") && bytes[6..8] == [0, 0] {
        Some("image/bmp")
    } else if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
        Some("image/tiff")
    } else {
        None
    }
}

pub(super) async fn write(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return "write: missing \"path\"".into();
    };
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return "write: missing \"content\"".into();
    };
    let path = resolve(cwd, path);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return format!("write: cannot create {}: {e}", parent.display()).into();
    }
    match write_atomic(&path, content) {
        Ok(()) => format!("wrote {} ({} bytes)", path.display(), content.len()).into(),
        Err(e) => format!("write: {e}").into(),
    }
}
/// Overwrite a file without a torn intermediate state: unique temp file in
/// the target directory + fsync + rename (+ best-effort directory sync),
/// preserving an existing file's mode — the reference scheme's `writeAtomic`
/// (spec §5.4): a crash mid-rewrite must never truncate a complete file.
#[cfg(unix)]
pub(super) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let existing_mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    write_atomic_inner(path, content, existing_mode)
}

#[cfg(not(unix))]
pub(super) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    write_atomic_inner(path, content, None)
}

fn write_atomic_inner(
    path: &Path,
    content: &str,
    existing_mode: Option<u32>,
) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = dir.to_path_buf();
    {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        temp = temp.join(format!(".tmp-{}-{nanos}", std::process::id()));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        use std::io::Write;
        file.write_all(content.as_bytes())?;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = existing_mode {
            let mut perm = file.metadata()?.permissions().clone();
            perm.set_mode(mode);
            file.set_permissions(perm)?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        // Directory sync is a durability optimization, best-effort (the rename
        // already committed the file).
        if let Ok(dir_file) = std::fs::File::open(dir) {
            let _ = dir_file.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub(super) async fn edit(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return "edit: missing \"path\"".into();
    };
    let (Some(from), Some(to), Some(content)) = (
        args.get("from").and_then(|v| v.as_str()),
        args.get("to").and_then(|v| v.as_str()),
        args.get("content").and_then(|v| v.as_str()),
    ) else {
        return "edit: \"from\", \"to\", and \"content\" are required".into();
    };
    let path = resolve(cwd, path);
    let original = match std::fs::read_to_string(&path) {
        Ok(c) => hashline::normalize(&c),
        Err(e) => return format!("edit: cannot read {}: {e}", path.display()).into(),
    };
    let edited = match hashline::apply_edit(
        &original,
        &hashline::Edit {
            from: from.to_owned(),
            to: to.to_owned(),
            content: content.to_owned(),
        },
    ) {
        Ok(e) => e,
        Err(e) => return e.to_string().into(),
    };
    if let Err(e) = write_atomic(&path, &edited.content) {
        return format!("edit: cannot write {}: {e}", path.display()).into();
    }
    // Bounded output (the reference returns the changed region, not the
    // whole file): the changed lines plus two context lines per side, with
    // their fresh anchors, plus the result line count.
    if edited.first_changed > edited.last_changed {
        return format!("edited {} (no change)", path.display()).into();
    }
    let rows = hashline::render(&edited.content).unwrap_or_default();
    let lo = edited.first_changed.saturating_sub(1).saturating_sub(2);
    let hi = (edited.last_changed + 2).min(rows.len());
    (format!(
        "edited {} (lines {}–{} of {})",
        path.display(),
        edited.first_changed,
        edited.last_changed,
        rows.len()
    ) + "\n"
        + &rows[lo..hi].join("\n"))
        .into()
}

pub(super) async fn bash(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
    let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
        return "bash: missing \"command\"".into();
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60)
        .max(1);
    let child = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // A timed-out (dropped) child is killed, not left behind.
        .kill_on_drop(true)
        .spawn();
    let output = match child {
        Ok(child) => match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return format!("bash: {e}").into(),
            // The timeout dropped the (now owned) child; kill_on_drop fired,
            // so a timed-out command cannot keep running and mutate files.
            Err(_) => return format!("bash: timed out after {timeout_secs}s").into(),
        },
        Err(e) => return format!("bash: {e}").into(),
    };
    let mut out = format!("exit {}", output.status.code().unwrap_or(-1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.is_empty() {
        out.push_str("\n--- stdout ---\n");
        out.push_str(&stdout);
    }
    if !stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(&stderr);
    }
    out.into()
}
