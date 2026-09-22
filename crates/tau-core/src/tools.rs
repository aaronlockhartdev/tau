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
        ToolSpec {
            kind: ToolKind::Function,
            name: "recall".into(),
            description:
                "Browse the raw session entries an observation group covers. Pass the group id (16 hex digits, from the observation log). A compacted sub-agent passes scope: \"parent\" to browse the parent session's raw history (its frozen prefix points there). Returns the entries in the group's range."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "group": {"type": "string"},
                    "scope": {"type": "string", "enum": ["self", "parent"]}
                },
                "required": ["group"]
            }),
        },
    ]
}

/// The parent-side sub-agent tools (spec §5.3): present on non-child
/// sessions only — the depth cap (a child cannot spawn) is structural.
pub fn subagent_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_spawn".into(),
            description: "Spawn a sub-agent that works the brief in its own session and reports back via parent_notify. Returns its handle. If the work is already a task, pass task — the child starts with it assigned; a separate assign costs an extra round trip. context_mode: fresh (default) = no parent history; compacted = the parent's observation log as a frozen context prefix; fork = a branched copy of the parent session.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["general"]},
                    "brief": {"type": "string"},
                    "context_mode": {"type": "string", "enum": ["fresh", "compacted", "fork"]},
                    "task": {"type": "string"}
                },
                "required": ["type", "brief"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_message".into(),
            description: "Message a sub-agent by handle. A running child takes it on its steering lane; a non-running child is resumed with it. Omit text for a pure resume (\"continue from where you stopped\").".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "handle": {"type": "string"},
                    "text": {"type": "string"}
                },
                "required": ["handle"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_stop".into(),
            description: "Stop a running sub-agent: its in-flight stream is cut (the partial is kept) and it ends in the stopped state — it stays resumable. A sub-agent that is not running returns its current state (already parked/done/failed/stopped) and is left alone.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"handle": {"type": "string"}},
                "required": ["handle"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_state".into(),
            description: "Inspect a sub-agent by handle: lifecycle state, what it is waiting for, its last message, usage, and its session id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"handle": {"type": "string"}},
                "required": ["handle"]
            }),
        },
    ]
}

/// The child-side tool (spec §5.3): the only channel back to the parent.
/// done:true requires a structured output and ends the child; a note keeps
/// it parked. waiting_on declares what a parking child waits for (the nudge
/// fork: done / a valid declaration / failed).
pub fn parent_notify_spec() -> ToolSpec {
    ToolSpec {
        kind: ToolKind::Function,
        name: "parent_notify".into(),
        description: "Notify your parent. done:true with an object output finishes the task (the output is the result handed back). Without done, the note parks you; waiting_on declares what you wait for: parent | user | subagent.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "done": {"type": "boolean"},
                "output": {"type": "object"},
                "waiting_on": {"type": "string", "enum": ["parent", "user", "subagent"]}
            },
            "required": ["text"]
        }),
    }
}

/// The seven task tools (spec §5.4): free text + ids in; the core enforces
/// the state machines, the model never sees the enums. Available in every
/// session — a parent can complete small tasks itself (ADR-0001).
pub fn task_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_create".into(),
            description: "Create a task in this session: a title, an ordered list of steps (each with its expected output), and acceptance criteria. Returns the task id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "steps": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "expected_output": {"type": "string"}}, "required": ["text", "expected_output"]}},
                    "criteria": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["title"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_assign".into(),
            description: "Assign a task to a worker session: its record copies into that session, which becomes the live one; this session's copy becomes a status pointer. worker is a session id. Prefer passing the task at spawn time; use this when the worker already exists.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "worker": {"type": "string"}
                },
                "required": ["task", "worker"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_start".into(),
            description: "Start working a task (pending, or unblock it).".into(),
            parameters: json!({
                "type": "object",
                "properties": {"task": {"type": "string"}},
                "required": ["task"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_evidence".into(),
            description: "Record evidence for a criterion: a summary, optionally a reproducible command and an artifact, and whether it passed. A task is done only when every criterion has passing evidence.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "criterion": {"type": "string"},
                    "summary": {"type": "string"},
                    "command": {"type": "string"},
                    "artifact": {"type": "string"},
                    "passed": {"type": "boolean"},
                    "step": {"type": "string"}
                },
                "required": ["task", "criterion", "summary"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_block".into(),
            description: "Block a task with a free-text reason and, optionally, what is needed to unblock it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "reason": {"type": "string"},
                    "needs": {"type": "string"}
                },
                "required": ["task", "reason"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_finish".into(),
            description: "Finish a task. Fails unless every criterion is satisfied by passing evidence; force:true with a reason is the documented escape.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "force": {"type": "boolean"},
                    "reason": {"type": "string"}
                },
                "required": ["task"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_cancel".into(),
            description: "Cancel a task (with an optional reason).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "reason": {"type": "string"}
                },
                "required": ["task"]
            }),
        },
    ]
}

