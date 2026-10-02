//! Workspace listing: the on-disk sessions of a workspace (ADR-0005).

use std::fs;
use std::path::Path;

use tau_protocol::snapshot::SessionMeta;

use super::{Header, SessionStore};

/// The workspace's on-disk sessions (ADR-0005): the `sessions/` files,
/// plus the `archive/` dir's compressed copies (the `archived` flag
/// set), one bounded header-line read per file, so a cold listing stays
/// cheap. A file whose header is missing, malformed, or names a
/// different id is skipped. The `workspace` field stays empty: the
/// caller (the harness) stamps the id it owns.
#[must_use]
#[allow(clippy::case_sensitive_file_extension_comparisons)] // tau writes `.jsonl`/`.jsonl.zst` lowercase; case-insensitive matching would change which files list
pub fn list_workspace(project_root: &Path) -> Vec<SessionMeta> {
    let mut out = Vec::new();
    for (dir, archived) in [
        (project_root.join(".tau").join("sessions"), false),
        (project_root.join(".tau").join("archive"), true),
    ] {
        let Ok(names) = fs::read_dir(&dir).map(|d| {
            d.filter_map(std::result::Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".jsonl") || n.ends_with(".jsonl.zst"))
                .collect::<Vec<_>>()
        }) else {
            continue;
        };
        for name in names {
            let Some(id) = name
                .strip_suffix(".jsonl.zst")
                .or_else(|| name.strip_suffix(".jsonl"))
                .map(std::borrow::ToOwned::to_owned)
            else {
                continue;
            };
            let first = if name.ends_with(".jsonl") {
                let Ok(line) = fs::File::open(dir.join(&name)).and_then(|f| {
                    use std::io::BufRead;
                    let mut line = String::new();
                    std::io::BufReader::new(f)
                        .read_line(&mut line)
                        .map(|_| line)
                }) else {
                    continue;
                };
                line.trim().to_owned()
            } else {
                let store = SessionStore::for_workspace(project_root, &id);
                match store.archive_header_line() {
                    Ok(line) => line,
                    Err(_) => continue,
                }
            };
            let Ok(h) = serde_json::from_str::<Header>(&first) else {
                continue;
            };
            if h.kind != "session" || h.id != id {
                continue;
            }
            out.push(SessionMeta {
                id: h.id,
                workspace: String::new(),
                title: h.title,
                parent: h.parent,
                created: h.created,
                leaf: h.leaf,
                model: None,
                usage: None,
                archived,
            });
        }
    }
    out
}
