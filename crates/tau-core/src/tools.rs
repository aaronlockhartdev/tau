//! The four core tools (spec §5.4): read / write / edit / bash. Paths are
//! absolute or workspace-relative (relative resolves against the workspace
//! root — tools are not sandboxed, ADR-0007). `read`/`edit` are
//! hash-anchored per the pi-better-edit scheme (§5.4).

use crate::hashline;
use crate::provider::{ToolKind, ToolSpec};
use serde_json::json;
use std::path::{Path, PathBuf};

/// One tool call as the model emitted it (`arguments` is raw JSON).
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// The tool's output, as the model will see it (errors are results, not
/// panics — the model reads the diagnostic and recovers, spec §5.4).
pub type ToolOutput = String;

/// The model-facing definitions of the four core tools.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "read".into(),
            description: "Read a text file as hash-anchored lines: each line is \
                 'HASH│content' (HASH = 3 chars). The hash is the anchor you pass \
                 to edit. Optional offset (1-based) and limit page long files."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "write".into(),
            description: "Write (create or replace) a text file with the given content.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "edit".into(),
            description: "Replace the line range [from, to] (inclusive) of a file \
                 with content. from/to are the 3-char hash anchors copied from read \
                 output (the 3 chars before │), bare. Stale or ambiguous anchors are \
                 rejected — re-read the file for fresh anchors. Pass empty content to \
                 delete the range."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "from", "to", "content"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "bash".into(),
            description: "Run a shell command with the workspace as the working \
                 directory. Returns the exit code, stdout, and stderr."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["command"]
            }),
        },
    ]
}

/// Dispatch one tool call. Never panics on bad input — the diagnostic is the
/// result (the model's only recovery path).
pub async fn dispatch(cwd: &Path, call: &ToolCall) -> ToolOutput {
    match call.name.as_str() {
        "read" => read(cwd, &call.args).await,
        "write" => write(cwd, &call.args).await,
        "edit" => edit(cwd, &call.args).await,
        "bash" => bash(cwd, &call.args).await,
        other => format!("unknown tool: {other}"),
    }
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

async fn read(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
    let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
        return "read: missing \"path\"".into();
    };
    let path = resolve(cwd, path);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return format!("read: cannot read {}: {e}", path.display()),
    };
    let rows = match hashline::render(&content) {
        Ok(rows) => rows,
        Err(e) => return e.to_string(),
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
    out
}

async fn write(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
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
        return format!("write: cannot create {}: {e}", parent.display());
    }
    match write_atomic(&path, content) {
        Ok(()) => format!("wrote {} ({} bytes)", path.display(), content.len()),
        Err(e) => format!("write: {e}"),
    }
}
/// Overwrite a file without a torn intermediate state: unique temp file in
/// the target directory + fsync + rename (+ best-effort directory sync),
/// preserving an existing file's mode — the reference scheme's `writeAtomic`
/// (spec §5.4): a crash mid-rewrite must never truncate a complete file.
#[cfg(unix)]
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let existing_mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    write_atomic_inner(path, content, existing_mode)
}

#[cfg(not(unix))]
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
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

async fn edit(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
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
        Ok(c) => c,
        Err(e) => return format!("edit: cannot read {}: {e}", path.display()),
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
        Err(e) => return e.to_string(),
    };
    if let Err(e) = write_atomic(&path, &edited.content) {
        return format!("edit: cannot write {}: {e}", path.display());
    }
    format!(
        "edited {} (lines {}–{})\n{}",
        path.display(),
        edited.first_changed,
        edited.last_changed,
        hashline::render(&edited.content)
            .map(|rows| rows.join("\n"))
            .unwrap_or_default()
    )
}

async fn bash(cwd: &Path, args: &serde_json::Value) -> ToolOutput {
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
        .spawn();
    let output = match child {
        Ok(child) => match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return format!("bash: {e}"),
            Err(_) => return format!("bash: timed out after {timeout_secs}s"),
        },
        Err(e) => return format!("bash: {e}"),
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_then_read_roundtrip_with_anchors() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &json!({"path": "a.txt", "content": "one\ntwo\n"}),
        )
        .await;
        let out = read(dir.path(), &json!({"path": "a.txt"})).await;
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].ends_with("one"));
        assert!(rows[1].ends_with("two"));
        let from = rows[0].split('│').next().unwrap().to_string();
        let to = rows[1].split('│').next().unwrap().to_string();
        let out = edit(
            dir.path(),
            &json!({"path": "a.txt", "from": from, "to": to, "content": "ONE AND TWO"}),
        )
        .await;
        assert!(out.starts_with("edited"), "edit failed: {out}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ONE AND TWO\n"
        );
    }

    #[tokio::test]
    async fn read_pages_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &json!({"path": "p.txt", "content": "l1\nl2\nl3\nl4\n"}),
        )
        .await;
        let out = read(
            dir.path(),
            &json!({"path": "p.txt", "offset": 2, "limit": 2}),
        )
        .await;
        assert!(out.ends_with("lines 2–3 of 4)"), "{out}");
        assert!(out.contains("l2") && out.contains("l3"));
        assert!(!out.contains("l1") && !out.contains("l4"));
    }

    #[tokio::test]
    async fn stale_anchor_diagnostic_does_not_touch_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &json!({"path": "s.txt", "content": "keep\n"})).await;
        let out = edit(
            dir.path(),
            &json!({"path": "s.txt", "from": "zzz", "to": "zzz", "content": "x"}),
        )
        .await;
        assert!(out.contains("stale anchor"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("s.txt")).unwrap(),
            "keep\n"
        );
    }

    #[tokio::test]
    async fn bash_captures_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(dir.path(), &json!({"command": "echo hello"})).await;
        assert!(out.starts_with("exit 0"), "{out}");
        assert!(out.contains("hello"), "{out}");
        let out = bash(dir.path(), &json!({"command": "echo oops >&2; exit 3"})).await;
        assert!(out.starts_with("exit 3"), "{out}");
        assert!(out.contains("oops"), "{out}");
    }
}
