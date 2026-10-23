//! The `read` page's rendering: the model-facing anchored rows, their
//! anchor-stripped GUI twin, and the integrity baseline. `impls.rs`
//! `read_page` resolves the offset; this module shapes the page.

use crate::hashline;

use super::impls::{READ_MAX_BYTES, READ_MAX_LINE_BYTES};

/// A `read` page: the model-facing text (hash-anchored rows + hints) and its
/// anchor-stripped twin for the GUI card (the `ui_detail` channel, #243):
/// the same rows without the `HASH│` prefixes, the same hints — the anchors
/// are a model↔tool protocol, not information for a human.
#[derive(Debug)]
pub(super) struct ReadPage {
    pub text: String,
    pub stripped: String,
}

/// The output of a rendered read page: the model-facing anchored rows plus
/// their anchor-stripped twin (#243), the served (anchor, checksum) baseline,
/// the oversized line numbers, and the byte count of the rendered output.
pub(super) struct PageRender {
    pub out: Vec<String>,
    pub stripped: Vec<String>,
    pub served: Vec<(String, String)>,
    pub oversized: Vec<usize>,
    pub out_bytes: usize,
}

/// The rendered page's two texts (#243): the model-facing anchored rows plus
/// hints, and the anchor-stripped twin the GUI card renders. The stripped rows
/// arrive pre-stripped from [`render_page`] (built row-by-row, where the anchor
/// and content are already separate); this joins them and appends the same
/// truncation/oversized/reclaim hints to both, so the card and the model's page
/// never drift.
pub(super) fn assemble_page_text(
    page: PageRender,
    shown: usize,
    start_line: usize,
    total: usize,
    reclaim: Option<String>,
    path: &str,
) -> ReadPage {
    let PageRender {
        out,
        stripped,
        served,
        oversized,
        out_bytes,
    } = page;
    let mut text = out.join("\n");
    let mut stripped = stripped.join("\n");
    let end_display = start_line + shown.saturating_sub(1);
    if shown == 0 {
        // The byte cap cut the page before any row fit.
        let hint = "\n… truncated (output cap 1 MiB)";
        text.push_str(hint);
        stripped.push_str(hint);
    } else if end_display < total {
        let last_anchor = served.last().map(|(h, _)| h.as_str()).unwrap_or_default();
        let hint = format!(
            "\n… truncated (showing lines {start_line}–{end_display} of {total} — {out_bytes} bytes). Use offset={last_anchor} to continue."
        );
        text.push_str(&hint);
        // The hint's next-page offset is the anchor — meaningless to a human,
        // but the line-count context is not: the stripped twin keeps it as-is.
        stripped.push_str(&hint);
    }
    if !oversized.is_empty() {
        // The reference's summary: name the lines and hand over the exact
        // command that inspects them.
        let addresses: Vec<String> = oversized
            .iter()
            .take(100)
            .map(|n| format!("{n}p"))
            .collect();
        let more = oversized.len() - 100.min(oversized.len());
        let hint = format!(
            "\n[{} oversized line(s) over 100 KiB; inspect with bash: sed -n '{}' {} | head -c {}{}]",
            oversized.len(),
            addresses.join(";"),
            path,
            READ_MAX_LINE_BYTES,
            if more > 0 {
                format!(" ({more} more)")
            } else {
                String::new()
            }
        );
        text.push_str(&hint);
        stripped.push_str(&hint);
    }
    if let Some(w) = reclaim {
        text = format!("{w}\n{text}");
        stripped = format!("{w}\n{stripped}");
    }
    ReadPage { text, stripped }
}

/// Render a page of `rows` (the file's anchors for the selected range) into
/// the model-facing lines: `HASH│content` per row, in file order. Rows over
/// [`READ_MAX_LINE_BYTES`] keep their row (anchor and all — the page's line
/// numbering stays intact, and the anchor stays usable for a targeted edit)
/// but the content is replaced by an inspection hint (parity with the
/// reference's per-line placeholder); they are also listed in `oversized` so
/// the caller can summarise them.
/// `served` is the (anchor, line checksum) pair per *shown* row — the
/// integrity baseline `read` registers (item 1); an oversized row is served
/// its *real* line's checksum, so the placeholder never poisons a retry.
/// The 1 MiB page cap (item 8) drops rows from the end once the accumulated
/// output would exceed it.
pub(super) fn render_page(
    path: &str,
    selected: &[String],
    lines: &[String],
    start_line: usize,
) -> PageRender {
    let mut out: Vec<String> = Vec::new();
    let mut stripped: Vec<String> = Vec::new();
    let mut served: Vec<(String, String)> = Vec::new();
    let mut oversized: Vec<usize> = Vec::new();
    let mut out_bytes = 0usize;
    for (i, pair) in selected.iter().zip(lines.iter()).enumerate() {
        let (hash, line) = pair;
        if line.len() > READ_MAX_LINE_BYTES {
            oversized.push(start_line + i);
            let note = format!(
                "[Line {} is {} bytes, over the 100 KiB cap; content not shown. Use bash: sed -n '{}p' {} | head -c {}]",
                start_line + i,
                line.len(),
                start_line + i,
                path,
                READ_MAX_LINE_BYTES
            );
            let row = format!("{hash}{}{note}", hashline::SEP);
            if out_bytes.saturating_add(row.len()) > READ_MAX_BYTES {
                break;
            }
            out_bytes += row.len();
            out.push(row);
            stripped.push(note);
            served.push((hash.clone(), hashline::line_checksum(line)));
            continue;
        }
        let row = format!("{}{}{line}", hash, hashline::SEP);
        if out_bytes.saturating_add(row.len()) > READ_MAX_BYTES {
            break;
        }
        out_bytes += row.len();
        out.push(row);
        // #243: the anchor-stripped twin, built row-by-row as the anchored
        // rows are (the card's `ui_detail` renders these; the model's text
        // joins `out`). The anchor and separator are never in `line`.
        stripped.push(line.clone());
        served.push((hash.clone(), hashline::line_checksum(line)));
    }
    PageRender {
        out,
        stripped,
        served,
        oversized,
        out_bytes,
    }
}
