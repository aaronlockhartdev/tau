pub(crate) fn mastra_file(name: &str) -> String {
    let path = format!(
        "{}/../../fixtures/references/mastra-om/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
}

/// The content of the template literal that opens at the first backtick
/// after `marker` (up to the next backtick) — the fidelity oracle for the
/// verbatim constants.
pub(crate) fn ts_literal(text: &str, marker: &str) -> String {
    let at = text.find(marker).expect(marker);
    let open = at
        + text
            .get(at..)
            .expect("offset from find() is a char boundary")
            .find('`')
            .expect("backtick after marker");
    let close = open
        + 1
        + text
            .get(open + 1..)
            .expect("offset from find() is a char boundary")
            .find('`')
            .expect("closing backtick");
    text.get(open + 1..close)
        .expect("offsets from find() are char boundaries")
        .to_owned()
}

/// Tau's one deliberate deviation from the upstream OM prompts: the
/// `current-task` section (and its `currentTaskEnabled` template slot) is
/// dropped — tau does not use current-task. Strip both from upstream-extracted
/// text so the "matches upstream" fidelity tests compare against the
/// post-deviation prompt.
pub(crate) fn strip_current_task(s: &str) -> String {
    // Template slot: remove the whole `${ currentTaskEnabled … }`.
    let s = if let Some(at) = s.find("currentTaskEnabled") {
        let open = s
            .get(..at)
            .expect("offset from find() is a char boundary")
            .rfind("${")
            .expect("currentTask slot open");
        let close = at
            + s.get(at..)
                .expect("offset from find() is a char boundary")
                .find("\n  }")
                .expect("currentTask slot close")
            + "\n  }".len();
        format!(
            "{}{}",
            s.get(..open)
                .expect("offset from find() is a char boundary"),
            s.get(close..)
                .expect("offset from find() is a char boundary")
        )
    } else {
        s.to_owned()
    };
    // Output-format block: remove `<current-task>…</current-task>` + the blank
    // line that follows it.
    if let Some(start) = s.find("<current-task>") {
        let close = start
            + s.get(start..)
                .expect("offset from find() is a char boundary")
                .find("</current-task>")
                .expect("current-task close")
            + "</current-task>".len();
        format!(
            "{}{}",
            s.get(..start)
                .expect("offset from find() is a char boundary"),
            s.get(close + 2..)
                .expect("offset from find() is a char boundary")
        )
    } else {
        s
    }
}
