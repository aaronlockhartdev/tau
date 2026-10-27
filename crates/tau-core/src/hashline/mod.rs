//! Hash-anchored read/edit (spec §5.4, the pi-better-edit scheme): 4-letter
//! anchors from a 52-letter alphabet (52⁴ = 7,311,616 slots, ADR-0030) where
//! uniqueness is *allocated* (a per-file bitset with collision probing),
//! canon = whitespace-stripped line truncated to 500 bytes before hashing,
//! survivor reuse across edits (LCS over per-line checksums), and strict
//! rejection of stale anchors — never fuzzy-matched.
//!
//! The tool path is registry-driven ([`registry`]): a per-session registry
//! owns which anchors the model was shown (`Registry::serve`), the integrity
//! check compares that against the file at edit time
//! ([`EditError::RangeStale`]), and past the 7,311,616-line cap a read
//! reclaims the coldest file's anchors instead of failing. The pure
//! [`plan_edits`] / [`apply_edits`] core stays registry-free (property tests,
//! and a no-registry caller).

mod diagnostics;
pub mod encoding;
pub mod line_endings;
pub mod pin_store;
pub mod registry;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_diag;
#[cfg(test)]
mod tests_store;

use std::collections::HashMap;
use std::fmt;

/// The 52-letter anchor alphabet (A-Z, a-z; no digits — tokenizer stability,
/// ADR-0030).
const ALPHA: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The full anchor space: 52⁴ = 7,311,616.
pub const HASH_SPACE: usize = 52 * 52 * 52 * 52;

/// Collision-probing stride (52⁴ - 3 ≡ -3 mod the space; coprime with it).
const STRIDE: usize = HASH_SPACE - 3;

/// The hash source is the canon truncated to this many bytes (minified lines
/// are bounded; two lines sharing a 500-byte canon prefix share a base slot
/// — allocation probing keeps anchors unique, ADR-0030).
pub const MAX_HASH_SOURCE_BYTES: usize = 500;

/// How many rows an [`EditError::RangeStale`] diagnostic shows before it
/// caps and points the model at `read` for the rest.
pub const MAX_RANGE_STALE_LINES: usize = 100;

/// The 500-byte hash source, truncated at a UTF-8 char boundary.
#[must_use]
pub fn hash_source(line: &str) -> String {
    let canon = canon(line);
    let mut end = canon.len().min(MAX_HASH_SOURCE_BYTES);
    while !canon.is_char_boundary(end) {
        end -= 1;
    }
    canon.get(..end).expect("end is a char boundary").to_owned()
}

/// The line's base slot: `xxh32(hash_source) >> 14 mod 52´` (ADR-0030) —
/// the allocation base for a line's content, shared by the pure allocator
/// ([`line_hashes`]), the post-edit re-anchor, and the registry's
/// [`registry::Registry::allocate`] mint.
#[must_use]
pub fn line_base_idx(line: &str) -> usize {
    // 52^4 hash space: a u32 holds it, so the narrowing is safe.
    usize::try_from(xxhash_rust::xxh32::xxh32(hash_source(line).as_bytes(), 0) >> 14)
        .expect("u32 fits in usize on the supported 64-bit targets")
        % HASH_SPACE
}

/// The integrity checksum of a line: xxh64 of the 500-byte hash source,
/// hex. The registry records it at serve time and the integrity check
/// recomputes it at edit time — a mismatch is file drift, not anchor drift.
#[must_use]
pub fn line_checksum(line: &str) -> String {
    format!(
        "{:016x}",
        xxhash_rust::xxh64::xxh64(hash_source(line).as_bytes(), 0)
    )
}

/// The line-separator between anchor and content in `read` output.
pub const SEP: char = '│';

/// The 7,311,616-line hard limit; beyond it the file falls back to `write`
/// (the registry path reclaims first, so this surfaces mainly on the pure
/// path).
#[derive(Debug)]
pub struct TooManyLines;

impl fmt::Display for TooManyLines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "file exceeds the {HASH_SPACE}-line limit for hash anchors; use write or a non-line-based approach"
        )
    }
}

