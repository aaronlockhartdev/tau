use super::{Path, PathBuf, ToolOutput, ToolResult, hashline};
use base64::Engine;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// The 1 MiB output cap on a `read` (item 8): a page that would exceed it
/// is cut at the last whole row that fits, and the pagination hint carries
/// the cap so the model knows why the page is short.
const READ_MAX_BYTES: usize = 1024 * 1024;
/// A line over 100 KiB is not shown (and not served): its anchor alone would
/// not fit the model's useful context; the hint says how to inspect it.
const READ_MAX_LINE_BYTES: usize = 100 * 1024;

pub(super) fn read(
    cwd: &Path,
    args: &serde_json::Value,
    image_max_bytes: Option<u64>,
    registry: Option<&mut hashline::registry::Registry>,
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
    // #207 §6: sniff BOM / UTF-16 and decode to UTF-8 (BOM stripped); a
    // BOM-less non-UTF-8 file is a loud refusal, as before.
    let raw = match hashline::encoding::sniff(&bytes) {
        Ok((text, _)) => text,
        Err(e) => {
            return ToolResult::err(format!("read: cannot read {}: {e}", path.display()).into());
        }
    };
    let content = hashline::normalize(&raw);
    let path_str = path.display().to_string();
    let outcome = match read_page(&path_str, &content, &raw, args, registry) {
        Ok(o) => o,
        Err(e) => return ToolResult::err(e.into()),
    };
    // #243: the GUI card renders the anchor-stripped twin (the anchors are a
    // model↔tool protocol, not for a human); the model gets the anchored page.
    ToolResult::ok_with_detail(outcome.text.into(), outcome.stripped.into())
}

/// The text path of `read`: allocate the file's anchors (the registry
/// reclaims LRU-cold files past the cap, item 2), resolve an anchor or
/// numeric offset, page with a byte cap, and serve the shown rows
/// (item 8: the pagination hint offers the last shown anchor as the next
/// offset; >100 KiB lines are marked and not served).
fn read_page(
    path: &str,
    content: &str,
    _raw: &str,
    args: &serde_json::Value,
    mut registry: Option<&mut hashline::registry::Registry>,
) -> Result<ReadPage, String> {
    let offset_arg = args
        .get("offset")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok());

    let (rows, reclaim) = match registry.as_deref_mut() {
        Some(reg) => {
            let (hashes, reclaim) = reg
                .allocate(path, content)
                .map_err(|e| format!("read: {e}"))?;
            (hashes, reclaim)
        }
        None => (
            hashline::line_hashes(content).map_err(|e| format!("read: {e}"))?,
            None,
        ),
    };
    let total = rows.len();

    let start_line = match &offset_arg {
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) => {
            s.parse::<usize>().map_or(1, |n| n.max(1))
        }
        Some(anchor) => match registry.as_deref_mut() {
            Some(reg) => {
                let zero = reg.resolve_offset(anchor, path, &rows)?;
                zero + 1
            }
            None => (0..rows.len())
                .find(|&i| rows[i] == *anchor)
                .map_or(1, |i| i + 1),
        },
        None => 1,
    };

    if start_line > total {
        let hint = format!(
            "Offset {start_line} is beyond end of file ({total} lines total). Use offset=1 to read from the start, or offset={total} to read the last line."
        );
        // A hint-only page has no anchored rows: the card shows the same text.
        return Ok(ReadPage {
            text: hint.clone(),
            stripped: hint,
        });
    }

    let lines: Vec<String> = content.split('\n').map(str::to_owned).collect();
    let end_line = limit.map_or(total, |l| (start_line - 1 + l).min(total));
    let selected = &rows[start_line - 1..end_line];
    let shown_lines = &lines[start_line - 1..];

    let page = render_page(path, selected, shown_lines, start_line);
    let shown = page.out.len();
    if let Some(reg) = registry {
        reg.serve(path, &page.served);
    }

    // #243: the anchored rows and their stripped twin are built together.
    let reclaim = reclaim.or(None);
    Ok(assemble_page_text(page, shown, start_line, total, reclaim))
}

