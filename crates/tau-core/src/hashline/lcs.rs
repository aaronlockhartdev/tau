//! The one sequence-alignment primitive in the crate (#298).
//!
//! Two callers need the same answer: given the checksums of an old file and
//! the checksums of a new one, which new line carries which old line. The
//! registry when a file is re-read after changing on disk, the pin store when
//! it re-anchors its pins, and `mod.rs` when an edit re-anchors the lines that
//! survived it. It used to be answered three times over two algorithms — a
//! greedy cursor walk for the app-write path, a dynamic-programming table for
//! the other two — and the two disagreed on 57,743 of 200,000 random pairs
//! (#298, measured at `main` `bb4d9d9`), so the same content re-anchored
//! differently depending on the path taken through it. This answers it once,
//! maximally.

/// The maximal in-order alignment of `old` to `new`: for each index of `new`,
/// the index of the `old` line it carries, or `None` for a line that carries
/// nothing. "Maximal" is the claim, and it is load-bearing: the carry decides
/// which lines keep their anchors, so a shorter common subsequence means more
/// anchors churn for the same edit.
///
/// The result is a common subsequence — matched indices rise in both
/// sequences and every matched pair holds `old[i] == new[j]` — and no longer
/// common subsequence exists. `the_carry_is_maximal` below pins that against a
/// brute-force oracle; `properties.rs` pins what the registry makes of it.
/// Space is `O(new.len())`: the alignment is built by Hirschberg's divide and
/// conquer, which keeps only two score rows at a time. A full table would be
/// `old.len() * new.len()` scores — 400 MB at 10k lines, which is the shape a
/// session's largest files reach.
pub(crate) fn carry(old: &[String], new: &[String]) -> Vec<Option<usize>> {
    let mut matched = vec![None; new.len()];
    if old.is_empty() || new.is_empty() {
        return matched;
    }
    // The bulk of a real edit is unchanged at both ends. Stripping the common
    // prefix and suffix first leaves only the touched window to align, which
    // is what keeps the quadratic walk cheap on a large file.
    let lead = common_prefix(old, new);
    for (slot, i) in matched[..lead].iter_mut().zip(0..lead) {
        *slot = Some(i);
    }
    let mut tail = common_suffix(&old[lead..], &new[lead..]);
    // The trims can overlap. They must leave a middle window of at least one
    // line per side, because each window's indices are set by its own start:
    // an identical file, and a file whose blank last line a real last line
    // matches, are the shapes that overrun.
    tail = tail
        .min((old.len() - lead).saturating_sub(1))
        .min((new.len() - lead).saturating_sub(1));
    // A suffix line sits the same distance from the end of both sequences.
    for (k, slot) in matched[new.len() - tail..].iter_mut().enumerate() {
        *slot = Some(old.len() - tail + k);
    }
    let middle = hirschberg(&old[lead..old.len() - tail], &new[lead..new.len() - tail]);
    for (slot, carried) in matched[lead..new.len() - tail].iter_mut().zip(middle) {
        *slot = carried.map(|i| i + lead);
    }
    matched
}

/// Hirschberg's LCS backtrace: linear space, quadratic time. Splits `old` at
/// its midpoint, scores the two halves against every prefix of `new` from
/// both directions, splits `new` where the scores add up best, and recurses.
fn hirschberg(old: &[String], new: &[String]) -> Vec<Option<usize>> {
    if new.is_empty() {
        return Vec::new();
    }
    if old.is_empty() {
        return vec![None; new.len()];
    }
    if old.len() == 1 {
        // One old line can carry at most one new line, and the choice is
        // forced: its first appearance.
        let mut matched = vec![None; new.len()];
        if let Some(j) = new.iter().position(|s| *s == old[0]) {
            matched[j] = Some(0);
        }
        return matched;
    }
    let mid = old.len() / 2;
    // L[j] = LCS(old[..mid], new[..j]); R[j] = LCS(old[mid..], new[j..]).
    // The split that adds up best is the one on an optimal path.
    let left_row = score_row(&old[..mid], new);
    let rev_old: Vec<String> = old[mid..].iter().rev().cloned().collect();
    let rev_new: Vec<String> = new.iter().rev().cloned().collect();
    let right_row = score_row(&rev_old, &rev_new);
    let split = (0..=new.len())
        .max_by_key(|j| left_row[*j] + right_row[new.len() - *j])
        .unwrap_or(0);
    let mut left = hirschberg(&old[..mid], &new[..split]);
    // The right half indexes `old[mid..]`; its carries shift back into
    // whole-file indices.
    let right = hirschberg(&old[mid..], &new[split..]);
    left.extend(right.into_iter().map(|i| i.map(|i| i + mid)));
    left
}

