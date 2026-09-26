pub(crate) fn mastra_file(name: &str) -> String {
    let path = format!(
        "{}/../../third_party/mastra-om/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {}", path, e))
}

/// The content of the template literal that opens at the first backtick
/// after `marker` (up to the next backtick) — the fidelity oracle for the
/// verbatim constants.
pub(crate) fn ts_literal(text: &str, marker: &str) -> String {
    let at = text.find(marker).expect(marker);
    let open = at + text[at..].find('`').expect("backtick after marker");
    let close = open + 1 + text[open + 1..].find('`').expect("closing backtick");
    text[open + 1..close].to_string()
}

/// Tau's one deliberate deviation from the upstream OM prompts: the
/// `current-task` section (and its `currentTaskEnabled` template slot) is
/// dropped — tau does not use current-task. Strip both from upstream-extracted
/// text so the "matches upstream" fidelity tests compare against the
/// post-deviation prompt.
pub(crate) fn strip_current_task(s: &str) -> String {
    // Template slot: remove the whole `${ currentTaskEnabled … }`.
    let s = if let Some(at) = s.find("currentTaskEnabled") {
        let open = s[..at].rfind("${").expect("currentTask slot open");
        let close = at + s[at..].find("\n  }").expect("currentTask slot close") + "\n  }".len();
        format!("{}{}", &s[..open], &s[close..])
    } else {
        s.to_owned()
    };
    // Output-format block: remove `<current-task>…</current-task>` + the blank
    // line that follows it.
    if let Some(start) = s.find("<current-task>") {
        let close = start
            + s[start..]
                .find("</current-task>")
                .expect("current-task close")
            + "</current-task>".len();
        format!("{}{}", &s[..start], &s[close + 2..])
    } else {
        s
    }
}
