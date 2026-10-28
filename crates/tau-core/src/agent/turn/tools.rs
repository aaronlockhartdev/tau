use crate::agent::{AgentError, AgentSession, Entry, FunctionCall, Ordering, Value, tools};
use std::collections::{HashMap, HashSet};
use tau_protocol::EntryKind;

/// #186: the batch indices of `edit` calls whose target file is also
/// targeted by another `edit` call in the same batch. They would share one
/// stale anchor snapshot, so every one of them is refused pre-write (batch
/// them into a single call instead) — the file stays untouched.
fn same_file_edit_conflicts(calls: &[FunctionCall]) -> HashSet<usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in calls.iter().filter(|c| c.name == "edit") {
        if let Some(path) = serde_json::from_str::<Value>(&c.arguments)
            .ok()
            .and_then(|v| v.get("path").and_then(Value::as_str).map(str::to_owned))
        {
            *counts.entry(path).or_insert(0) += 1;
        }
    }
    let mut conflicts = HashSet::new();
    for (i, c) in calls.iter().enumerate() {
        if c.name != "edit" {
            continue;
        }
        let Some(path) = serde_json::from_str::<Value>(&c.arguments)
            .ok()
            .and_then(|v| v.get("path").and_then(Value::as_str).map(str::to_owned))
        else {
            continue;
        };
        if counts.get(&path).is_some_and(|&n| n > 1) {
            conflicts.insert(i);
        }
    }
    conflicts
}

impl AgentSession {
    /// #222: a user stop cancels the in-flight tool batch. The call it
    /// reaches is recorded as cancelled (its in-flight process, if any, is
    /// already killed by the tool's own stop flag), and the rest of the
    /// batch is not executed.
    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn cancelled_tool_entry(&self, call: &FunctionCall) -> Result<(), AgentError> {
        let entry_id = self.inner.lock().store.mint_id();
        self.append_id(
            &entry_id,
            EntryKind::Tool,
            tau_protocol::payload::ToolPayload {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                args: Value::Null,
                output: tau_protocol::payload::ToolOutput::Text(
                    "cancelled by user stop".to_owned(),
                ),
                ui_detail: None,
                status: Some("cancelled".to_owned()),
            }
            .to_value(),
        )
    }

    #[allow(clippy::too_many_lines)] // one dispatch pass per call; splitting is refactoring
    pub(super) async fn run_tools(&self, calls: &[FunctionCall]) -> Result<(), AgentError> {
        let bad_edits = same_file_edit_conflicts(calls);
        for (ci, call) in calls.iter().enumerate() {
            if self.stop_requested() {
                // A stop landed before this call dispatched: record the
                // cancellation and drop the rest of the batch (#222).
                self.cancelled_tool_entry(call)?;
                break;
            }
            if bad_edits.contains(&ci) {
                // Same file already edited earlier in this batch: refuse
                // before dispatch — the file is left untouched.
                let entry_id = self.inner.lock().store.mint_id();
                self.append_id(
                    &entry_id,
                    EntryKind::Tool,
                    tau_protocol::payload::ToolPayload {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        args: Value::Null,
                        output: tau_protocol::payload::ToolOutput::Text(
                            "edit: file already edited in this batch; batch it into one edit call"
                                .into(),
                        ),
                        ui_detail: None,
                        status: Some("error".to_owned()),
                    }
                    .to_value(),
                )?;
                continue;
            }
            // ticket #70: keep the parse result so an unparseable call below
            // becomes a clear error, not a silent null.
            let parsed = serde_json::from_str::<Value>(&call.arguments);
            let args = parsed.as_ref().map_or(Value::Null, Value::clone);
            let tc = tools::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args: args.clone(),
            };
            // One id per call (ADR-0008): the call phase re-emits the entry
            // wire-only (empty output); the result persists the same id, so
            // the file keeps one line for the call.
            let (entry_id, parent, timestamp, upsert) = {
                let mut inner = self.inner.lock();
                let parent = inner.store.leaf().ok().flatten().map(|e| e.id);
                (
                    inner.store.mint_id(),
                    parent,
                    inner.store.now(),
                    inner.entry_upsert_hook.clone(),
                )
            };
            if let Some(hook) = &upsert {
                hook(&Entry {
                    id: entry_id.clone(),
                    parent: parent.clone(),
                    kind: EntryKind::Tool,
                    timestamp,
                    payload: tau_protocol::payload::ToolPayload {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        args: args.clone(),
                        output: tau_protocol::payload::ToolOutput::Text(String::new()),
                        ui_detail: None,
                        status: None,
                    }
                    .to_value(),
                    blob: None,
                    first_kept_entry_id: None,
                    crc: None,
                });
            }
            // Issue #197: build the in-flight progress handle for this call
            // (streaming tools report their growing output through it; the
            // coalesced upserts are wire-only, the final result wins).
            // Boxed: the handle is ~250 bytes inline (an `Arc`, several `String`s,
            // a `Value`, two `Mutex<String>`); a pointer keeps `run_tools`' future
            // small (clippy `large_futures`).
            let progress = Box::new(tools::ToolProgress::new(
                upsert.clone(),
                entry_id.clone(),
                parent,
                timestamp,
                call.call_id.clone(),
                call.name.clone(),
                args.clone(),
            ));
            // The surface routes the call (R1): the loop matches no tool
            // names — the table in tools::surface owns the mapping. The
            // links are cloned under the lock and invoked outside it: the
            // routes re-lock this session's `inner` (a child notify
            // records a state entry; a spawn reads the parent's OM
            // record), and the lock is not reentrant.
            let (sup, link, cwd, image_max, image_pixels) = {
                let inner = self.inner.lock();
                (
                    inner.subagents.clone(),
                    inner.child.clone(),
                    inner.cwd.clone(),
                    inner.turn.image_max_bytes,
                    inner.turn.image_max_pixels,
                )
            };
            // ticket #70: an unparseable call is a parse error, never a silent
            // null-args dispatch — the model reads the error and self-corrects.
            let result = match &parsed {
                Ok(_) => {
                    let ctx = tools::surface::DispatchCtx {
                        session: self,
                        cwd: &cwd,
                        image_max_bytes: image_max,
                        image_max_pixels: image_pixels,
                        progress: Some(progress.as_ref()),
                        supervisor: sup,
                        child: link,
                    };
                    // Boxed: the streaming `bash_inner` future is large; a `Pin<Box>`
                    // keeps `run_tools`' future small (clippy `large_futures`).
                    Box::pin(tools::surface::dispatch(ctx, &tc)).await
                }
                Err(e) => tools::ToolResult::err(tau_protocol::payload::ToolOutput::Text(format!(
                    "error: arguments are not valid JSON: {e}"
                ))),
            };
            let status = result.status();
            self.append_id(
                &entry_id,
                EntryKind::Tool,
                tau_protocol::payload::ToolPayload {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    args,
                    output: result.output,
                    ui_detail: result.ui_detail,
                    status: Some(status.to_owned()),
                }
                .to_value(),
            )?;
        }
        Ok(())
    }
}
