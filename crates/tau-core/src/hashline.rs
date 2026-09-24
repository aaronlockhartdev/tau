//! Hash-anchored read/edit (spec §5.4, the pi-better-edit scheme): 3-char
//! anchors from a 62-char alphabet where uniqueness is *allocated* (a
//! per-file bitset with collision probing), canon = whitespace-stripped line,
//! survivor reuse across edits, and strict rejection of stale anchors —
//! never fuzzy-matched.

use std::collections::HashMap;
use std::fmt;

/// The 62-char anchor alphabet (A-Z, a-z, 0-9).
const ALPHA: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The full anchor space: 62³ = 238,328.
pub const HASH_SPACE: usize = 62 * 62 * 62;

/// Collision-probing stride (62² + 62 + 1, coprime with the space).
const STRIDE: usize = 62 * 62 + 62 + 1;

/// The line-separator between anchor and content in `read` output.
pub const SEP: char = '│';

/// The 238,328-line hard limit; beyond it the file falls back to `write`.
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

/// A stale or invalid anchor: not found, ambiguous, reversed, malformed,
/// or an attempt to empty a file via edit.
#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    /// The anchor is not in the file's current anchor set.
    Stale { anchor: String },
    /// The file exceeds the 238,328-line anchor cap; fall back to `write`.
    TooLarge,
    /// anchor_from resolves after anchor_to.
    Reversed { from: String, to: String },
    /// Not a bare 3-char anchor from the alphabet.
    BadAnchor(String),
    /// A non-empty file cannot be emptied through edit; use write.
    EmptyFile,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale { anchor } => write!(
                f,
                "stale anchor \"{anchor}\": not in the file's current anchor set. Re-read the file and copy the fresh 3-char anchors (the 3 chars before {SEP})."
            ),
            Self::TooLarge => write!(f, "{TooManyLines}"),
            Self::Reversed { from, to } => write!(
                f,
                "reversed range: \"{from}\" resolves after \"{to}\". Swap from/to."
            ),
            Self::BadAnchor(a) => write!(
                f,
                "invalid anchor \"{a}\": expected a bare 3-char alphanumeric (e.g. \"wUp\")."
            ),
            Self::EmptyFile => write!(f, "cannot empty a non-empty file via edit; use write."),
        }
    }
}

