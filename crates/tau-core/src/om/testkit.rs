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