/// A stale or invalid anchor: not found, unowned (ownership gate),
/// reversed, malformed, a stale range, a plan/commit drift, or an attempt to
/// empty a file via edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The anchor is not in the file's current anchor set.
    Stale {
        anchor: String,
        /// The resolved other bound, with its ±1-line neighbourhood
        /// re-anchored: the model sees where things are without a re-read
        /// (parity with the reference's `fmtMismatchWithHashes`).
        context: Option<StaleContext>,
    },
    /// The file exceeds the 7,311,616-line anchor cap; fall back to `write`.
    TooLarge,
    /// `anchor_from` resolves after `anchor_to` (strict mode; lenient mode
    /// swaps the range with a warning).
    Reversed { from: String, to: String },
    /// Not a bare 4-letter anchor from the alphabet.
    BadAnchor(String),
    /// A non-empty file cannot be emptied through edit; use write.
    EmptyFile,
    /// Two edits in one call target intersecting or touching ranges.
    Overlapping {
        a: usize,
        b: usize,
        from: usize,
        to: usize,
    },
    /// `content` carries HASH│ prefixes — the model pasted read output
    /// (strict mode; lenient mode strips the prefixes with a warning).
    AnchoredContent { at: usize },
    /// `content` carries diff-preview markers (`+`/`-` rows) — the model
    /// pasted a diff (strict mode; lenient mode strips them with a warning).
    InvalidPatch { at: usize },
    /// The range's lines no longer match what the session showed the model:
    /// the file drifted after the read. `rows` carries the range with fresh
    /// anchors inline; the model retries with them, no read needed.
    RangeStale {
        edit: usize,
        /// The file the range is in: the continuation hint names it.
        path: String,
        from: usize,
        to: usize,
        /// Fresh `HASH│line` rows for the range (capped at
        /// [`MAX_RANGE_STALE_LINES`]).
        rows: Vec<String>,
        /// Fresh anchor → checksum for the shown rows: the retry's integrity
        /// baseline.
        fresh: Vec<(String, String)>,
        /// `true` when the range was capped (the model must read the rest).
        capped: bool,
    },
    /// The file changed between planning and writing the edit; nothing was
    /// written.
    OpAborted { reason: String },
    /// The session does not own the anchor (never minted in this session, or
    /// already freed): the pre-plan ownership gate's rejection (parity with
    /// the reference's `resolveEditTarget`). `case_hint` is the case-folded
    /// candidate clause ("" when there is none).
    NotOwned { anchor: String, case_hint: String },
    /// The edit's anchors are owned by different files: one file per call.
    CrossFile { files: Vec<String> },
    /// The anchor is owned, but by a file other than the edit's target.
    OwnedElsewhere { anchors: Vec<String>, owner: String },
}

/// The resolved other bound a stale diagnostic points at: where it is, with
/// its ±1-line neighbourhood re-anchored (fresh `HASH│line` rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleContext {
    /// The other bound's anchor.
    pub anchor: String,
    /// 1-based line of the other bound.
    pub line: usize,
    /// The neighbourhood rows, `HASH│<clipped line>` each, line-labelled.
    pub rows: Vec<String>,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale { anchor, context } => {
                write!(
                    f,
                    "[E_STALE_ANCHOR] stale anchor \"{anchor}\": the file changed since read. Call read() for fresh anchors."
                )?;
                if let Some(ctx) = context {
                    write!(
                        f,
                        "\n{}",
                        diagnostics::stale_context_block(&ctx.anchor, ctx.line, &ctx.rows)
                    )?;
                }
                Ok(())
            }
            Self::TooLarge => write!(f, "{TooManyLines}"),
            Self::NotOwned { anchor, case_hint } => write!(
                f,
                "[E_STALE_ANCHOR] \"{anchor}\" is not owned in this session.{case_hint} Call read() on the target file first."
            ),
            Self::CrossFile { files } => write!(
                f,
                "[E_BAD_SHAPE] The anchors are owned by different files ({}); edit one file per call.",
                files.join(", ")
            ),
            Self::OwnedElsewhere { anchors, owner } => {
                let refs = anchors
                    .iter()
                    .map(|a| format!("\"{a}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                let (verb, noun) = match anchors.len() {
                    1 => ("is", "that anchor"),
                    _ => ("are", "those anchors"),
                };
                write!(
                    f,
                    "[E_STALE_ANCHOR] {refs} {verb} owned by {owner}. Use a line number, or read {owner} to use {noun}."
                )
            }
            Self::Reversed { from, to } => write!(
                f,
                "[E_RANGE_REVERSED] reversed range: \"{from}\" resolves after \"{to}\"; swap from/to"
            ),
            Self::BadAnchor(a) => write!(f, "{}", diagnostics::bad_ref_message(a)),
            Self::EmptyFile => write!(f, "cannot empty a non-empty file via edit; use write"),
            Self::Overlapping { a, b, .. } => write!(
                f,
                "[E_OVERLAPPING] edits[{a}] and edits[{b}] overlap; make the ranges disjoint"
            ),
            Self::AnchoredContent { at } => write!(
                f,
                "[E_ANCHORED_CONTENT] edits[{at}].content must be unanchored: strip the HASH{SEP} prefixes"
            ),
            Self::InvalidPatch { at } => write!(
                f,
                "[W_INVALID_PATCH] edits[{at}].content carries diff-preview markers (+/-); strip them before the edit"
            ),
            Self::RangeStale {
                edit,
                path,
                from,
                to,
                rows,
                capped,
                ..
            } => {
                write!(
                    f,
                    "[E_RANGE_STALE] the range (lines {from}–{to}) of edits[{edit}] no longer matches what was shown; the file changed after the read. Current range with fresh anchors:\n\n{}",
                    rows.join("\n")
                )?;
                if *capped {
                    write!(
                        f,
                        "\n\n[The range has {} lines; showing the first {}. Call read() on {} with offset={} to see the rest.]",
                        to - from + 1,
                        rows.len(),
                        path,
                        from + rows.len()
                    )?;
                } else {
                    write!(f, "\n\nRetry with the fresh anchors above without a read.")?;
                }
                Ok(())
            }
            Self::OpAborted { reason } => write!(f, "[E_OP_ABORTED] {reason}"),
        }
    }
}

