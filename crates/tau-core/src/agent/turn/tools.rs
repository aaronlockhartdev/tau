use crate::agent::{AgentError, AgentSession, Entry, FunctionCall, KIND_TOOL, Value, tools};

impl AgentSession {
    pub(super) async fn run_tools(&self, calls: &[FunctionCall]) -> Result<(), AgentError> {
        for call in calls {
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
                let mut inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
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
                    parent,
                    kind: KIND_TOOL.to_owned(),
                    timestamp,
                    payload: tau_protocol::payload::ToolPayload {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        args: args.clone(),
                        output: tau_protocol::payload::ToolOutput::Text(String::new()),
                        status: None,
                    }
                    .to_value(),
                    blob: None,
                    first_kept_entry_id: None,
                    crc: None,
                });
            }
            // The surface routes the call (R1): the loop matches no tool
            // names — the table in tools::surface owns the mapping. The
            // links are cloned under the lock and invoked outside it: the
            // routes re-lock this session's `inner` (a child notify
            // records a state entry; a spawn reads the parent's OM
            // record), and the lock is not reentrant.
            let (sup, link, cwd, image_max) = {
                let inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                (
                    inner.subagents.clone(),
                    inner.child.clone(),
                    inner.cwd.clone(),
                    inner.turn.image_max_bytes,
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
                        supervisor: sup,
                        child: link,
                    };
                    tools::surface::dispatch(ctx, &tc).await
                }
                Err(e) => tools::ToolResult::err(tau_protocol::payload::ToolOutput::Text(format!(
                    "error: arguments are not valid JSON: {e}"
                ))),
            };
            let status = result.status();
            self.append_id(
                &entry_id,
                KIND_TOOL,
                tau_protocol::payload::ToolPayload {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    args,
                    output: result.output,
                    status: Some(status.to_owned()),
                }
                .to_value(),
            )?;
        }
        Ok(())
    }
}