/// The rendered page's two texts (#243): the model-facing anchored rows plus
/// hints, and the anchor-stripped twin the GUI card renders. The stripped rows
/// arrive pre-stripped from [`render_page`] (built row-by-row, where the anchor
/// and content are already separate); this joins them and appends the same
/// truncation/oversized/reclaim hints to both, so the card and the model's page
/// never drift.
fn assemble_page_text(
    page: PageRender,
    shown: usize,
    start_line: usize,
    total: usize,
    reclaim: Option<String>,
) -> ReadPage {
    let PageRender {
        out,
        stripped,
        served,
        oversized,
        out_bytes,
    } = page;
    let mut text = out.join("\n");
    let mut stripped = stripped.join("\n");
    let end_display = start_line + shown.saturating_sub(1);
    if shown == 0 {
        // The byte cap cut the page before any row fit.
        let hint = "\n… truncated (output cap 1 MiB)";
        text.push_str(hint);
        stripped.push_str(hint);
    } else if end_display < total {
        let last_anchor = served.last().map(|(h, _)| h.as_str()).unwrap_or_default();
        let hint = format!(
            "\n… truncated (showing lines {start_line}–{end_display} of {total} — {out_bytes} bytes). Use offset={last_anchor} to continue."
        );
        text.push_str(&hint);
        // The hint's next-page offset is the anchor — meaningless to a human,
        // but the line-count context is not: the stripped twin keeps it as-is.
        stripped.push_str(&hint);
    }
    if !oversized.is_empty() {
        let listed: Vec<String> = oversized
            .iter()
            .take(10)
            .map(std::string::ToString::to_string)
            .collect();
        let hint = format!(
            "\n[{} oversized line(s) over 100 KiB not shown: lines {}]",
            oversized.len(),
            listed.join(", ")
        );
        text.push_str(&hint);
        stripped.push_str(&hint);
    }
    let reclaim = reclaim.or(None);
    if let Some(w) = reclaim {
        text = format!("{w}\n{text}");
        stripped = format!("{w}\n{stripped}");
    }
    ReadPage { text, stripped }
}
/// The output of a rendered read page: the model-facing anchored rows plus
/// their anchor-stripped twin (#243), the served (anchor, checksum) baseline,
/// the oversized line numbers, and the byte count of the rendered output.
struct PageRender {
    out: Vec<String>,
    stripped: Vec<String>,
    served: Vec<(String, String)>,
    oversized: Vec<usize>,
    out_bytes: usize,
}

/// Render a page of `rows` (the file's anchors for the selected range) into
/// the model-facing lines: `HASH│content` per row, in file order. Rows over
/// [`READ_MAX_LINE_BYTES`] are not shown (their anchor alone would burn
/// context); they are listed in `oversized` so the caller can hint at them.
/// `served` is the (anchor, line checksum) pair per *shown* row — the
/// integrity baseline `read` registers (item 1). The 1 MiB page cap (item 8)
/// drops rows from the end once the accumulated output would exceed it.
fn render_page(
    _path: &str,
    selected: &[String],
    lines: &[String],
    start_line: usize,
) -> PageRender {
    let mut out: Vec<String> = Vec::new();
    let mut stripped: Vec<String> = Vec::new();
    let mut served: Vec<(String, String)> = Vec::new();
    let mut oversized: Vec<usize> = Vec::new();
    let mut out_bytes = 0usize;
    for (i, pair) in selected.iter().zip(lines.iter()).enumerate() {
        let (hash, line) = pair;
        if line.len() > READ_MAX_LINE_BYTES {
            oversized.push(start_line + i);
            continue;
        }
        let row = format!("{}{}{line}", hash, hashline::SEP);
        if out_bytes.saturating_add(row.len()) > READ_MAX_BYTES {
            break;
        }
        out_bytes += row.len();
        out.push(row);
        // #243: the anchor-stripped twin, built row-by-row as the anchored
        // rows are (the card's `ui_detail` renders these; the model's text
        // joins `out`). The anchor and separator are never in `line`.
        stripped.push(line.clone());
        served.push((hash.clone(), hashline::line_checksum(line)));
    }
    PageRender {
        out,
        stripped,
        served,
        oversized,
        out_bytes,
    }
}
/// A `read` page: the model-facing text (hash-anchored rows + hints) and its
/// anchor-stripped twin for the GUI card (the `ui_detail` channel, #243):
/// the same rows without the `HASH│` prefixes, the same hints — the anchors
/// are a model↔tool protocol, not information for a human.
#[derive(Debug)]
pub(super) struct ReadPage {
    pub text: String,
    pub stripped: String,
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

pub(super) async fn write(
    cwd: &Path,
    args: &serde_json::Value,
    progress: Option<&crate::tools::ToolProgress>,
) -> ToolResult {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResult::err("write: missing \"path\"".into());
    };
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return ToolResult::err("write: missing \"content\"".into());
    };
    // Issue #197: a large write streams its content in-flight. Since #243
    // the final result is the ack only, so the in-flight content rides the
    // `ui_detail` channel — the live card and the final card render from the
    // same place the model never sees.
    if let Some(p) = progress {
        p.report_detail(content);
    }
    let path = resolve(cwd, path);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ToolResult::err(format!("write: cannot create {}: {e}", parent.display()).into());
    }
    match write_atomic(&path, content) {
        Ok(()) => {
            let lines = if content.is_empty() {
                0
            } else {
                content.matches('\n').count() + 1
            };
            // #184's ack to the model; #243: the echoed content is UI-only.
            ToolResult::ok_with_detail(
                format!(
                    "wrote {} ({} bytes, {} lines)",
                    path.display(),
                    content.len(),
                    lines
                )
                .into(),
                content.to_owned().into(),
            )
        }
        Err(e) => ToolResult::err(format!("write: {e}").into()),
    }
}

