use super::*;

pub(crate) fn store_with_text_entries(
    dir: &std::path::Path,
    n: usize,
    chars_each: usize,
) -> SessionStore {
    let mut store = SessionStore::for_workspace(dir, "s1");
    store.create().unwrap();
    let mut parent: Option<String> = None;
    for _ in 0..n {
        let text = "w".repeat(chars_each);
        let entry = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": text, "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(entry.id);
    }
    store
}

pub(crate) fn turn_result(text: &str) -> crate::provider::TurnResult {
    crate::provider::TurnResult {
        text: text.into(),
        reasoning: String::new(),
        usage: None,
        duration: None,
        outcome: crate::provider::StreamOutcome::Completed,
        calls: Vec::new(),
        blocks: Vec::new(),
        mid_stream_errors: Vec::new(),
    }
}

mod plan;
mod promote;
mod record;
mod settle;
mod transcript;
mod window;
/// A provider that records what each Observer call was asked — the transcript
/// it carried — and serves one canned observation body.
pub(crate) struct RecordingProvider {
    calls: std::sync::Mutex<Vec<(bool, String)>>,
    body: String,
}

impl RecordingProvider {
    pub(crate) fn new(observer_text: &str) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            body: crate::agent::testkit::sse_json(&format!(
                "<observations>{observer_text}</observations>"
            )),
        }
    }
    pub(crate) fn observer_calls(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(is_observer, _)| *is_observer)
            .count()
    }
    pub(crate) fn observer_transcripts(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(is_observer, _)| *is_observer)
            .map(|(_, transcript)| transcript.clone())
            .collect()
    }
}

impl crate::provider::TurnProvider for RecordingProvider {
    fn call<'a>(
        &self,
        request: &crate::provider::ResponseRequest,
        sink: &'a mut dyn crate::provider::TurnSink,
    ) -> crate::provider::ProviderTurn<'a> {
        let is_observer = request
            .instructions()
            .is_some_and(|i| i == crate::om::observer_system_prompt());
        let transcript = serde_json::to_string(&request.input()).unwrap_or_default();
        self.calls.lock().unwrap().push((is_observer, transcript));
        crate::provider::canned(&self.body).call(request, sink)
    }
}
