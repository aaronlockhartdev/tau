use super::{Path, PathBuf, ToolOutput, ToolResult, hashline};
use base64::Engine;
use std::fmt::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
) -> ToolResult {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResult::err("read: missing \"path\"".into());
    };
    let path = resolve(cwd, path);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return ToolResult::err(format!("read: cannot read {}: {e}", path.display()).into());
        }
    };
    // #34: an image is content for a vision endpoint, not UTF-8 text.
    if let Some(media_type) = image_media_type(&bytes) {
        // #36: a bounded read — an oversized image is a loud refusal, not a
        // silent drop (stateless, it would ride every request).
        if let Some(cap) = image_max_bytes
            && u64::try_from(bytes.len()).expect("byte length, well under u64::MAX") > cap
        {
            return ToolResult::err(format!(
                "read: {} is a {media_type} image of {} bytes, over the {cap}-byte cap — not returned",
                path.display(),
                bytes.len()
            )
            .into());
        }
        return ToolResult::ok(ToolOutput::Image(tau_protocol::payload::ImageBlock {
            kind: "image".into(),
            media_type: media_type.to_owned(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }));
    }
    let content = match std::str::from_utf8(&bytes) {
        Ok(c) => hashline::normalize(c),
        // The same diagnostic `read_to_string` produced: the text path stays
        // byte-for-byte (#34).
        Err(_) => {
            return ToolResult::err(
                format!(
                    "read: cannot read {}: stream did not contain valid UTF-8",
                    path.display()
                )
                .into(),
            );
        }
    };
    let rows = match hashline::render(&content) {
        Ok(rows) => rows,
        Err(e) => return ToolResult::err(format!("read: {e}").into()),
    };
    let offset = usize::try_from(
        args.get("offset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1)
            .max(1),
    )
    .expect("tool offset fits in usize on the supported 64-bit targets");
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok());
    let total = rows.len();
    let shown: Vec<String> = rows
        .iter()
        .skip(offset - 1)
        .take(limit.unwrap_or(usize::MAX))
        .cloned()
        .collect();
    let mut out = shown.join("\n");
    if offset - 1 + shown.len() < total {
        let _ = write!(
            out,
            "\n… truncated (showing lines {}–{} of {})",
            offset,
            offset - 1 + shown.len(),
            total
        );
    }
    ToolResult::ok(out.into())
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

pub(super) async fn write(cwd: &Path, args: &serde_json::Value) -> ToolResult {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResult::err("write: missing \"path\"".into());
    };
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return ToolResult::err("write: missing \"content\"".into());
    };
    let path = resolve(cwd, path);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ToolResult::err(format!("write: cannot create {}: {e}", parent.display()).into());
    }
    match write_atomic(&path, content) {
        Ok(()) => {
            ToolResult::ok(format!("wrote {} ({} bytes)", path.display(), content.len()).into())
        }
        Err(e) => ToolResult::err(format!("write: {e}").into()),
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
            .map_or(0, |d| d.as_nanos());
        temp = temp.join(format!(".tmp-{}-{nanos}", std::process::id()));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        use std::io::Write;
        file.write_all(content.as_bytes())?;
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

pub(super) async fn edit(cwd: &Path, args: &serde_json::Value) -> ToolResult {
    const CAP: usize = 200;
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResult::err("edit: missing \"path\"".into());
    };
    let (Some(from), Some(to), Some(content)) = (
        args.get("from").and_then(|v| v.as_str()),
        args.get("to").and_then(|v| v.as_str()),
        args.get("content").and_then(|v| v.as_str()),
    ) else {
        return ToolResult::err("edit: \"from\", \"to\", and \"content\" are required".into());
    };
    let path = resolve(cwd, path);
    let original = match std::fs::read_to_string(&path) {
        Ok(c) => hashline::normalize(&c),
        Err(e) => {
            return ToolResult::err(format!("edit: cannot read {}: {e}", path.display()).into());
        }
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
        Err(e) => return ToolResult::err(format!("edit: {e}").into()),
    };
    if let Err(e) = write_atomic(&path, &edited.content) {
        return ToolResult::err(format!("edit: cannot write {}: {e}", path.display()).into());
    }
    // Bounded before/after diff (the reference returns the changed region,
    // not the whole file): removed lines, added lines with their fresh
    // anchors, a little unchanged context — capped so a huge edit stays small.
    if edited.first_changed > edited.last_changed {
        return ToolResult::ok(format!("edited {} (no change)", path.display()).into());
    }
    let new_lines: Vec<String> = edited.content.lines().map(str::to_owned).collect();
    let total = new_lines.len();
    // The changed region in the new file, 0-based.
    let lo = edited.first_changed.saturating_sub(1);
    let hi = edited.last_changed.min(total);
    // Change lines: removed (old) then added (new, carrying the fresh anchor).
    let mut change: Vec<String> = Vec::with_capacity(edited.removed_lines.len() + (hi - lo));
    for line in &edited.removed_lines {
        change.push(format!("-{line}"));
    }
    for (line, hash) in new_lines[lo..hi].iter().zip(edited.hashes[lo..hi].iter()) {
        change.push(format!("+ {hash} {line}"));
    }
    // Cap the body at ~200 lines; a larger change shows its head + a count.
    let mut body: Vec<String> = Vec::new();
    let mut notice: Option<String> = None;
    if change.len() > CAP {
        body.extend(change.iter().take(CAP).cloned());
        notice = Some(format!("… {} more lines changed", change.len() - CAP));
    } else {
        body.extend(change);
        // A little unchanged context for orientation, kept within the cap.
        let budget = CAP - body.len();
        let mut ctx = Vec::new();
        for line in new_lines.iter().take(lo).rev().take(2) {
            ctx.push(format!(" {line}"));
        }
        for line in new_lines.iter().skip(hi).take(2) {
            if ctx.len() >= 2 {
                break;
            }
            ctx.push(format!(" {line}"));
        }
        body.extend(ctx.into_iter().take(budget));
    }
    let summary = format!(
        "edited {} (lines {}–{} of {})",
        path.display(),
        edited.first_changed,
        edited.last_changed,
        total
    );
    let mut out = summary + "\n";
    out.push_str(&body.join("\n"));
    if let Some(n) = notice {
        out.push('\n');
        out.push_str(&n);
    }
    ToolResult::ok(out.into())
}

pub(super) async fn bash(cwd: &Path, args: &serde_json::Value) -> ToolResult {
    let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
        return ToolResult::err("bash: missing \"command\"".into());
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(serde_json::Value::as_u64)
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
            Ok(Err(e)) => return ToolResult::err(format!("bash: {e}").into()),
            // The timeout dropped the (now owned) child; kill_on_drop fired,
            // so a timed-out command cannot keep running and mutate files.
            Err(_) => {
                return ToolResult::err(format!("bash: timed out after {timeout_secs}s").into());
            }
        },
        Err(e) => return ToolResult::err(format!("bash: {e}").into()),
    };
    let code = output.status.code().unwrap_or(-1);
    let mut out = format!("exit {code}");
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
    ToolResult {
        output: out.into(),
        ok: code == 0,
    }
}