/// Overwrite a file without a torn intermediate state: unique temp file in
/// the target directory + fsync + rename (+ best-effort directory sync),
/// preserving an existing file's mode — the reference scheme's `writeAtomic`
/// (spec §5.4): a crash mid-rewrite must never truncate a complete file.
/// The file's current permission mode (`None` if it does not exist or the
/// metadata read fails) — carried across an atomic rewrite so an `edit`
/// does not reset a file's mode (#207 §6).
fn current_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(unix)]
pub(super) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let existing_mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    write_atomic_bytes(path, content.as_bytes(), existing_mode)
}

/// Write raw bytes (a `write`/`edit` that must preserve a non-UTF-8 file's
/// on-disk encoding, #207 §6): the atomic temp+rename path over a byte slice.
#[cfg(unix)]
pub(super) fn write_atomic_bytes(
    path: &Path,
    bytes: &[u8],
    existing_mode: Option<u32>,
) -> std::io::Result<()> {
    write_atomic_bytes_inner(path, bytes, existing_mode)
}

#[cfg(not(unix))]
pub(super) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, content.as_bytes(), None)
}

#[cfg(not(unix))]
pub(super) fn write_atomic_bytes(
    path: &Path,
    bytes: &[u8],
    _existing_mode: Option<u32>,
) -> std::io::Result<()> {
    write_atomic_bytes_inner(path, bytes, None)
}

fn write_atomic_bytes_inner(
    path: &Path,
    bytes: &[u8],
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
        file.write_all(bytes)?;
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
/// Parse and validate the `path` + `edits` arguments of an `edit` call.
fn parse_edit_args(args: &serde_json::Value) -> Result<(String, Vec<hashline::Edit>), String> {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return Err("edit: missing \"path\"".to_owned());
    };
    let Some(edits_json) = args.get("edits").and_then(|v| v.as_array()) else {
        return Err("edit: \"edits\" array is required".to_owned());
    };
    if edits_json.is_empty() {
        return Err("edit: \"edits\" must contain at least one edit".to_owned());
    }
    let mut edits = Vec::with_capacity(edits_json.len());
    for (i, item) in edits_json.iter().enumerate() {
        let (Some(from), Some(to), Some(content)) = (
            item.get("from").and_then(|v| v.as_str()),
            item.get("to").and_then(|v| v.as_str()),
            item.get("content").and_then(|v| v.as_str()),
        ) else {
            return Err(format!(
                "edit: edits[{i}] requires \"from\", \"to\", and \"content\""
            ));
        };
        edits.push(hashline::Edit {
            from: from.to_owned(),
            to: to.to_owned(),
            content: content.to_owned(),
        });
    }
    Ok((path.to_owned(), edits))
}

