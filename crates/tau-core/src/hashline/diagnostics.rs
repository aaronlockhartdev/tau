//! Model-facing diagnostic formatting (parity with the reference's
//! `diagRef`, `fmtMismatchWithHashes`, and `stripRowPrefix`): the
//! shape-specific malformed-ref message, the stale-anchor context block,
//! and the lenient-fix row helpers.

use super::{SEP, hash_to_idx};

/// Clip a line for inclusion in a diagnostic: newlines made literal, capped
/// at 200 chars (the reference's `clipLine`).
#[must_use]
pub(super) fn clip_line(line: &str) -> String {
    let flat = line.replace('\n', "\\n");
    let mut chars = flat.chars();
    let clipped: String = chars.by_ref().take(200).collect();
    if chars.next().is_some() {
        format!("{clipped}...")
    } else {
        clipped
    }
}

/// The shape-specific malformed-ref message (parity with the reference's
/// `diagRef`): each shape tells the model exactly what to send instead.
#[must_use]
pub(super) fn bad_ref_message(ref_: &str) -> String {
    let trimmed = ref_.trim();
    if trimmed.is_empty() {
        return "[E_BAD_REF] invalid anchor: expected a 4-character anchor (letters only, e.g. \"Hasu\")".into();
    }
    if trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        return "[E_BAD_REF] invalid anchor: use the anchor alone (e.g. \"Hasu\") — no line numbers or trailing content"
            .into();
    }
    if trimmed.contains(SEP) && trimmed.contains('\n') {
        let lines: Vec<&str> = trimmed.split('\n').collect();
        let first = lines.first().copied().unwrap_or("");
        let last = lines.last().copied().unwrap_or("");
        // The block's own first/last anchors are the intended bounds.
        let hash_of = |s: &str| -> String {
            s.split(SEP)
                .next()
                .filter(|h| hash_to_idx(h).is_some())
                .map_or_else(|| "Hasu".to_owned(), str::to_owned)
        };
        let preview: String = first.chars().take(60).collect();
        return format!(
            "[E_BAD_REF] invalid anchor — from and to must each be a single bare 4-character anchor (letters only, e.g. \"Hasu\"), not a block with HASH{SEP}content. Received {} line(s) starting \"{}…\" — use only the first hash \"{}\" as from and \"{}\" as to, and put the new content (without HASH{SEP}) in content.",
            lines.len(),
            preview,
            hash_of(first),
            hash_of(last)
        );
    }
    if trimmed.contains(SEP) {
        return format!(
            "[E_BAD_REF] invalid anchor \"{trimmed}\": use only the 4-character anchor, drop everything from \"{SEP}\" onward"
        );
    }
    format!(
        "[E_BAD_REF] invalid anchor \"{trimmed}\": expected a 4-character anchor (letters only, e.g. \"Hasu\")"
    )
}

/// The stale-anchor context block (parity with the reference's
/// `fmtMismatchWithHashes`): the ±1 rows around the resolved other bound,
/// re-anchored, so the model sees where things are without a re-read.
/// `rows` are pre-rendered `HASH│<clipped line>` lines, 1-based labelled.
#[must_use]
pub(super) fn stale_context_block(anchor: &str, line: usize, rows: &[String]) -> String {
    format!(
        "  Current context around resolved anchor \"{anchor}\" (line {line}):\n{}",
        rows.join("\n")
    )
}

/// The diff-marker prefix of a pasted row, if any: `+`/`-` + a 4–5-letter
/// anchor + `│`, or the `-` blanked-anchor form (`-` + 4 spaces + `│`,
/// the reference's minus regex). A plain `- item` markdown line has no
/// marker. Returns the prefix length to drop.
#[must_use]
pub(super) fn diff_marker(line: &str) -> Option<usize> {
    let sep = SEP.to_string();
    let (minus, rest) = match line.strip_prefix('+') {
        Some(r) => (false, r),
        None => line.strip_prefix('-').map(|r| (true, r))?,
    };
    if minus && rest.starts_with(&format!("    {SEP}")) {
        return Some(1 + 4 + sep.len());
    }
    // tau's own diff preview renders `+ AbCd│…`/`- AbCd│…` with a space
    // after the sign; the reference's spaceless form is also accepted. A
    // plain `- item` (no anchor after the space) matches neither form.
    let (spaced, rest) = match rest.strip_prefix(' ') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    for n in 4..=5usize {
        let Some(hash) = rest.get(..n) else {
            break;
        };
        if hash.chars().all(|c| c.is_ascii_alphabetic())
            && rest.get(n..n + sep.len()) == Some(sep.as_str())
        {
            return Some(1 + usize::from(spaced) + n + sep.len());
        }
    }
    None
}

/// `true` when any content line carries a diff-preview marker (the strict
/// mode's rejection check; see `EditError::InvalidPatch`).
#[must_use]
pub(super) fn has_diff_markers(content: &str) -> bool {
    content.lines().any(|l| diff_marker(l).is_some())
}

/// Strip pasted diff-preview markers from every content line (the
/// reference's `[W_INVALID_PATCH]` lenient fix). Returns how many lines
/// were touched.
#[must_use]
pub(super) fn strip_diff_markers(content: &mut String) -> usize {
    let mut stripped = 0usize;
    let fixed: Vec<String> = content
        .lines()
        .map(|l| match diff_marker(l) {
            Some(len) => {
                stripped += 1;
                l.get(len..).unwrap_or(l).to_owned()
            }
            None => l.to_owned(),
        })
        .collect();
    *content = fixed.join("\n");
    stripped
}

/// The reference's unicode-escape notice: a literal `\uDDDD` left in the
/// content is a paste artifact — flagged, never auto-corrected.
#[must_use]
pub(super) fn unicode_esc_warning(content: &str) -> Option<String> {
    content
        .lines()
        .any(|l| l.to_lowercase().contains("\\udddd"))
        .then(|| "Detected literal \\uDDDD in edit content; no autocorrection applied.".to_owned())
}