impl std::error::Error for EditError {}

/// canon: the line with all whitespace stripped (reformatting never
/// invalidates an anchor).
#[must_use]
pub fn canon(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
        .collect()
}

/// LF-normalize: CRLF (and lone CR) line endings become LF. The read path
/// normalizes, so canon, hashing, and the served content agree on line
/// endings; the written bytes keep the file's original endings around the
/// edited span ([`line_endings`]).
#[must_use]
pub fn normalize(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn split_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = content.split('\n').map(str::to_owned).collect();
    if lines.last().is_some_and(std::string::String::is_empty) {
        lines.pop();
    }
    lines
}

pub(crate) fn idx_to_hash(idx: usize) -> String {
    let mut out = [0u8; 4];
    let mut i = idx;
    for slot in (0..4).rev() {
        out[slot] = ALPHA.as_bytes()[i % ALPHA.len()];
        i /= ALPHA.len();
    }
    String::from_utf8(out.to_vec()).expect("alphabet is ASCII")
}

pub(crate) fn hash_to_idx(hash: &str) -> Option<usize> {
    if hash.len() != 4 {
        return None;
    }
    let mut idx = 0usize;
    for ch in hash.bytes() {
        let digit = ALPHA.as_bytes().iter().position(|&b| b == ch)?;
        idx = idx * ALPHA.len() + digit;
    }
    Some(idx)
}

fn next_zero(used: &mut [bool], start: usize) -> Result<usize, TooManyLines> {
    let mut idx = start % HASH_SPACE;
    for _ in 0..HASH_SPACE {
        if !used[idx] {
            return Ok(idx);
        }
        idx = (idx + STRIDE) % HASH_SPACE;
    }
    Err(TooManyLines)
}

/// Allocate a unique anchor per line: the line's base index
/// (`xxh32(hash_source) >> 14 mod 52⁴`) when free, else a probe from the
/// running hint — so every anchor is unique in the file by construction.
pub fn line_hashes(content: &str) -> Result<Vec<String>, TooManyLines> {
    let lines = split_lines(content);
    if lines.len() > HASH_SPACE {
        return Err(TooManyLines);
    }
    let mut used = vec![false; HASH_SPACE];
    let mut hint = 0usize;
    let mut out = Vec::with_capacity(lines.len());
    for line in &lines {
        let base = line_base_idx(line);
        let idx = if used[base] {
            let idx = next_zero(&mut used, hint)?;
            used[idx] = true;
            hint = (idx + STRIDE) % HASH_SPACE;
            idx
        } else {
            used[base] = true;
            hint = (base + STRIDE) % HASH_SPACE;
            base
        };
        out.push(idx_to_hash(idx));
    }
    Ok(out)
}

