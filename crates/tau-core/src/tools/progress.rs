//! In-flight tool progress (issue #197): the tool-side mirror of the streaming
//! assistant entry. A tool that produces meaningful output before it completes
//! reports it through a [`ToolProgress`]; the handle coalesces the reports into
//! at most one wire-only re-upsert of the tool entry per display frame
//! (ADR-0008's `EntryUpsert` lane — the tool is the only mid-stream re-upsert
//! left; the assistant streams deltas, #303).
//! The final result wins: the file line is written once, at completion, under
//! the same id (order-preservation rule #148 untouched — progress upserts are
//! wire-only).

use crate::session::{Entry, EntryEventHook};
use crate::tools::ToolOutput;
use parking_lot::Mutex;
use std::time::{Duration, Instant};
use tau_protocol::EntryKind;

/// The display-frame cadence: at most one in-flight re-upsert per frame
/// (ADR-0008, the same display-frame window the assistant's sink streams by).
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(16);

/// One tool call's in-flight output, streamed to the entry upsert hook.
///
/// The dispatch builds one per call (with the call phase's id, timestamp, and
/// a clone of the session's `entry_upsert_hook`) and hands it to the tool.
/// A tool with no in-flight shape (most core tools) simply never calls
/// [`report`]; a streaming tool (bash) calls it as output arrives. With no
/// hook (the direct test path) reports are a no-op.
pub struct ToolProgress {
    // None: no live wire path — reports are no-ops.
    upsert: Option<EntryEventHook>,
    id: String,
    parent: Option<String>,
    timestamp: u64,
    call_id: String,
    name: String,
    args: serde_json::Value,
    // The in-flight output accumulated so far (the growing Text).
    output: Mutex<String>,
    // The in-flight ui_detail (#243): the split tools stream their card
    // content here, so the live card and the final card render from the
    // same channel. Never touches `output`.
    detail: Mutex<String>,
    // The display-frame last-emitted clock (frame-aligned coalescing).
    last: Mutex<Option<Instant>>,
}

impl ToolProgress {
    #[must_use]
    pub fn new(
        upsert: Option<EntryEventHook>,
        id: String,
        parent: Option<String>,
        timestamp: u64,
        call_id: String,
        name: String,
        args: serde_json::Value,
    ) -> Self {
        Self {
            upsert,
            id,
            parent,
            timestamp,
            call_id,
            name,
            args,
            output: Mutex::new(String::new()),
            detail: Mutex::new(String::new()),
            last: Mutex::new(None),
        }
    }

    /// Fold a chunk of in-flight output in and re-emit the growing entry,
    /// at most once per display frame. Called from the tool's output readers;
    /// it never awaits, so the std mutexes are held across no yield.
    pub fn report(&self, chunk: &str) {
        if chunk.is_empty() || self.upsert.is_none() {
            return;
        }
        let now = Instant::now();
        {
            let mut last = self.last.lock();
            if last.is_some_and(|t| now.duration_since(t) < SNAPSHOT_INTERVAL) {
                return;
            }
            *last = Some(now);
        }
        let snapshot = {
            let mut out = self.output.lock();
            out.push_str(chunk);
            out.clone()
        };
        let Some(hook) = &self.upsert else {
            return;
        };
        let entry = Entry {
            id: self.id.clone(),
            parent: self.parent.clone(),
            kind: EntryKind::Tool,
            timestamp: self.timestamp,
            payload: tau_protocol::payload::ToolPayload {
                call_id: self.call_id.clone(),
                name: self.name.clone(),
                args: self.args.clone(),
                output: ToolOutput::Text(snapshot),
                ui_detail: None,
                status: None,
            }
            .to_value(),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        };
        hook(&entry);
    }

    /// Fold a chunk of in-flight GUI detail in and re-emit the growing entry,
    /// at most once per display frame — the `ui_detail` twin of [`report`]
    /// (#243): the final result wins, and the detail never rides `output`.
    pub fn report_detail(&self, chunk: &str) {
        if chunk.is_empty() || self.upsert.is_none() {
            return;
        }
        let now = Instant::now();
        {
            let mut last = self.last.lock();
            if last.is_some_and(|t| now.duration_since(t) < SNAPSHOT_INTERVAL) {
                return;
            }
            *last = Some(now);
        }
        let snapshot = {
            let mut out = self.detail.lock();
            out.push_str(chunk);
            out.clone()
        };
        let Some(hook) = &self.upsert else {
            return;
        };
        let entry = Entry {
            id: self.id.clone(),
            parent: self.parent.clone(),
            kind: EntryKind::Tool,
            timestamp: self.timestamp,
            payload: tau_protocol::payload::ToolPayload {
                call_id: self.call_id.clone(),
                name: self.name.clone(),
                args: self.args.clone(),
                output: ToolOutput::Text(String::new()),
                ui_detail: Some(ToolOutput::Text(snapshot)),
                status: None,
            }
            .to_value(),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        };
        hook(&entry);
    }
}