/// `row[j]` = the length of the LCS of `old` and the first `j` lines of
/// `new`, in two rolling rows: the row is the only state the recurrence needs,
/// which is what makes the whole alignment linear-space.
fn score_row(old: &[String], new: &[String]) -> Vec<usize> {
    let mut prev = vec![0usize; new.len() + 1];
    let mut row = vec![0usize; new.len() + 1];
    for a in old {
        row[0] = 0;
        for (j, b) in new.iter().enumerate() {
            row[j + 1] = if a == b {
                prev[j] + 1
            } else {
                prev[j + 1].max(row[j])
            };
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev
}

fn common_prefix(old: &[String], new: &[String]) -> usize {
    old.iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count()
}

fn common_suffix(old: &[String], new: &[String]) -> usize {
    old.iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn matched_pairs(m: &[Option<usize>]) -> Vec<(usize, usize)> {
        m.iter()
            .enumerate()
            .filter_map(|(j, i)| i.map(|i| (i, j)))
            .collect()
    }

    /// The oracle: the LCS length from the textbook full table — the dynamic
    /// program the registry used to run, kept here as an independent check on
    /// the linear-space implementation.
    fn lcs_len(old: &[String], new: &[String]) -> usize {
        let n = old.len();
        let m = new.len();
        let mut table = vec![vec![0usize; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i][j] = if old[i] == new[j] {
                    table[i + 1][j + 1] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        table[0][0]
    }

    #[test]
    fn an_unchanged_file_carries_every_line() {
        let old = lines(&["a", "b", "c"]);
        assert_eq!(carry(&old, &old), vec![Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn an_insertion_keeps_both_sides() {
        let old = lines(&["a", "b"]);
        let new = lines(&["a", "x", "b"]);
        assert_eq!(carry(&old, &new), vec![Some(0), None, Some(1)]);
    }

    #[test]
    fn a_deletion_keeps_the_rest() {
        let old = lines(&["a", "b", "c"]);
        let new = lines(&["a", "c"]);
        assert_eq!(carry(&old, &new), vec![Some(0), Some(2)]);
    }

    #[test]
    fn a_change_in_the_middle_keeps_the_later_lines() {
        let old = lines(&["a", "b", "c", "d"]);
        let new = lines(&["a", "B", "c", "d"]);
        assert_eq!(carry(&old, &new), vec![Some(0), None, Some(2), Some(3)]);
    }

    #[test]
    fn the_greedy_walk_left_lines_unmatched_that_a_maximal_carry_takes() {
        // The divergence this ticket measured: a greedy cursor walk matches
        // `b` first, then finds no `a` after it and carries one line; a
        // maximal carry takes `a` and `b`.
        let old = lines(&["a", "b"]);
        let new = lines(&["b", "a", "b"]);
        let m = carry(&old, &new);
        assert_eq!(
            matched_pairs(&m).len(),
            lcs_len(&old, &new),
            "the carry is not maximal: {m:?}"
        );
        assert_eq!(matched_pairs(&m).len(), 2);
    }

    #[test]
    fn a_repeat_keeps_its_own_position() {
        // Two identical lines must not both claim the same old line.
        let old = lines(&["x", "y", "x"]);
        let new = lines(&["x", "x"]);
        assert_eq!(matched_pairs(&carry(&old, &new)), vec![(0, 0), (2, 1)]);
    }

    #[test]
    fn an_empty_side_carries_nothing() {
        assert_eq!(carry(&[], &lines(&["a"])), vec![None]);
        assert_eq!(carry(&lines(&["a"]), &[]), Vec::<Option<usize>>::new());
        assert_eq!(carry(&[], &[]), Vec::<Option<usize>>::new());
    }

    #[test]
    fn a_wholesale_replacement_carries_nothing() {
        let old = lines(&["a", "b", "c"]);
        let new = lines(&["x", "y", "z"]);
        assert_eq!(carry(&old, &new), vec![None, None, None]);
    }

    #[test]
    fn the_carry_is_a_common_subsequence() {
        let old = lines(&["a", "b", "c", "d", "e"]);
        let new = lines(&["c", "a", "d", "x", "e"]);
        let m = carry(&old, &new);
        let pairs = matched_pairs(&m);
        assert!(pairs.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
        assert!(pairs.iter().all(|(i, j)| old[*i] == new[*j]));
        assert_eq!(pairs.len(), lcs_len(&old, &new));
    }

    #[test]
    fn an_identical_file_ending_in_a_blank_line_carries_every_line() {
        let file = lines(&["a", "b", "c", ""]);
        assert_eq!(
            carry(&file, &file),
            vec![Some(0), Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn a_trailing_blank_line_is_not_matched_by_a_real_line() {
        // A newline-terminated file ends in a blank line, and `""` carries a
        // checksum like any other line: a real last line must not be aligned
        // onto it, and the suffix trim must stop where the two trims meet.
        let old = lines(&["a", "b", "c", ""]);
        let new = lines(&["a", "b", "c", "d"]);
        assert_eq!(carry(&old, &new), vec![Some(0), Some(1), Some(2), None]);
    }

    #[test]
    fn the_middle_window_reports_whole_file_indices() {
        // The middle is aligned in isolation, so its indices have to be shifted
        // back: `d` is old line 3, not window line 0, and a middle match on
        // `c` is old line 2, not the window's first line.
        let old = lines(&["a", "b", "c", "d", "e"]);
        let new = lines(&["a", "x", "d", "e"]);
        assert_eq!(carry(&old, &new), vec![Some(0), None, Some(3), Some(4)]);
        let new = lines(&["a", "c", "e"]);
        assert_eq!(carry(&old, &new), vec![Some(0), Some(2), Some(4)]);
    }

    proptest! {

        /// The claim in the doc comment: the carry is a common subsequence,
        /// and it is as long as the brute-force LCS — so no edit path can
        /// churn more anchors than this one does.
        #[test]
        fn the_carry_is_maximal(old in prop::collection::vec("[a-e]{1,3}", 0..40),
                                new in prop::collection::vec("[a-e]{1,3}", 0..40)) {
            let old = lines(&old.iter().map(String::as_str).collect::<Vec<_>>());
            let new = lines(&new.iter().map(String::as_str).collect::<Vec<_>>());
            let m = carry(&old, &new);
            let pairs = matched_pairs(&m);
            prop_assert!(pairs.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1),
                "matched indices must rise in both sequences: {m:?}");
            prop_assert!(pairs.iter().all(|(i, j)| old[*i] == new[*j]),
                "a matched pair must hold equal checksums: {m:?}");
            prop_assert_eq!(pairs.len(), lcs_len(&old, &new), "the carry is not maximal");
        }
    }
}
