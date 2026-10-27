use super::*;

use proptest::prelude::*;
use std::collections::HashSet;

proptest! {
    /// Uniqueness is allocation, not the hash function: even when every
    /// line has the same canon, no two anchors may collide (spec §5.4).
    #[test]
    fn every_line_gets_a_unique_anchor(lines in prop::collection::vec("abc", 0..40)) {
        let content = lines.join("\n");
        let hashes = line_hashes(&content).unwrap();
        assert_eq!(hashes.len(), split_lines(&content).len());
        let mut seen = HashSet::new();
        for h in &hashes {
            assert!(seen.insert(h.clone()), "duplicate anchor {h}");
        }
    }

    /// The anchor is a function of canon + preceding allocation state:
    /// whitespace padding changes nothing the allocator sees, so the
    /// anchor set is identical (determinism, canon invariance).
    #[test]
    fn anchors_follow_the_canon_not_the_whitespace(lines in prop::collection::vec("abc", 1..20)) {
        let base: String = lines.join("\n");
        let padded: String = lines
            .iter()
            .map(|l| format!("  {l}\t"))
            .collect::<Vec<_>>()
            .join("\n");
        prop_assert_eq!(line_hashes(&base).unwrap(), line_hashes(&padded).unwrap());
    }

    /// Survivor reuse: an edit of one line leaves every other line's
    /// anchor in place (a single-line replacement shifts nothing).
    #[test]
    fn survivors_keep_their_anchor_across_an_edit(
        (lines, at) in (prop::collection::vec("abcd", 5..15), 0usize..15),
    ) {
        let replacement = String::from("wxyz");
        let content = lines.join("\n");
        let at = at.min(lines.len() - 1);
        let hashes = line_hashes(&content).unwrap();
        let edit = Edit {
            from: hashes[at].clone(),
            to: hashes[at].clone(),
            content: replacement.clone(),
        };
        let edited = apply_edit(&content, &edit).unwrap();
        for (i, line) in lines.iter().enumerate() {
            if i == at {
                continue;
            }
            assert_eq!(edited.hashes[i], hashes[i], "line {i} ({line}) lost its anchor");
        }
    }

    /// A fresh 4-letter draw that misses the current set is stale: rejected,
    /// never fuzzy-matched.
    #[test]
    fn an_anchor_not_in_the_file_is_stale_not_fuzzy_matched(
        (lines, mut seed) in (prop::collection::vec("abc", 1..10), 0usize..HASH_SPACE),
    ) {
        let content = lines.join("\n");
        let hashes = line_hashes(&content).unwrap();
        let mut candidate = idx_to_hash(seed);
        while hashes.contains(&candidate) {
            seed = (seed + STRIDE) % HASH_SPACE;
            candidate = idx_to_hash(seed);
        }
        let edit = Edit {
            from: candidate.clone(),
            to: candidate,
            content: "x".into(),
        };
        assert!(matches!(
            apply_edit(&content, &edit),
            Err(EditError::Stale { .. })
        ));
    }

    /// A malformed anchor (wrong length or an out-of-alphabet char) can
    /// never resolve to an application.
    #[test]
    fn malformed_anchors_never_apply(
        (len, chars) in (0usize..5, prop::collection::vec("Z0~|@", 0..6)),
    ) {
        let mut anchor: String = chars.iter().take(len).cloned().collect();
        anchor.push('~');
        let edit = Edit {
            from: anchor.clone(),
            to: anchor,
            content: "x".into(),
        };
        assert!(apply_edit("one\ntwo\n", &edit).is_err());
    }

    /// render → apply_edit → content: the edited file is exactly the
    /// expected line splice, and it re-anchors to a fresh unique set.
    #[test]
    fn render_then_edit_reproduces_the_expected_content(
        (lines, a, b) in (prop::collection::vec("abc", 1..12), 0usize..12, 0usize..12),
    ) {
        let replacement = String::from("xyz");
        let content = lines.join("\n");
        let hashes = line_hashes(&content).unwrap();
        let from = a.min(lines.len() - 1);
        let to = b.min(lines.len() - 1).max(from);
        let edit = Edit {
            from: hashes[from].clone(),
            to: hashes[to].clone(),
            content: replacement.clone(),
        };
        let edited = apply_edit(&content, &edit).unwrap();
        let mut expected: Vec<String> = lines[..from].to_vec();
        expected.push(replacement.clone());
        expected.extend(lines[to + 1..].iter().cloned());
        assert_eq!(edited.content, expected.join("\n"));
        let re = line_hashes(&edited.content).unwrap();
        let mut seen = HashSet::new();
        for h in &re {
            assert!(seen.insert(h.clone()), "duplicate anchor {h}");
        }
    }
}

proptest! {
    // 1000 cases: a two-symbol alphabet over short lines is chosen so the
    // script produces repeated lines in swapped order — the shape where an
    // in-order walk and a maximal match part company.
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// The §7.3 bar: a read → edit → re-read re-labels no more lines than the
    /// maximal in-order match allows. It is measured in the anchors a caller
    /// actually receives, against the full dynamic program, because before
    /// #298 the app-write path and the registry path ran different algorithms
    /// and the same edit re-labelled a different number of lines depending on
    /// which path produced the read.
    #[test]
    fn a_re_read_relabels_no_more_lines_than_the_maximal_match(
        (lines, from, to, replacement) in (
            prop::collection::vec("[ab]{1,2}", 1..12),
            0usize..12,
            0usize..12,
            prop::collection::vec("[ab]{1,2}", 0..4),
        ),
    ) {
        let content = lines.join("\n");
        let hashes = line_hashes(&content).unwrap();
        let from = from.min(lines.len() - 1);
        let to = to.min(lines.len() - 1).max(from);
        let edit = Edit {
            from: hashes[from].clone(),
            to: hashes[to].clone(),
            content: replacement.join("\n"),
        };
        let mut reg = registry::Registry::new();
        let (before, _) = reg.allocate("f", &content).expect("first read");
        let Ok(edited) = apply_edit(&content, &edit) else {
            return Ok(());
        };
        let (after, _) = reg.allocate("f", &edited.content).expect("re-read");
        let fresh = after.iter().filter(|a| !before.contains(*a)).count();
        let maximal = fresh_lines_by_full_table(&content, &edited.content);
        prop_assert!(
            fresh <= maximal,
            "{fresh} lines re-labelled; the maximal match leaves {maximal}"
        );
    }
}

/// How many lines of `after` a maximal in-order checksum match leaves fresh,
/// computed by the full dynamic program — the independent oracle for the
/// property above.
fn fresh_lines_by_full_table(before: &str, after: &str) -> usize {
    let sums = |content: &str| -> Vec<String> {
        split_lines(content)
            .iter()
            .map(|l| line_checksum(l))
            .collect()
    };
    let (old, new) = (sums(before), sums(after));
    let mut table = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    new.len() - table[0][0]
}