/// The anchors of `content` as `HASH│line` rows.
pub fn render(content: &str) -> Result<Vec<String>, TooManyLines> {
    let hashes = line_hashes(content)?;
    let lines = split_lines(content);
    Ok(lines
        .iter()
        .zip(hashes)
        .map(|(line, h)| format!("{h}{SEP}{line}"))
        .collect())
}

/// One edit: a `[from, to]` anchor range (inclusive) replaced by `content`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub from: String,
    pub to: String,
    pub content: String,
}

/// The edited file: new content plus the new anchor set, where unchanged
/// lines keep their old anchors (survivor reuse, LCS over per-line
/// checksums — stable identity across edits).
#[derive(Debug)]
pub struct Edited {
    pub content: String,
    pub hashes: Vec<String>,
    /// 1-based line numbers of the changed region in the new file.
    pub first_changed: usize,
    pub last_changed: usize,
    /// The old content of the replaced ranges — the removed lines, for the
    /// bounded before/after diff the edit tool returns.
    pub removed_lines: Vec<String>,
    /// The file's per-line endings after the edit (what the write joins with).
    pub line_endings: Vec<line_endings::Ending>,
}

/// Options for [`plan_edits`].
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Strict input (the reference's `strictInput`): the auto-fixes (reversed
    /// range swap, prefix stripping) become hard rejections.
    pub strict: bool,
    /// The file being edited: the range-stale continuation hint names it.
    pub path: String,
    /// The session's served baseline for the file (anchor → line checksum).
    /// `None` disables the integrity check (pure path, first read in the
    /// same call).
    pub served: Option<HashMap<String, String>>,
    /// The file's current anchor set (in line order) to resolve against. When
    /// present (the registry path) this is the single source of truth; when
    /// absent (the pure path) it is re-derived with `line_hashes`.
    pub hashes: Option<Vec<String>>,
    /// Per-member demotion (#207 §5): a batch that is not fully valid applies
    /// the valid members and reports the failed ones, instead of rejecting the
    /// whole batch. Off by default — the pure path and the strict/lenient
    /// posture keep their atomic-reject contract.
    pub demote: bool,
}
/// A planned (not yet written) edit batch: the resolved ranges, the new file
/// lines and anchors, and the warnings the lenient auto-fixes produced. The
/// tool layer commits it after a drift re-check ([`EditError::OpAborted`]).
#[derive(Debug)]
pub struct Planned {
    /// The new file lines (LF-normalized, unjoined).
    pub lines: Vec<String>,
    /// The new anchor set (survivors kept, fresh lines allocated).
    pub hashes: Vec<String>,
    /// The new file's per-line endings.
    pub line_endings: Vec<line_endings::Ending>,
    /// `true` when the original file ends with a terminator.
    pub trailing: bool,
    /// Warnings from the lenient auto-fixes (reversed range, stripped
    /// prefixes).
    pub warnings: Vec<String>,
    /// The removed lines, in range order.
    pub removed_lines: Vec<String>,
    /// The resolved ranges (edit index, start, end) in document order.
    pub ranges: Vec<(usize, usize, usize)>,
    /// Members a demote-mode plan skipped, in original order: the edit index
    /// and the reason it was not applied (#207 §5). Empty in the non-demotion
    /// path (a fully valid batch, or a whole-batch reject).
    pub demoted: Vec<(usize, EditError)>,
}

/// The stale-range diagnostic: the range's rows with fresh anchors and the
/// checksums that go with them — the retry's integrity baseline.
fn range_stale(
    edit: usize,
    path: &str,
    lines: &[String],
    hashes: &[String],
    from: usize,
    to: usize,
) -> EditError {
    let range_len = to - from + 1;
    let shown = range_len.min(MAX_RANGE_STALE_LINES);
    let mut rows = Vec::with_capacity(shown);
    let mut fresh = Vec::with_capacity(shown);
    for i in from..from + shown {
        rows.push(format!(
            "{}{SEP}{}",
            hashes[i],
            diagnostics::clip_line(&lines[i])
        ));
        fresh.push((hashes[i].clone(), line_checksum(&lines[i])));
    }
    EditError::RangeStale {
        edit,
        path: path.to_owned(),
        from: from + 1,
        to: to + 1,
        rows,
        fresh,
        capped: range_len > shown,
    }
}

