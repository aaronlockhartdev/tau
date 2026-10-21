//! The bounded before/after diff in an `edit` result (item 4): the changed
//! region — removed lines, added lines with their fresh anchors, a little
//! unchanged context — never the whole file. Since #243 it rides the GUI-only
//! `ui_detail` channel; the model sees [`model_ack`] instead.

use std::fmt::Write as _;

use crate::hashline::{self, Planned};
/// A success output: a one-line summary plus the bounded diff of the changed
/// region, lenient-fix warnings, and a truncation notice.
pub(super) fn diff_output(path: &str, original: &str, planned: &Planned) -> String {
    // Cap the diff body at ~200 lines; a larger change shows its head + a count.
    const CAP: usize = 200;
    let (first_changed, last_changed) = hashline::changed_region_for(original, &planned.lines);
    if first_changed > last_changed {
        let mut out = format!("edited {path} (no change)");
        for w in &planned.warnings {
            out.push('\n');
            out.push_str(w);
        }
        for (i, e) in &planned.demoted {
            out.push('\n');
            let _ = write!(out, "[H_DEMOTED] edits[{i}] not applied: {e}");
        }
        return out;
    }
    // The changed region in the new file, 0-based.
    let new_lines = &planned.lines;
    let total = new_lines.len();
    let lo = first_changed.saturating_sub(1);
    let hi = last_changed.min(total);
    // Change lines: removed (old) then added (new, carrying the fresh anchor).
    let mut change: Vec<String> =
        Vec::with_capacity(planned.removed_lines.len() + hi.saturating_sub(lo));
    for line in &planned.removed_lines {
        change.push(format!("-{line}"));
    }
    for (line, hash) in new_lines[lo..hi].iter().zip(planned.hashes[lo..hi].iter()) {
        change.push(format!("+ {hash}{}{line}", hashline::SEP));
    }
    // Cap the diff body at ~200 lines; a larger change shows its head + a count.
    let mut body: Vec<String> = Vec::new();
    let mut notice: Option<String> = None;
    if change.len() > CAP {
        body.extend(change.iter().take(CAP).cloned());
        notice = Some(format!("… {} more lines changed", change.len() - CAP));
    } else {
        body.extend(change);
        // A little unchanged context for orientation, kept within the cap.
        let budget = CAP - body.len();
        let mut ctx = Vec::new();
        for line in new_lines.iter().take(lo).rev().take(2) {
            ctx.push(format!(" {line}"));
        }
        for line in new_lines.iter().skip(hi).take(2) {
            if ctx.len() >= 2 {
                break;
            }
            ctx.push(format!(" {line}"));
        }
        body.extend(ctx.into_iter().take(budget));
    }
    let summary = format!("edited {path} (lines {first_changed}–{last_changed} of {total})");
    let mut out = summary + "\n";
    out.push_str(&body.join("\n"));
    for w in &planned.warnings {
        out.push('\n');
        out.push_str(w);
    }
    // Demoted members (#207 §5): the batch applied its valid members; each
    // skipped one is reported with its reason so the model can retry it.
    for (i, e) in &planned.demoted {
        out.push('\n');
        let _ = write!(out, "[H_DEMOTED] edits[{i}] not applied: {e}");
    }
    if let Some(n) = notice {
        out.push('\n');
        out.push_str(&n);
    }
    out
}

/// The model-facing `edit` ack (spec §5.4, #243): one line of what changed
/// plus the line counts — the diff itself rides the GUI-only `ui_detail`
/// channel, never the model's context. Demotion notices stay model-facing:
/// the model needs them to retry the skipped members.
pub(super) fn model_ack(path: &str, original: &str, planned: &Planned) -> String {
    // Added = the new lines that replace no old line: a same-length
    // replacement reports both sides, a pure deletion reports 0 added.
    let added = planned.lines.len().saturating_sub(
        original
            .lines()
            .count()
            .saturating_sub(planned.removed_lines.len()),
    );
    let mut out = format!(
        "Successfully edited {path}. Added {added} line(s), removed {} line(s).",
        planned.removed_lines.len()
    );
    for w in &planned.warnings {
        out.push('\n');
        out.push_str(w);
    }
    for (i, e) in &planned.demoted {
        let _ = write!(out, "\n[H_DEMOTED] edits[{i}] not applied: {e}");
    }
    out
}