#[allow(clippy::too_many_lines)] // the #243 ack + detail tail pushed it 3 over
pub(super) fn edit(
    cwd: &Path,
    args: &serde_json::Value,
    mut registry: Option<&mut hashline::registry::Registry>,
    strict: bool,
) -> ToolResult {
    let (path, edits) = match parse_edit_args(args) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.into()),
    };
    let path = resolve(cwd, &path);
    let path_str = path.display().to_string();
    // (3) Drift re-read at commit: the file is read twice — once to plan,
    // once to commit — and a change between the two aborts the write.
    // #207 §6: sniff BOM / UTF-16 on the plan read; the sniffed encoding is
    // re-applied on the commit write so the file round-trips its form.
    let (raw, sniffed) = match std::fs::read(&path) {
        Ok(b) => match hashline::encoding::sniff(&b) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::err(
                    format!("edit: cannot read {}: {e}", path.display()).into(),
                );
            }
        },
        Err(e) => {
            return ToolResult::err(format!("edit: cannot read {}: {e}", path.display()).into());
        }
    };
    let original = hashline::normalize(&raw);
    let ending = hashline::line_endings::detect(&raw);
    // Demotion is the default posture (#207 §5): apply the valid members of a
    // batch, report the rest. Strict mode opts back into the atomic-reject
    // contract (a bad member rejects the whole batch).
    let mut opts = hashline::PlanOptions {
        strict,
        demote: !strict,
        ..Default::default()
    };
    if let Some(reg) = registry.as_ref() {
        if let Some(served) = reg.served_map(&path_str) {
            opts.served = Some(served.clone());
        }
        // Resolve against the registry's file anchors — the single source of
        // truth — not a re-derived `line_hashes` (which would disagree).
        if let Some(anchors) = reg.anchors_for(&path_str) {
            opts.hashes = Some(anchors.to_vec());
        }
    }
    let planned = match hashline::plan_edits(&original, ending, &edits, &opts) {
        Ok(p) => p,
        // A stale range: adopt the diagnostic's fresh anchors in place so the
        // model can retry with them without a re-read.
        Err(hashline::EditError::RangeStale {
            fresh, ref rows, ..
        }) => {
            if let Some(reg) = registry.as_mut() {
                reg.adopt(&path_str, &fresh);
            }
            // The full diagnostic (with the fresh anchor rows inline) plus a
            // note that the anchors are now registered for a no-read retry.
            let msg = format!(
                "edit: [E_RANGE_STALE] the range no longer matches what was shown; the file changed after the read. Current range with fresh anchors:\n\n{}\n\nThe fresh anchors above are now registered — retry the edit with them, no re-read needed.",
                rows.join("\n")
            );
            return ToolResult::err(msg.into());
        }
        Err(e) => return ToolResult::err(format!("edit: {e}").into()),
    };

    // The drift re-read: the plan is only as good as the file it planned
    // against. Re-read and compare the normalized content; a change means
    // the anchors (and the integrity baseline) are stale — abort, write
    // nothing.
    let raw2 = match std::fs::read(&path) {
        Ok(b) => match hashline::encoding::sniff(&b) {
            Ok((text, _)) => text,
            Err(_) => {
                return ToolResult::err(
                    format!(
                        "edit: cannot re-read {}: not valid UTF-8; the file changed between reads — aborted, nothing written",
                        path.display()
                    )
                    .into(),
                );
            }
        },
        Err(_) => {
            return ToolResult::err(
                format!(
                    "edit: [E_OP_ABORTED] the file at {} was deleted or became unreadable between the plan and the write; nothing was written",
                    path.display()
                )
                .into(),
            );
        }
    };
    if hashline::normalize(&raw2) != original {
        return ToolResult::err(
            format!(
                "edit: [E_OP_ABORTED] the file at {} changed between planning and writing the edit; nothing was written. Re-read the file for fresh anchors.",
                path.display()
            )
            .into(),
        );
    }

    // #207 §6: re-encode to the file's sniffed form (BOM / UTF-16 preserved).
    let content = hashline::assemble(&planned);
    let bytes = hashline::encoding::encode(&content, &sniffed);
    if let Err(e) = write_atomic_bytes(&path, &bytes, current_mode(&path)) {
        return ToolResult::err(format!("edit: cannot write {}: {e}", path.display()).into());
    }
    // The write succeeded: the registry now owns the new anchor set, and the
    // shown baseline is refreshed to what the model was just told.
    if let Some(reg) = registry {
        reg.refresh_owned(&path_str, &hashline::normalize(&content), &planned.hashes);
        let served_rows: Vec<(String, String)> = planned
            .hashes
            .iter()
            .zip(planned.lines.iter())
            .map(|(h, l)| (h.clone(), hashline::line_checksum(l)))
            .collect();
        reg.serve(&path_str, &served_rows);
    }

    // #243: the model gets the one-line ack; the bounded diff rides the GUI-only channel.
    ToolResult::ok_with_detail(
        super::edit_diff::model_ack(&path_str, &original, &planned).into(),
        super::edit_diff::diff_output(&path_str, &original, &planned).into(),
    )
}
/// A sane PATH for a bash child (#172): the inherited PATH (if any) plus the
/// common per-user tool dirs, de-duplicated. The app is launched from a GUI,
/// so its ambient PATH is a minimal launchd set that misses `~/.cargo/bin`,
/// Homebrew, nvm, and friends.
fn bash_path() -> String {
    let mut parts: Vec<String> = std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .map(str::to_owned)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !parts.contains(&dir.to_owned()) {
            parts.push(dir.to_owned());
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for sub in ["cargo/bin", "local/bin"] {
            let dir = home.join(sub).to_string_lossy().into_owned();
            if !parts.contains(&dir) {
                parts.push(dir);
            }
        }
    }
    parts.join(":")
}