/// The integrity check (item 1): every line of the range must match the
/// checksum the session served it with. A mismatch means the file drifted
/// after the read — the range is reported with fresh anchors inline.
#[allow(clippy::too_many_arguments)] // the path feeds the range-stale cap hint (G5)
fn assert_range_served(
    lines: &[String],
    hashes: &[String],
    served: &HashMap<String, String>,
    edit: usize,
    path: &str,
    from: usize,
    to: usize,
    deletion: bool,
) -> Result<(), EditError> {
    for i in from..=to {
        let Some(served_sum) = served.get(&hashes[i]) else {
            // Lines the session never showed: for a deletion, the interior
            // was not shown to the model (the range was given by anchors) —
            // only the bookends count.
            if deletion && i != from && i != to {
                continue;
            }
            return Err(range_stale(edit, path, lines, hashes, from, to));
        };
        if served_sum != &line_checksum(&lines[i]) {
            return Err(range_stale(edit, path, lines, hashes, from, to));
        }
    }
    Ok(())
}

/// The lenient auto-fixes (item 5): swap a reversed range, and strip the
/// `HASH│` prefixes the model pasted from read output. In strict mode each
/// fix is a hard rejection instead.
fn lenient_fixes(
    edits: &mut [Edit],
    hashes: &[String],
    opts: &PlanOptions,
) -> Result<Vec<String>, EditError> {
    let mut warnings = Vec::new();
    for (i, edit) in edits.iter_mut().enumerate() {
        // Reversed range: swap, with a warning the model can audit. Bounds
        // that don't both resolve are left for the resolution phase, which
        // owns the stale/overlapping diagnostics (incl. the other-bound
        // context) — an early reject here would pre-empt the richer message.
        let (Ok(from_line), Ok(to_line)) = (
            resolve_anchor(hashes, &edit.from),
            resolve_anchor(hashes, &edit.to),
        ) else {
            continue;
        };
        if from_line > to_line {
            if opts.strict {
                return Err(EditError::Reversed {
                    from: edit.from.clone(),
                    to: edit.to.clone(),
                });
            }
            std::mem::swap(&mut edit.from, &mut edit.to);
            warnings.push(format!(
                "[W_RANGE_REVERSED] edits[{i}]: \"from\" referred to a line after \"to\"; the range was swapped to document order."
            ));
        }
        // Pasted prefixes: strip every line that carries one.
        let stripped = strip_anchored_content(&mut edit.content, i, opts)?;
        if !stripped.is_empty() {
            warnings.extend(stripped);
        }
        // Pasted diff-preview markers (`+HASH│` / `-HASH│` rows): the model
        // pasted a diff. Strip with a warning (the reference's
        // `[W_INVALID_PATCH]`); strict mode rejects.
        if opts.strict {
            if diagnostics::has_diff_markers(&edit.content) {
                return Err(EditError::InvalidPatch { at: i });
            }
        } else {
            let n = diagnostics::strip_diff_markers(&mut edit.content);
            if n > 0 {
                warnings.push(format!(
                    "[W_INVALID_PATCH] edits[{i}]: stripped diff-preview markers from {n} content line(s)."
                ));
            }
        }
        // A literal \uDDDD is a paste artifact: flagged, never auto-corrected.
        if let Some(w) = diagnostics::unicode_esc_warning(&edit.content) {
            warnings.push(w);
        }
    }
    Ok(warnings)
}

/// Strip `HASH│` prefixes from `content`; returns the warnings. In strict
/// mode a pasted prefix is a hard rejection.
fn strip_anchored_content(
    content: &mut String,
    at: usize,
    opts: &PlanOptions,
) -> Result<Vec<String>, EditError> {
    let any_prefixed = content.lines().any(|l| {
        l.split_once(SEP)
            .is_some_and(|(h, _)| hash_to_idx(h).is_some())
    });
    if !any_prefixed {
        return Ok(Vec::new());
    }
    if opts.strict {
        return Err(EditError::AnchoredContent { at });
    }
    let fixed: Vec<String> = content
        .lines()
        .map(|l| match l.split_once(SEP) {
            Some((h, rest)) if hash_to_idx(h).is_some() => rest.to_owned(),
            _ => l.to_owned(),
        })
        .collect();
    *content = fixed.join("\n");
    Ok(vec![format!(
        "[W_BARE_HASH_PREFIX] edits[{at}]: stripped pasted HASH{SEP} prefixes from the content."
    )])
}

