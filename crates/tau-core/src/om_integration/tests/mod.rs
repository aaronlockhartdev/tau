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
                "user",
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
        completed: true,
        calls: Vec::new(),
        mid_stream_errors: Vec::new(),
    }
}

mod plan;
mod record;
mod window;