/// Resolve once `flag` is set. A stop is a plain `AtomicBool` (no channel),
/// so this polls it; the interval is short enough to feel immediate (#174).
async fn wait_for_flag(flag: &AtomicBool) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Read a child stream to EOF, reporting each chunk through `progress` as it
/// arrives (issue #197) and returning the full text. Runs inside the caller's
/// `join!` (not a spawned task) so it may borrow `progress`.
async fn read_stream(
    progress: Option<&crate::tools::ToolProgress>,
    mut stream: impl tokio::io::AsyncRead + Unpin,
) -> String {
    use tokio::io::AsyncReadExt;
    let mut full = Vec::new();
    // A heap buffer (not a stack array): an 8 KiB `chunk` in the future would
    // push the two reader futures — and every caller's — over clippy's 16 KiB
    // `large_futures` threshold.
    let mut chunk = vec![0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                full.extend_from_slice(&chunk[..n]);
                if let Some(p) = progress {
                    p.report(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
            // The child was killed (stop/timeout) or the stream errored: stop
            // draining and return what we have.
            _ => break,
        }
    }
    String::from_utf8_lossy(&full).into_owned()
}

/// Run a bash command with a sane PATH (#172); `stop`, once set, kills the
/// running child (#174). `progress`, when present, streams stdout/stderr to
/// the entry upsert hook as they arrive (issue #197); the coalesced in-flight
/// upserts are wire-only, and the final result below replaces them.
#[allow(clippy::too_many_lines)] // one streaming child: spawn, read, race stop
pub(super) async fn bash_inner(
    cwd: &Path,
    args: &serde_json::Value,
    stop: &AtomicBool,
    progress: Option<&crate::tools::ToolProgress>,
) -> ToolResult {
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
        .env("PATH", bash_path())
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // A dropped child (timeout or stop) is killed, not left behind.
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return ToolResult::err(format!("bash: {e}").into()),
    };
    // Take the owned pipes (leaving `child` for `wait`) and drain them while
    // the child runs, reporting each chunk in-flight.
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    // Race: (a) both pipes hit EOF (child exited), (b) the timeout elapses
    // (explicit `child.kill()` — the readers would otherwise block forever on
    // live pipes), (c) the stop flag (the child drops → kill_on_drop).
    let (streams, outcome) = tokio::select! {
        r = async {
            tokio::join!(
                read_stream(progress, stdout),
                read_stream(progress, stderr),
            )
        } => (Some(r), None),
        () = async {
            tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)).await;
            child.kill().await.ok();
        } => (None, Some("timeout")),
        () = wait_for_flag(stop) => (None, Some("stop")),
    };
    match outcome {
        Some("stop") => {
            // A user stop cancels the in-flight command (ticket #222): the
            // child is killed on drop, the result is a cancellation, not a
            // failure of the command itself.
            return ToolResult::cancelled("bash: cancelled by user stop".into());
        }
        Some("timeout") => {
            return ToolResult::err(format!("bash: timed out after {timeout_secs}s").into());
        }
        None => {}
        Some(_) => {
            // `&str` is not an exhaustive set; a leg other than the two named
            // above is an internal invariant break, surfaced, not a panic.
            return ToolResult::err("bash: internal error: unknown outcome".into());
        }
    }
    let (stdout, stderr) = streams.expect("the join leg always yields its pair");
    let code = match child.wait().await {
        Ok(status) => status.code().unwrap_or(-1),
        Err(e) => return ToolResult::err(format!("bash: {e}").into()),
    };
    let mut out = format!("exit {code}");
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
        ui_detail: None,
        ok: code == 0,
        cancelled: false,
    }
}

/// The `bash` tool as the tests see it: a command with no stop signal (a
/// fresh, never-set flag). The live dispatch path (`surface.rs`) calls
/// [`bash_inner`] with the session's real stop flag.
#[cfg(test)]
pub(super) async fn bash(cwd: &Path, args: &serde_json::Value) -> ToolResult {
    let no_stop = AtomicBool::new(false);
    bash_inner(cwd, args, &no_stop, None).await
}