/// Re-derive anchors after an edit, LCS over per-line checksums (item 6):
/// a new line whose checksum matches an old line's, in order, keeps the old
/// anchor — unchanged lines in the replacement keep their exact anchors,
/// and moves/deletions are detected by checksum, not by canon +
/// nearest-position heuristics.
fn lcs_reanchor(
    old_lines: &[String],
    old_hashes: &[String],
    new_lines: &[String],
) -> Result<Vec<String>, TooManyLines> {
    // The caller's anchor set (the registry snapshot) can be shorter than the
    // file when lines were appended out-of-band since the last app write;
    // the walk must cover no more lines than there are anchors to re-use, or
    // `old_hashes[i]` below runs off the end.
    let old_sums: Vec<String> = old_lines
        .iter()
        .take(old_hashes.len())
        .map(|l| line_checksum(l))
        .collect();
    let new_sums: Vec<String> = new_lines.iter().map(|l| line_checksum(l)).collect();

    // In-order checksum matching (the LCS walk): for each new line, find the
    // next old line (after the match cursor) with the same checksum.
    let mut cursor = 0usize;
    let mut matched: Vec<Option<usize>> = vec![None; new_lines.len()];
    for (j, s) in new_sums.iter().enumerate() {
        while cursor < old_sums.len() && old_sums[cursor] != *s {
            cursor += 1;
        }
        if cursor < old_sums.len() && old_sums[cursor] == *s {
            matched[j] = Some(cursor);
            cursor += 1;
        }
    }

    let mut used = vec![false; HASH_SPACE];
    for h in old_hashes {
        if let Some(idx) = hash_to_idx(h) {
            used[idx] = true;
        }
    }
    let mut new_hashes: Vec<Option<String>> = vec![None; new_lines.len()];
    for (j, m) in matched.iter().enumerate() {
        if let Some(i) = *m {
            new_hashes[j] = Some(old_hashes[i].clone());
        }
    }
    let mut hint = 0usize;
    for i in 0..new_lines.len() {
        if new_hashes[i].is_some() {
            continue;
        }
        let base = line_base_idx(&new_lines[i]);
        let idx = if used[base] {
            let idx = next_zero(&mut used, hint)?;
            used[idx] = true;
            hint = (idx + STRIDE) % HASH_SPACE;
            idx
        } else {
            used[base] = true;
            hint = (base + STRIDE) % HASH_SPACE;
            base
        };
        new_hashes[i] = Some(idx_to_hash(idx));
    }
    Ok(new_hashes
        .into_iter()
        .map(|h| h.expect("all lines anchored"))
        .collect())
}

/// Resolve `anchor` to a 0-based line position in `hashes`. A duplicate
/// anchor (a broken set) resolves to the first occurrence — the reference
/// implementation's deterministic behavior — never an error.
fn resolve_anchor(hashes: &[String], anchor: &str) -> Result<usize, EditError> {
    let Some(_idx) = hash_to_idx(anchor) else {
        return Err(EditError::BadAnchor(anchor.to_owned()));
    };
    match hashes.iter().position(|h| h == anchor) {
        Some(pos) => Ok(pos),
        None => Err(EditError::Stale {
            anchor: anchor.to_owned(),
            context: None,
        }),
    }
}

/// Resolve with the other bound as diagnostic context: when this anchor is
/// stale but the other resolved, the error carries the other's line.
fn resolve_anchor_with_context(
    lines: &[String],
    hashes: &[String],
    other: &str,
    anchor: &str,
) -> Result<usize, EditError> {
    match resolve_anchor(hashes, anchor) {
        Ok(pos) => Ok(pos),
        Err(EditError::Stale { ref anchor, .. }) => {
            // The other bound resolved: point the diagnostic at it, with its
            // ±1-line neighbourhood re-anchored so the model recovers without
            // a re-read (parity with the reference's `fmtMismatchWithHashes`).
            let context = resolve_anchor(hashes, other).ok().map(|i| {
                let from = i.saturating_sub(1);
                let to = (i + 1).min(hashes.len() - 1);
                let rows = (from..=to)
                    .map(|j| {
                        format!(
                            "    {}: {}{SEP}{}",
                            j + 1,
                            hashes[j],
                            diagnostics::clip_line(&lines[j])
                        )
                    })
                    .collect();
                StaleContext {
                    anchor: other.to_owned(),
                    line: i + 1,
                    rows,
                }
            });
            Err(EditError::Stale {
                anchor: anchor.clone(),
                context,
            })
        }
        Err(e) => Err(e),
    }
}

