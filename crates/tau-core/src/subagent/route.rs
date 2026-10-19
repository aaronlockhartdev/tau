use super::Supervisor;
use super::states::{ContextMode, StoppedBy};
use crate::agent::Lane;
use crate::session::SessionStore;
use crate::tools;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// Route a tool call at the parent's sub-agent surface (called from the
/// loop's tool dispatch; diagnostics are results, never panics).
pub fn route_parent(sup: &Arc<Supervisor>, tc: &tools::ToolCall) -> Result<String, String> {
    match tc.name.as_str() {
        "subagent_spawn" => {
            let Some(agent_type) = tc.args.get("type").and_then(Value::as_str) else {
                return Err("subagent_spawn: missing \"type\"".into());
            };
            let Some(brief) = tc.args.get("brief").and_then(Value::as_str) else {
                return Err("subagent_spawn: missing \"brief\"".into());
            };
            let context_mode =
                tc.args
                    .get("context_mode")
                    .and_then(Value::as_str)
                    .map(|m| match m {
                        "compacted" => ContextMode::Compacted,
                        "fork" => ContextMode::Fork,
                        _ => ContextMode::Fresh,
                    });
            match sup.spawn(agent_type, brief, context_mode, &tc.id) {
                Ok(s) => {
                    // The spawn reports the child's name: the model refers
                    // to it by name in the sub-agent tools that follow
                    // (the session id stays the machine key, internal).
                    Ok(format!(
                        "spawned sub-agent {}; it reports back via parent_notify",
                        s.name
                    ))
                }
                Err(e) => Err(e),
            }
        }
        "subagent_message" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return Err("subagent_message: missing \"name\"".into());
            };
            let text = tc
                .args
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match sup.message(
                name,
                text,
                Lane::Steering,
                // The parent model is the sender: stamp its session id so the
                // child's entry is attributed to it, not the user (#187).
                Some(sup.parent_session().to_owned()),
            ) {
                Ok(s) => Ok(s),
                Err(e) => Err(e),
            }
        }
        "subagent_stop" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return Err("subagent_stop: missing \"name\"".into());
            };
            match sup.stop(name, StoppedBy::Parent) {
                Ok(s) => Ok(s),
                Err(e) => Err(e),
            }
        }
        "subagent_state" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return Err("subagent_state: missing \"name\"".into());
            };
            match sup.state_info(name) {
                Some(info) => {
                    let name = sup
                        .resolve(name)
                        .map_or_else(|| info.child.clone(), |c| c.name.clone());
                    Ok(format!(
                        "{name}: {}{} — session {}",
                        info.state.kind(),
                        info.waiting_on
                            .map(|w| format!(" (waiting on {})", w.as_str()))
                            .unwrap_or_default(),
                        info.child
                    ))
                }
                None => Err(format!(
                    "subagent_state: no sub-agent {name} in this session"
                )),
            }
        }
        // The core tools route through the loop's normal dispatch.
        other => Err(format!("unknown tool: {other}")),
    }
}

/// True when a session file in the workspace already carries this title
/// (the header is the record, so the check reads the disk).
pub(super) fn title_on_disk(cwd: &Path, title: &str) -> bool {
    let dir = cwd.join(".tau").join("sessions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    rd.filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .any(|e| {
            let id = e
                .file_name()
                .to_string_lossy()
                .to_string()
                .trim_end_matches(".jsonl")
                .to_string();
            let mut s = SessionStore::for_workspace(cwd, &id);
            s.open().is_ok() && s.title() == Some(title)
        })
}
