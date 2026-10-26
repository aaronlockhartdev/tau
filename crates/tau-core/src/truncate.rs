//! The text caps the record channel writes with: shortened for storage or
//! display, always on a character boundary, always marked.

/// The first `max` characters of `text`, with `…` when anything was dropped.
/// The cap counts characters, so a cut never splits a multi-byte one.
#[must_use]
pub(crate) fn head(text: &str, max: usize) -> String {
    let head: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        head + "…"
    } else {
        head
    }
}

/// The complete lines of `text` that fit in `max` characters (#219): a partial
/// trailing line is dropped rather than quoted, so when not even one line fits
/// the marker stands alone.
#[must_use]
pub(crate) fn head_lines(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let window: String = text.chars().take(max).collect();
    match window.rsplit_once('\n') {
        // `kept` ends at the last complete line; `_partial` is the cut tail.
        Some((kept, _partial)) => format!("{kept}…"),
        None => "…".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_keeps_short_text_whole() {
        assert_eq!(head("no such model", 20), "no such model");
    }

    #[test]
    fn a_head_cuts_long_text_and_marks_the_cut() {
        assert_eq!(head("abcdefghij", 4), "abcd…");
        // A cut lands on a character boundary, never inside a multi-byte char.
        assert_eq!(head("héllo wörld", 3), "hél…");
    }

    #[test]
    fn head_lines_keeps_short_text_whole() {
        assert_eq!(head_lines("short", 4_000), "short");
    }

    #[test]
    fn head_lines_cuts_at_a_line_boundary() {
        // #219: the last visible line is complete and the marker follows it.
        let big = (0..600)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(big.chars().count() > 500);
        let out = head_lines(&big, 500);
        assert!(out.ends_with('…'));
        let body = out.strip_suffix('…').expect("the marker");
        assert!(!body.ends_with('\n'));
        assert!(body.ends_with("line 054"), "last visible line: {out:?}");
    }

    #[test]
    fn head_lines_without_a_complete_line_shows_only_the_marker() {
        assert_eq!(head_lines("xxxxxxxxxx", 4), "…");
    }
}
