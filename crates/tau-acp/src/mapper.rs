//! The event → ACP `session/update` mapping (research doc §3.4):
//! snapshot-diffed text/thought chunks and two-phase tool calls. The only
//! stateful piece is the per-entry last-sent length; provider calls are
//! strictly sequential per session, so in-flight assistant entries are a
//! FIFO and `StreamEnd` finalizes the oldest.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Value, json};
use tau_protocol::snapshot::ViewEntry;

const KIND_ASSISTANT: &str = "assistant";
const KIND_TOOL: &str = "tool";
const TITLE_CAP: usize = 80;

#[derive(Default)]
pub struct Mapper {
    text_sent: HashMap<String, usize>,
    reasoning_sent: HashMap<String, usize>,
    in_flight: VecDeque<String>,
    in_flight_set: HashSet<String>,
    /// Entries `on_stream_end` has finalized; a re-upsert of one of these
    /// (the journal-resync case) resumes it at the front of the queue.
    finalized: HashSet<String>,
    tool_created: HashSet<String>,
    /// Tool ids whose result `tool_call_update` was already owed: a
    /// repeated snapshot of a finished call owes nothing.
    tool_result_sent: HashSet<String>,
}

impl Mapper {
    /// The updates one entry upsert owes the client (empty when nothing new).
    pub fn on_entry(&mut self, entry: &ViewEntry) -> Vec<Value> {
        match entry.kind.as_str() {
            KIND_ASSISTANT => self.on_assistant(entry),
            KIND_TOOL => self.on_tool(entry),
            // User entries surface (user_message_chunk): the headless
            // episode loop's continuation prompts are user entries and the
            // benchmark forensics (acp-events.jsonl) must see them.
            "user" => Self::on_user(entry),
            // system/subagent/task entries: no ACP v0 surface.
            _ => Vec::new(),
        }
    }

    fn on_user(entry: &ViewEntry) -> Vec<Value> {
        let text = entry
            .payload
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("");
        if text.is_empty() {
            return Vec::new();
        }
        vec![json!({
            "sessionUpdate": "user_message_chunk",
            "content": { "type": "text", "text": text },
        })]
    }

    /// Finalize every in-flight entry: the turn-settled `StreamEnd` (issue
    /// #82) closes the whole turn — all of its calls — not just the last.
    pub fn on_stream_end(&mut self) {
        while let Some(id) = self.in_flight.pop_front() {
            self.in_flight_set.remove(&id);
            self.finalized.insert(id.clone());
            self.text_sent.remove(&id);
            self.reasoning_sent.remove(&id);
        }
    }

    fn on_assistant(&mut self, entry: &ViewEntry) -> Vec<Value> {
        let mut out = Vec::new();
        if !self.in_flight_set.contains(&entry.id) {
            self.in_flight_set.insert(entry.id.clone());
            // A resumed (previously finalized) entry is the current call
            // again: newest in the finalize queue. A fresh entry is the
            // oldest not yet finalized.
            if self.finalized.contains(&entry.id) {
                self.in_flight.push_front(entry.id.clone());
            } else {
                self.in_flight.push_back(entry.id.clone());
            }
            self.text_sent.insert(entry.id.clone(), 0);
            self.reasoning_sent.insert(entry.id.clone(), 0);
        }
        for (sent, field, variant) in [
            (&mut self.text_sent, "text", "agent_message_chunk"),
            (&mut self.reasoning_sent, "reasoning", "agent_thought_chunk"),
        ] {
            let text = entry
                .payload
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or("");
            let sent_chars = sent.get(&entry.id).copied().unwrap_or(0);
            let chars = text.chars().count();
            if chars > sent_chars {
                // Char-offset safe: snapshots re-emit the whole growing
                // text, the delta is its suffix.
                let delta: String = text.chars().skip(sent_chars).collect();
                sent.insert(entry.id.clone(), chars);
                out.push(json!({
                    "sessionUpdate": variant,
                    "content": { "type": "text", "text": delta },
                    "messageId": entry.id,
                }));
            }
        }
        out
    }

    fn on_tool(&mut self, entry: &ViewEntry) -> Vec<Value> {
        let name = entry
            .payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let args = entry.payload.get("args").cloned().unwrap_or(Value::Null);
        let output = entry.payload.get("output");
        let output_text = output.and_then(Value::as_str);
        // Image outputs (the `read` tool on a .png) have no text surface in
        // v0: the create carries the input, the result is rawOutput only.
        let has_result =
            output_text.is_some() || output.is_some_and(|o| !o.is_null() && !o.is_string());
        let mut out = Vec::new();
        if !self.tool_created.contains(&entry.id) {
            self.tool_created.insert(entry.id.clone());
            out.push(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": entry.id,
                "name": name,
                "title": tool_title(name, &args),
                "kind": tool_kind(name),
                "status": "in_progress",
                "rawInput": args,
            }));
        } else if has_result && !self.tool_result_sent.contains(&entry.id) {
            self.tool_result_sent.insert(entry.id.clone());
            let mut update = json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": entry.id,
                "status": "completed",
                "rawOutput": output.cloned().unwrap_or(Value::Null),
            });
            if let Some(text) = output_text {
                update["content"] = json!([
                    { "type": "content", "content": { "type": "text", "text": text } }
                ]);
            }
            out.push(update);
        }
        out
    }
}