/// The changed region, measured against the old lines from both ends
/// (1-based bounds in the new file). A net deletion shrinks `new`, so the
/// suffix walk can consume the whole tail and invert the bounds (first >
/// last) — which the tool layer reads as "no change" (#235). When that
/// happens with a net length delta, the deletion has no positional
/// counterpart, so anchor the region one line wide at the change point;
/// adds and same-length replaces are untouched.
fn changed_region(old: &[String], new: &[String]) -> (usize, usize) {
    let min_len = old.len().min(new.len());
    let mut lo = 0;
    while lo < min_len && new[lo] == old[lo] {
        lo += 1;
    }
    let mut hi = 0;
    while hi < min_len.saturating_sub(lo) && new[new.len() - 1 - hi] == old[old.len() - 1 - hi] {
        hi += 1;
    }
    let (first, last) = (lo + 1, new.len().saturating_sub(hi));
    if first > last && old.len() != new.len() {
        // #235: a pure deletion — the removed line has no positional
        // counterpart, so report one line at the change point (clamped into
        // the new file: a tail deletion leaves `lo` at the full length).
        let pos = (lo + 1).min(new.len());
        return (pos, pos);
    }
    (first, last)
}

/// The changed region of a batch, given the old file content (LF-normalized)
/// and the planned new lines — the 1-based bounds the tool output reports.
#[must_use]
pub fn changed_region_for(old_content: &str, new_lines: &[String]) -> (usize, usize) {
    changed_region(&split_lines(old_content), new_lines)
}

/// The file's trailing-terminator state (LF-normalized view).
fn trailing_of(content: &str) -> bool {
    !content.is_empty() && content.ends_with('\n')
}

