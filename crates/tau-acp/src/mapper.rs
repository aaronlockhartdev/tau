//! The event → ACP `session/update` mapping (research doc §3.4):
//! snapshot-diffed text/thought chunks and two-phase tool calls. The only
//! stateful piece is the per-entry last-sent length; provider calls are
//! strictly sequential per session, so in-flight assistant entries are a
//! FIFO and `StreamEnd` finalizes the oldest.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Value, json};
use tau_protocol::EntryKind;
use tau_protocol::snapshot::ViewEntry;

const TITLE_CAP: usize = 80;

#[derive(Default)]
pub struct Mapper {
    text_sent: HashMap<String, usize>,
    reasoning_sent: HashMap<String, usize>,
    /// The last delta seq applied per entry (#303): a frame applies only as the
    /// next one, so the sent-counters above stay aligned with the characters
    /// the client holds — the entry's file line is diffed against them.
    delta_seq: HashMap<String, u64>,
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
        match entry.kind {
            EntryKind::Assistant => self.on_assistant(entry),
            EntryKind::Tool => self.on_tool(entry),
            // User entries surface (user_message_chunk): the headless
            // episode loop's continuation prompts are user entries and the
            // benchmark forensics (acp-events.jsonl) must see them.
            EntryKind::User => Self::on_user(entry),
            // system/subagent entries: no ACP v0 surface.
            EntryKind::System
            | EntryKind::Om
            | EntryKind::Subagent
            | EntryKind::SpawnSnapshot
            | EntryKind::Diagnostic
            | EntryKind::Other(_) => Vec::new(),
        }
    }
    /// The updates one streaming delta owes the client (#303): the producer
    /// already cut the text to the new characters, so this emits one chunk per
    /// non-empty field, with no diffing of the accumulation. It advances the
    /// same sent-counters the entry's file line is diffed against, so the
    /// committed entry then owes nothing extra.
    ///
    /// A frame that is not the next one — the event pipe is bounded, and a
    /// delta can be dropped (ADR-0008: the wire is lossy, the file is the
    /// recovery) — owes nothing and freezes the run: the client's text stays
    /// short by that frame, and the entry's file line below owes the whole rest
    /// from the sent-counter, so it repairs what the client sees. Forwarding an
    /// out-of-order frame would leave a hole the record cannot see, because its
    /// diff starts where the text the client holds ends.
    pub fn on_delta(&mut self, id: &str, seq: u64, text: &str, reasoning: &str) -> Vec<Value> {
        let next = self.delta_seq.get(id).map_or(1, |&last| last + 1);
        if seq != next {
            return Vec::new();
        }
        self.delta_seq.insert(id.to_owned(), seq);
        self.track_assistant(id);
        let mut out = Vec::new();
        for (sent, chunk, variant) in [
            (&mut self.text_sent, text, "agent_message_chunk"),
            (&mut self.reasoning_sent, reasoning, "agent_thought_chunk"),
        ] {
            let chars = chunk.chars().count();
            if chars == 0 {
                continue;
            }
            *sent.entry(id.to_owned()).or_insert(0) += chars;
            out.push(json!({
                "sessionUpdate": variant,
                "content": { "type": "text", "text": chunk },
                "messageId": id,
            }));
        }
        out
    }

    /// The in-flight bookkeeping for an assistant entry seen for the first
    /// time, shared by the deltas and the entry's file line.
    fn track_assistant(&mut self, id: &str) {
        if !self.in_flight_set.contains(id) {
            self.in_flight_set.insert(id.to_owned());
            // A resumed (previously finalized) entry is the current call
            // again: newest in the finalize queue. A fresh entry is the
            // oldest not yet finalized.
            if self.finalized.contains(id) {
                self.in_flight.push_front(id.to_owned());
            } else {
                self.in_flight.push_back(id.to_owned());
            }
            self.text_sent.insert(id.to_owned(), 0);
            self.reasoning_sent.insert(id.to_owned(), 0);
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
            self.delta_seq.remove(&id);
        }
    }

    fn on_assistant(&mut self, entry: &ViewEntry) -> Vec<Value> {
        let mut out = Vec::new();
        self.track_assistant(&entry.id);
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
        let has_result = output.is_some_and(|o| !o.is_null());
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
        n if n.starts_with("subagent_") => "think",
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
            kind: EntryKind::parse(kind),
            timestamp: 1,
            payload,
            blob: None,
            first_kept: None,
        }
    }

    /// #303: the producer cut the delta, so the mapper forwards it — no diff
    /// to compute, and the counters advance so the file line owes nothing.
    #[test]
    fn a_delta_forwards_the_new_characters() {
        let mut m = Mapper::default();
        let out = m.on_delta("e1", 1, "Hel", "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(out[0]["content"]["text"], "Hel");
        assert_eq!(out[0]["messageId"], "e1");
        let out = m.on_delta("e1", 2, "lo", "");
        assert_eq!(out[0]["content"]["text"], "lo");
    }

    #[test]
    fn a_delta_with_both_fields_emits_both_chunks() {
        let mut m = Mapper::default();
        let out = m.on_delta("e1", 1, "said", "thought");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(out[1]["sessionUpdate"], "agent_thought_chunk");
        assert_eq!(out[1]["content"]["text"], "thought");
    }

    #[test]
    fn a_delta_with_nothing_owes_nothing() {
        let mut m = Mapper::default();
        assert!(m.on_delta("e1", 1, "", "").is_empty());
    }

    /// The contract the whole delta design buys: the deltas of a stream and
    /// its file line together produce the text once, never twice.
    #[test]
    fn the_deltas_and_the_file_line_produce_the_text_once() {
        let mut m = Mapper::default();
        m.on_delta("e1", 1, "Hel", "th");
        m.on_delta("e1", 2, "lo", "inking");
        let out = m.on_entry(&entry(
            "e1",
            "assistant",
            json!({ "text": "Hello", "reasoning": "thinking" }),
        ));
        assert!(
            out.is_empty(),
            "the record owes nothing after its deltas: {out:?}"
        );
        // The journal-resync case: the entry is live again, and its frames
        // continue the seq the run left off at.
        let out = m.on_delta("e1", 3, " again", "");
        assert_eq!(out[0]["content"]["text"], " again");
    }

    #[test]
    fn a_multibyte_delta_counts_characters_for_the_record() {
        let mut m = Mapper::default();
        m.on_delta("e1", 1, "ταυ", "");
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "ταυ" })));
        assert!(out.is_empty(), "bytes were counted, not chars: {out:?}");
    }

    /// A frame the pipe dropped (the channel is bounded, ADR-0008) must not
    /// reach the client out of order: the run freezes at its last contiguous
    /// prefix, and the entry's file line owes everything from there — the
    /// record is the recovery. Forwarding the frame would leave a hole the
    /// record's diff cannot see, since that diff starts where the client's
    /// text ends.
    #[test]
    fn a_dropped_frame_leaves_the_record_to_carry_the_rest() {
        let mut m = Mapper::default();
        assert_eq!(m.on_delta("e1", 1, "ab", "")[0]["content"]["text"], "ab");
        assert!(
            m.on_delta("e1", 3, "cd", "").is_empty(),
            "a frame out of order owes nothing"
        );
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "abcdef" })));
        assert_eq!(out[0]["content"]["text"], "cdef");
    }

    #[test]
    fn a_repeated_frame_owes_nothing() {
        let mut m = Mapper::default();
        m.on_delta("e1", 1, "ab", "");
        assert!(m.on_delta("e1", 1, "ab", "").is_empty());
        assert!(m.on_delta("e1", 5, "zz", "").is_empty());
        let out = m.on_entry(&entry("e1", "assistant", json!({ "text": "abcdef" })));
        assert_eq!(out[0]["content"]["text"], "cdef");
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

    #[test]
    fn blob_backed_tool_with_inlined_payload_owes_the_result_update() {
        // #265: once the upsert hook inlines the sidecar blob, the tool's
        // result phase reaches the mapper with `output` present — the
        // two-phase update fires, no client-side resolution needed.
        let mut m = Mapper::default();
        let start = entry(
            "t1",
            "tool",
            json!({ "name": "bash", "args": { "command": "ls" }, "output": null }),
        );
        assert_eq!(m.on_entry(&start).len(), 1);

        // The result upsert arrives with the inlined payload (payload is the
        // decoded JSON, the blob ref is kept).
        let mut done = entry(
            "t1",
            "tool",
            json!({ "name": "bash", "args": { "command": "ls" }, "output": "the bytes" }),
        );
        done.blob = Some(tau_protocol::snapshot::BlobRef {
            id: "t1".into(),
            size: 12,
            hash: "0000000000000000".into(),
        });
        let out = m.on_entry(&done);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(out[0]["status"], "completed");
        assert_eq!(out[0]["rawOutput"], "the bytes");
    }
}