fn tool_title(name: &str, args: &Value) -> String {
    let detail = match name {
        "bash" => args.get("command").and_then(Value::as_str),
        "read" | "write" | "edit" => args.get("path").and_then(Value::as_str),
        _ => None,
    }
    .unwrap_or(name);
    let mut title: String = detail.chars().take(TITLE_CAP).collect();
    if detail.chars().count() > TITLE_CAP {
        title.push('…');
    }
    title
}

fn tool_kind(name: &str) -> &'static str {
    match name {
        "read" => "read",
        "write" | "edit" => "edit",
        "bash" => "execute",
        "recall" => "search",
        n if n.starts_with("subagent_") || n.starts_with("task_") => "think",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: &str, payload: Value) -> ViewEntry {
        ViewEntry {
            id: id.to_owned(),
            parent: None,
            kind: kind.to_owned(),
            timestamp: 1,
            payload,
            blob: None,
            first_kept: None,
        }
    }

    #[test]
    fn assistant_snapshots_diff_to_chunks() {
        let mut m = Mapper::default();
        let first = entry("e1", "assistant", json!({ "text": "Hel", "reasoning": "" }));
        let out = m.on_entry(&first);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(out[0]["content"]["text"], "Hel");
        assert_eq!(out[0]["messageId"], "e1");

        let grown = entry(
            "e1",
            "assistant",
            json!({ "text": "Hello", "reasoning": "" }),
        );
        let out = m.on_entry(&grown);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"]["text"], "lo");

        // A snapshot that repeats the same text owes nothing.
        assert!(m.on_entry(&grown).is_empty());
    }

    #[test]
    fn multibyte_text_never_splits_mid_char() {
        let mut m = Mapper::default();
        m.on_entry(&entry("e1", "assistant", json!({ "text": "ταυ" })));
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "ταυ ✓" })));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"]["text"], " ✓");
    }

    #[test]
    fn reasoning_streams_as_thought_chunks() {
        let mut m = Mapper::default();
        let out = m.on_entry(&entry(
            "e1",
            "assistant",
            json!({ "text": "", "reasoning": "hmm" }),
        ));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "agent_thought_chunk");
    }

    #[test]
    fn stream_end_finalizes_every_in_flight() {
        let mut m = Mapper::default();
        m.on_entry(&entry("e1", "assistant", json!({ "text": "a" })));
        // An in-flight entry's unchanged snapshot owes nothing.
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "a" })));
        assert!(out.is_empty(), "unchanged in-flight snapshot owes nothing");
        m.on_entry(&entry("e2", "assistant", json!({ "text": "b" })));
        // The turn-settled `StreamEnd` (issue #82) closes the whole turn:
        // every in-flight entry finalizes at once.
        m.on_stream_end();
        // A resumed (finalized) entry is the current call again: a fresh
        // message with the full text, newest in the finalize queue.
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "ab" })));
        assert_eq!(out[0]["content"]["text"], "ab");
        let out = m.on_entry(&entry("e2", "assistant", json!({ "text": "b" })));
        assert_eq!(out[0]["content"]["text"], "b");
    }

    #[test]
    fn tool_call_is_two_phase() {
        let mut m = Mapper::default();
        let start = entry(
            "t1",
            "tool",
            json!({
                "name": "bash", "call_id": "c1",
                "args": { "command": "cargo test" }, "output": ""
            }),
        );
        let out = m.on_entry(&start);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "tool_call");
        assert_eq!(out[0]["kind"], "execute");
        assert_eq!(out[0]["status"], "in_progress");
        assert_eq!(out[0]["rawInput"]["command"], "cargo test");

        let done = entry(
            "t1",
            "tool",
            json!({
                "name": "bash", "call_id": "c1",
                "args": { "command": "cargo test" }, "output": "ok"
            }),
        );
        let out = m.on_entry(&done);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(out[0]["status"], "completed");
        assert_eq!(out[0]["rawOutput"], "ok");
        assert_eq!(out[0]["content"][0]["content"]["text"], "ok");

        // A repeat of the result owes nothing.
        assert!(m.on_entry(&done).is_empty());
    }

    #[test]
    fn non_assistant_tool_kinds_map() {
        let mut m = Mapper::default();
        for (name, kind) in [
            ("read", "read"),
            ("write", "edit"),
            ("edit", "edit"),
            ("recall", "search"),
            ("subagent_spawn", "think"),
            ("task_create", "think"),
            ("mystery", "other"),
        ] {
            let out = m.on_entry(&entry(
                &format!("t-{name}"),
                "tool",
                json!({ "name": name, "args": {}, "output": "" }),
            ));
            assert_eq!(out[0]["kind"], kind, "{name}");
        }
    }

    #[test]
    fn file_tool_title_uses_the_path() {
        let mut m = Mapper::default();
        let out = m.on_entry(&entry(
            "t1",
            "tool",
            json!({ "name": "read", "args": { "path": "/src/main.rs" }, "output": "" }),
        ));
        assert_eq!(out[0]["title"], "/src/main.rs");
    }
}