/// A non-child session's tool set: the core tools + the sub-agent tools +
/// the task tools.
pub fn agent_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.extend(subagent_tool_specs());
    v.extend(task_tool_specs());
    v
}

/// The worker side of the task tools: a child works the record its parent
/// assigned — it starts, evidences, blocks, and finishes it (spec §5.3).
pub fn worker_task_tool_specs() -> Vec<ToolSpec> {
    const WORKER: &[&str] = &["task_start", "task_evidence", "task_block", "task_finish"];
    task_tool_specs()
        .into_iter()
        .filter(|s| WORKER.contains(&s.name.as_str()))
        .collect()
}

/// A child session's tool set: the core tools + parent_notify + the
/// worker-side task tools — no sub-agent tools (a child cannot spawn,
/// ADR-0001 depth cap), and no create/assign/cancel: a child is a leaf, so
/// a task it created itself would be invisible to the parent and would die
/// with it (spec §5.3).
pub fn child_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.push(parent_notify_spec());
    v.extend(worker_task_tool_specs());
    v
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
        Ok(c) => hashline::normalize(&c),
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
        Ok(c) => hashline::normalize(&c),
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
    // Bounded output (the reference returns the changed region, not the
    // whole file): the changed lines plus two context lines per side, with
    // their fresh anchors, plus the result line count.
    if edited.first_changed > edited.last_changed {
        return format!("edited {} (no change)", path.display());
    }
    let rows = hashline::render(&edited.content).unwrap_or_default();
    let lo = edited.first_changed.saturating_sub(1).saturating_sub(2);
    let hi = (edited.last_changed + 2).min(rows.len());
    format!(
        "edited {} (lines {}–{} of {})",
        path.display(),
        edited.first_changed,
        edited.last_changed,
        rows.len()
    ) + "\n"
        + &rows[lo..hi].join("\n")
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
            Ok(Err(e)) => return format!("bash: {e}"),
            // The timeout dropped the (now owned) child; kill_on_drop fired,
            // so a timed-out command cannot keep running and mutate files.
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
    async fn crlf_files_are_normalized_through_read_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        let out = read(dir.path(), &json!({"path": "w.txt"})).await;
        assert!(!out.contains('\r'), "{out}");
        let rows: Vec<&str> = out.lines().collect();
        let anchor = rows[0].split('│').next().unwrap().to_string();
        edit(
            dir.path(),
            &json!({"path": "w.txt", "from": anchor, "to": anchor, "content": "ONE"}),
        )
        .await;
        // The written file is consistently LF — untouched lines included.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("w.txt")).unwrap(),
            "ONE\ntwo\nthree\n"
        );
    }

    #[tokio::test]
    async fn bash_timeout_kills_the_child_not_just_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let out = bash(
            dir.path(),
            &json!({"command": "sleep 5; touch marker", "timeout_secs": 1}),
        )
        .await;
        assert!(out.contains("timed out after 1s"), "{out}");
        // If the child survived the timeout it would finish its sleep and
        // write the marker; give it time to prove it is dead.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!marker.exists(), "the timed-out child kept running");
    }

    #[tokio::test]
    async fn edit_output_is_bounded_to_the_changed_region() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (0..50).map(|i| format!("line{i}\n")).collect();
        write(
            dir.path(),
            &json!({"path": "big.txt", "content": content.as_str()}),
        )
        .await;
        let read_out = read(dir.path(), &json!({"path": "big.txt"})).await;
        let lines: Vec<&str> = read_out.lines().collect();
        let anchor = lines[24].split('│').next().unwrap().to_string();
        let out = edit(
            dir.path(),
            &json!({"path": "big.txt", "from": anchor, "to": anchor, "content": "LINE25"}),
        )
        .await;
        // The changed region plus two context lines per side — never the 50-line file.
        let shown = out.lines().count() - 1;
        assert!(shown <= 7, "output had {shown} rows: {out}");
        assert!(out.contains("LINE25"), "{out}");
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