/// canon: the line with all whitespace stripped (reformatting never
/// invalidates an anchor).
/// CRLF (and lone CR) line endings become LF. The reference normalizes on
/// read, so canon, hashing, and the content written back all agree on line
/// endings — an edit never leaves a CRLF file half-converted.
pub fn normalize(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn canon(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
        .collect()
}

fn split_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = content.split('\n').map(str::to_owned).collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

fn idx_to_hash(idx: usize) -> String {
    let mut out = [0u8; 3];
    let mut i = idx;
    for slot in (0..3).rev() {
        out[slot] = ALPHA.as_bytes()[i % ALPHA.len()];
        i /= ALPHA.len();
    }
    String::from_utf8(out.to_vec()).expect("alphabet is ASCII")
}

fn hash_to_idx(hash: &str) -> Option<usize> {
    if hash.len() != 3 {
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
/// (`xxh32(canon) >> 14 mod 62³`) when free, else a probe from the running
/// hint — so every anchor is unique in the file by construction.
pub fn line_hashes(content: &str) -> Result<Vec<String>, TooManyLines> {
    let lines = split_lines(content);
    if lines.len() > HASH_SPACE {
        return Err(TooManyLines);
    }
    let mut used = vec![false; HASH_SPACE];
    let mut hint = 0usize;
    let mut out = Vec::with_capacity(lines.len());
    for line in &lines {
        let base = ((xxhash_rust::xxh32::xxh32(canon(line).as_bytes(), 0) >> 14)
            % HASH_SPACE as u32) as usize;
        let idx = if !used[base] {
            used[base] = true;
            hint = (base + STRIDE) % HASH_SPACE;
            base
        } else {
            let idx = next_zero(&mut used, hint)?;
            used[idx] = true;
            hint = (idx + STRIDE) % HASH_SPACE;
            idx
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
pub struct Edit {
    pub from: String,
    pub to: String,
    pub content: String,
}

/// The edited file: new content plus the new anchor set, where unchanged
/// lines keep their old anchors (survivor reuse, matched by canon + nearest
/// position — stable identity across edits).
#[derive(Debug)]
pub struct Edited {
    pub content: String,
    pub hashes: Vec<String>,
    /// 1-based line numbers of the changed region in the new file.
    pub first_changed: usize,
    pub last_changed: usize,
}

/// Re-derive anchors after an edit: survivors keep their anchors, fresh
/// lines are allocated, and a deleted line's slot stays used for this edit
/// only — cross-edit tombstone persistence (the reference's hash store) is
/// outside v0's per-edit allocation scope, so a later edit may re-allocate a
/// freed slot.
fn stable_hashes(
    old_content: &str,
    old_hashes: &[String],
    new_content: &str,
    removed: &[String],
) -> Result<Vec<String>, TooManyLines> {
    let old = split_lines(old_content);
    let new = split_lines(new_content);
    let mut used = vec![false; HASH_SPACE];
    let mut old_pos: HashMap<String, usize> = HashMap::new();
    for (i, h) in old_hashes.iter().enumerate() {
        if let Some(idx) = hash_to_idx(h) {
            used[idx] = true;
        }
        old_pos.insert(h.clone(), i);
    }
    let removed_pos: Vec<usize> = removed
        .iter()
        .filter_map(|h| old_pos.get(h).copied())
        .collect();

    let (mut span_start, mut span_end) = (usize::MAX, 0usize);
    for i in &removed_pos {
        span_start = span_start.min(*i);
        span_end = span_end.max(*i);
    }
    if removed_pos.is_empty() {
        span_start = old.len();
    }
    let span_len = if span_end >= span_start {
        span_end - span_start + 1
    } else {
        0
    };
    // Survivors after the span shift by the net line-count change.
    let shift = new.len() as i64 - old.len() as i64 + span_len as i64 - span_len as i64;

    // Candidate new positions per old canon (in order).
    let mut by_canon: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, line) in new.iter().enumerate() {
        by_canon.entry(canon(line)).or_default().push(i);
    }

    let mut new_hashes: Vec<Option<String>> = vec![None; new.len()];
    let mut hint = 0usize;
    for (i, h) in old_hashes.iter().enumerate() {
        if removed_pos.contains(&i) {
            continue;
        }
        if let Some(idx) = hash_to_idx(h) {
            hint = hint.max((idx + STRIDE) % HASH_SPACE);
        }
        let Some(candidates) = by_canon.get_mut(&canon(&old[i])) else {
            continue;
        };
        let target = if i > span_end {
            (i as i64 + shift).clamp(0, new.len() as i64) as usize
        } else {
            i
        };
        // Nearest candidate to the survivor's target position.
        let pos = match candidates.binary_search(&target) {
            Ok(p) => p,
            Err(p) => {
                if p == 0 {
                    0
                } else if p == candidates.len() {
                    p - 1
                } else {
                    let (l, r) = (candidates[p - 1], candidates[p]);
                    if target - l <= r - target { p - 1 } else { p }
                }
            }
        };
        if pos < candidates.len() {
            let position = candidates[pos];
            candidates.remove(pos);
            new_hashes[position] = Some(h.clone());
        }
    }

    for i in 0..new.len() {
        if new_hashes[i].is_some() {
            continue;
        }
        let base = ((xxhash_rust::xxh32::xxh32(canon(&new[i]).as_bytes(), 0) >> 14)
            % HASH_SPACE as u32) as usize;
        let idx = if !used[base] {
            used[base] = true;
            hint = (base + STRIDE) % HASH_SPACE;
            base
        } else {
            let idx = next_zero(&mut used, hint)?;
            used[idx] = true;
            hint = (idx + STRIDE) % HASH_SPACE;
            idx
        };
        new_hashes[i] = Some(idx_to_hash(idx));
    }
    Ok(new_hashes
        .into_iter()
        .map(|h| h.expect("all lines anchored"))
        .collect())
}

/// Apply one edit to `content`: resolve the anchors strictly (stale/ambiguous
/// are rejected, never fuzzy-matched), replace the range, and return the new
/// file with stable anchors.
pub fn apply_edit(content: &str, edit: &Edit) -> Result<Edited, EditError> {
    if content.is_empty() {
        return do_edit(content, edit, &[]);
    }
    let hashes = line_hashes(content).map_err(|_| EditError::TooLarge)?;
    do_edit(content, edit, &hashes)
}

fn do_edit(content: &str, edit: &Edit, hashes: &[String]) -> Result<Edited, EditError> {
    let resolve = |anchor: &str| -> Result<usize, EditError> {
        let positions: Vec<usize> = (0..hashes.len())
            .filter(|&i| hashes[i] == *anchor)
            .collect();
        match positions.as_slice() {
            [] => Err(EditError::Stale {
                anchor: anchor.to_owned(),
            }),
            [pos] => Ok(*pos),
            // Allocation makes every anchor unique in the file, so a repeat is
            // structurally impossible; fail loudly if the invariant breaks
            // rather than fuzzy-match.
            _ => unreachable!("anchor set is unique by allocation"),
        }
    };
    let start = resolve(&edit.from)?;
    let end = resolve(&edit.to)?;
    if start > end {
        return Err(EditError::Reversed {
            from: edit.from.clone(),
            to: edit.to.clone(),
        });
    }
    let lines = split_lines(content);
    let was_nonempty = !content.is_empty();
    let replacement = if edit.content.is_empty() {
        Vec::new()
    } else {
        let mut r: Vec<String> = edit.content.split('\n').map(str::to_owned).collect();
        if r.last().is_some_and(|l| l.is_empty()) {
            r.pop();
        }
        r
    };
    if was_nonempty && start == 0 && end == lines.len() - 1 && replacement.is_empty() {
        return Err(EditError::EmptyFile);
    }
    let mut new_lines = lines[..start].to_vec();
    new_lines.extend(replacement.iter().cloned());
    new_lines.extend(lines[end + 1..].iter().cloned());
    let mut new_content = new_lines.join("\n");
    if content.ends_with('\n') && !new_lines.is_empty() {
        new_content.push('\n');
    }
    let removed: Vec<String> = hashes[start..=end].to_vec();
    let new_hashes =
        stable_hashes(content, hashes, &new_content, &removed).map_err(|_| EditError::TooLarge)?;
    // The changed region, measured against the old lines from both ends.
    let min_len = new_lines.len().min(lines.len());
    let mut lo = 0;
    while lo < min_len && new_lines[lo] == lines[lo] {
        lo += 1;
    }
    let mut hi = 0;
    while hi < min_len - lo && new_lines[new_lines.len() - 1 - hi] == lines[lines.len() - 1 - hi] {
        hi += 1;
    }
    Ok(Edited {
        content: new_content,
        hashes: new_hashes,
        first_changed: lo + 1,
        last_changed: new_lines.len().saturating_sub(hi),
    })
}

#[cfg(test)]
#[cfg(test)]
mod properties;

#[cfg(test)]
mod tests;