/// Plan a batch of edits against `content`: lenient auto-fixes (or strict
/// rejections), strict anchor resolution, overlap rejection, and the
/// integrity check against `opts.served`. Returns the new file's lines,
/// anchors, endings, and warnings — not yet written.
#[allow(clippy::too_many_lines)] // one resolution pass over the batch
pub fn plan_edits(
    content: &str,
    file_ending: line_endings::Ending,
    edits: &[Edit],
    opts: &PlanOptions,
) -> Result<Planned, EditError> {
    let lines = split_lines(content);
    let hashes = match &opts.hashes {
        Some(h) => h.clone(),
        None => {
            if content.is_empty() {
                Vec::new()
            } else {
                line_hashes(content).map_err(|_| EditError::TooLarge)?
            }
        }
    };

    let mut fixed_edits = edits.to_vec();
    let warnings = lenient_fixes(&mut fixed_edits, &hashes, opts)?;

    // Resolve each member's anchors against the same snapshot. In demote mode
    // (#207 §5) a member that cannot resolve is *demoted* (recorded, skipped)
    // and the rest proceed; otherwise the first failure rejects the whole
    // batch (the atomic-reject contract).
    let mut ranges: Vec<(usize, usize, usize)> = Vec::with_capacity(edits.len());
    let mut demoted: Vec<(usize, EditError)> = Vec::new();
    for (i, edit) in fixed_edits.iter().enumerate() {
        let start = resolve_anchor_with_context(&lines, &hashes, &edit.to, &edit.from);
        let end = resolve_anchor_with_context(&lines, &hashes, &edit.from, &edit.to);
        match (start, end) {
            (Ok(start), Ok(end)) => ranges.push((i, start, end)),
            (Err(e), _) | (_, Err(e)) => {
                if opts.demote {
                    demoted.push((i, e));
                } else {
                    return Err(e);
                }
            }
        }
    }

    // Sort by start position; stable, so equal starts keep original order.
    ranges.sort_by_key(|&(_, start, _)| start);

    // Reject overlapping or touching ranges (inclusive bounds). A new range
    // can only overlap the most recent kept one (the set is start-sorted and
    // non-overlapping), so a last-element check suffices. In demote mode the
    // later member of a collision is demoted; otherwise the batch rejects.
    let mut kept: Vec<(usize, usize, usize)> = Vec::with_capacity(ranges.len());
    for r in ranges {
        if let Some(&(a, _, a_end)) = kept.last()
            && a_end + 1 >= r.1
        {
            let err = EditError::Overlapping {
                a,
                b: r.0,
                from: a_end,
                to: r.1,
            };
            if opts.demote {
                demoted.push((r.0, err));
            } else {
                return Err(err);
            }
            continue;
        }
        kept.push(r);
    }

    // The integrity check: the kept ranges must match what the session showed.
    if let Some(served) = &opts.served {
        let mut checked = Vec::with_capacity(kept.len());
        for &(i, start, end) in &kept {
            match assert_range_served(
                &lines,
                &hashes,
                served,
                i,
                &opts.path,
                start,
                end,
                fixed_edits[i].content.is_empty(),
            ) {
                Ok(()) => checked.push((i, start, end)),
                Err(e) => {
                    if opts.demote {
                        demoted.push((i, e));
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        kept = checked;
    }
    let ranges = kept;

    // Splice: walk the sorted ranges, carrying unchanged lines between them.
    let mut new_lines: Vec<String> = Vec::new();
    let mut removed_lines: Vec<String> = Vec::new();
    let mut cursor = 0;
    for &(orig, start, end) in &ranges {
        new_lines.extend(lines[cursor..start].iter().cloned());
        if !fixed_edits[orig].content.is_empty() {
            let mut r: Vec<String> = fixed_edits[orig]
                .content
                .split('\n')
                .map(str::to_owned)
                .collect();
            if r.last().is_some_and(String::is_empty) {
                r.pop();
            }
            new_lines.extend(r);
        }
        for line in &lines[start..=end] {
            removed_lines.push(line.clone());
        }
        cursor = end + 1;
    }
    new_lines.extend(lines[cursor..].iter().cloned());

    if !content.is_empty() && new_lines.is_empty() {
        // An edit that empties a file is rejected in every mode — the
        // resulting state is invalid, so there is no demotion to apply.
        return Err(EditError::EmptyFile);
    }

    let new_hashes = lcs_reanchor(&lines, &hashes, &new_lines).map_err(|_| EditError::TooLarge)?;
    // Per-line endings of the new file: apply every range to the original
    // endings in document order (each application shifts the later spans).
    let file_endings = vec![file_ending; lines.len()];
    let mut line_endings = file_endings;
    for &(orig, start, end) in &ranges {
        let count = fixed_edits[orig].content.lines().count();
        let count = if fixed_edits[orig].content.is_empty() {
            0
        } else {
            count
        };
        line_endings = line_endings::map_endings(&line_endings, start, end, count);
    }

    Ok(Planned {
        lines: new_lines,
        hashes: new_hashes,
        line_endings,
        trailing: trailing_of(content),
        warnings,
        removed_lines,
        ranges,
        demoted,
    })
}

/// The old `apply_edits`: plan + assemble in one step (the pure,
/// registry-free path — property tests and non-session callers). Strict
/// semantics: no auto-fixes, no integrity check.
pub fn apply_edits(content: &str, edits: &[Edit]) -> Result<Edited, EditError> {
    apply_edits_opts(content, edits, &PlanOptions::default())
}

/// [`apply_edits`] with options.
pub fn apply_edits_opts(
    content: &str,
    edits: &[Edit],
    opts: &PlanOptions,
) -> Result<Edited, EditError> {
    let raw_ending = line_endings::detect(content);
    let planned = plan_edits(content, raw_ending, edits, opts)?;
    let (first_changed, last_changed) = changed_region(&split_lines(content), &planned.lines);
    let content = line_endings::join(&planned.lines, raw_ending, planned.trailing);
    Ok(Edited {
        content,
        hashes: planned.hashes,
        first_changed,
        last_changed,
        removed_lines: planned.removed_lines,
        line_endings: planned.line_endings,
    })
}

/// Apply one edit to `content`: resolve the anchors strictly (stale/ambiguous
/// are rejected, never fuzzy-matched), replace the range, and return the new
/// file with stable anchors.
pub fn apply_edit(content: &str, edit: &Edit) -> Result<Edited, EditError> {
    apply_edits(content, std::slice::from_ref(edit))
}

/// Assemble a planned edit into the file's bytes: the new lines joined with
/// the file's endings, the trailing terminator preserved.
#[must_use]
pub fn assemble(plan: &Planned) -> String {
    let ending = plan
        .line_endings
        .first()
        .copied()
        .unwrap_or(line_endings::Ending::Lf);
    line_endings::join(&plan.lines, ending, plan.trailing)
}
