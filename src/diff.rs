use std::ops::Range;

use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::segment::segment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Equal,
    Changed,
    AddedOrRemoved,
}

/// Output of `assign_roles_and_emphasis`: `(left_roles, right_roles,
/// left_emphasis, right_emphasis)`. Extracted as a named alias so
/// signatures stay under `clippy::type_complexity` without the CI
/// `-D warnings` gate refusing to compile.
pub type RolesAndEmphasis = (
    Vec<Role>,
    Vec<Role>,
    Vec<Vec<Range<usize>>>,
    Vec<Vec<Range<usize>>>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiffToken<'a> {
    /// Segment bytes with a single trailing `\r` stripped (compare-only).
    pub content: &'a [u8],
    /// Mirrors `LineSeg::ends_with_newline`. Two otherwise-identical segments
    /// differ when one is terminated and the other is not — that is how
    /// `"a\nb\n"` vs `"a\nb"` produces a `Changed` on the last pair.
    pub terminated: bool,
}

/// Turn a buffer into a vector of `DiffToken`s using the canonical `segment()`.
///
/// CRLF normalization is compare-only and lives here: if a segment ends with
/// `\r` (which would have been right before a `\n`), that single trailing
/// `\r` is stripped from the token's `content` so `"a\r\n"` and `"a\n"`
/// compare equal token-for-token. Segment counts never change.
pub fn tokenize(s: &str) -> Vec<DiffToken<'_>> {
    let segs = segment(s);
    let bytes = s.as_bytes();
    segs.into_iter()
        .map(|seg| {
            let mut content = seg.range.clone();
            if content.start < content.end && bytes[content.end - 1] == b'\r' {
                content.end -= 1;
            }
            DiffToken {
                content: &bytes[content],
                terminated: seg.ends_with_newline,
            }
        })
        .collect()
}

/// Assign per-line roles for both panes.
///
/// Rules:
/// - `Equal`: both sides' corresponding segments are `Equal`.
/// - Pure `Delete` (left-only): left segments get `AddedOrRemoved`.
/// - Pure `Insert` (right-only): right segments get `AddedOrRemoved`.
/// - `Replace` of lengths `n` (left) and `m` (right): first `min(n, m)`
///   segments on each side are `Changed`; the remainder on the longer side
///   is `AddedOrRemoved`.
/// - Empty-pane rule: diffing anything against the empty pane degenerates to
///   a single `Delete` (or `Insert`), so every line on the non-empty side is
///   `AddedOrRemoved`. No special case needed — the Delete/Insert branches
///   above already produce that result.
///
/// Post-condition: `left_roles.len() == left.len()` and
/// `right_roles.len() == right.len()`. Callers that want the
/// `line_roles.len() == segment(buffer).len()` invariant should pass tokens
/// produced by `tokenize()` over the same buffer.
#[cfg(test)]
fn assign_roles(left: &[DiffToken<'_>], right: &[DiffToken<'_>]) -> (Vec<Role>, Vec<Role>) {
    let (lr, rr, _, _) = assign_roles_and_emphasis(left, right);
    (lr, rr)
}

/// Like `assign_roles` but also returns per-line inline emphasis spans for
/// paired `Changed` lines (operation-oriented: a span on left means bytes
/// that were deleted going from left → right; a span on right means bytes
/// that were inserted). Pure add-only and remove-only lines always get an
/// empty emphasis vector. Equal lines always get an empty emphasis vector.
///
/// Post-condition: `left_emphasis.len() == left.len()` and
/// `right_emphasis.len() == right.len()`.
pub fn assign_roles_and_emphasis(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
) -> RolesAndEmphasis {
    let mut left_roles = vec![Role::Equal; left.len()];
    let mut right_roles = vec![Role::Equal; right.len()];
    let mut left_emphasis: Vec<Vec<Range<usize>>> = vec![Vec::new(); left.len()];
    let mut right_emphasis: Vec<Vec<Range<usize>>> = vec![Vec::new(); right.len()];
    let ops = capture_diff_slices(Algorithm::Myers, left, right);
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    left_roles[old_index + i] = Role::Equal;
                    right_roles[new_index + i] = Role::Equal;
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    left_roles[old_index + i] = Role::AddedOrRemoved;
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    right_roles[new_index + i] = Role::AddedOrRemoved;
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    left_roles[old_index + i] = Role::Changed;
                    right_roles[new_index + i] = Role::Changed;
                    let (lemph, remph) = compute_inline_emphasis(
                        left[old_index + i].content,
                        right[new_index + i].content,
                    );
                    left_emphasis[old_index + i] = lemph;
                    right_emphasis[new_index + i] = remph;
                }
                for i in paired..old_len {
                    left_roles[old_index + i] = Role::AddedOrRemoved;
                }
                for i in paired..new_len {
                    right_roles[new_index + i] = Role::AddedOrRemoved;
                }
            }
        }
    }
    (left_roles, right_roles, left_emphasis, right_emphasis)
}

/// Word tokenization for inline emphasis.
///
/// Splits a byte slice into maximal runs of ASCII-alphanumeric bytes (plus
/// `_`) versus non-alphanumeric bytes. Each run becomes one token. ASCII
/// classification is UTF-8-safe: a multi-byte UTF-8 character's bytes are
/// all classified as non-word and grouped together, so run boundaries
/// never split a UTF-8 code point.
fn word_tokens(s: &[u8]) -> (Vec<Vec<u8>>, Vec<Range<usize>>) {
    if s.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut tokens = Vec::new();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut prev = is_word(s[0]);
    for i in 1..s.len() {
        let cur = is_word(s[i]);
        if cur != prev {
            tokens.push(s[start..i].to_vec());
            ranges.push(start..i);
            start = i;
            prev = cur;
        }
    }
    tokens.push(s[start..].to_vec());
    ranges.push(start..s.len());
    (tokens, ranges)
}

/// Operation-oriented inline emphasis for a paired changed-line pair.
///
/// Reuses `similar` for word-level diffing (no new diff dependency). Each
/// returned span is a contiguous byte range inside its side's line content
/// covering tokens that were deleted (left side) or inserted (right side).
/// Replace ops contribute both sides. Equal tokens contribute nothing.
pub fn compute_inline_emphasis(
    left_content: &[u8],
    right_content: &[u8],
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if left_content == right_content {
        return (Vec::new(), Vec::new());
    }
    let (ltok, lranges) = word_tokens(left_content);
    let (rtok, rranges) = word_tokens(right_content);
    let ops = capture_diff_slices(Algorithm::Myers, &ltok, &rtok);
    let mut left_spans: Vec<Range<usize>> = Vec::new();
    let mut right_spans: Vec<Range<usize>> = Vec::new();
    let push_contiguous = |spans: &mut Vec<Range<usize>>, start: usize, end: usize| {
        if end <= start {
            return;
        }
        if let Some(last) = spans.last_mut()
            && last.end == start
        {
            last.end = end;
            return;
        }
        spans.push(start..end);
    };
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                if old_len > 0 {
                    let start = lranges[old_index].start;
                    let end = lranges[old_index + old_len - 1].end;
                    push_contiguous(&mut left_spans, start, end);
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                if new_len > 0 {
                    let start = rranges[new_index].start;
                    let end = rranges[new_index + new_len - 1].end;
                    push_contiguous(&mut right_spans, start, end);
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                if old_len > 0 {
                    let start = lranges[old_index].start;
                    let end = lranges[old_index + old_len - 1].end;
                    push_contiguous(&mut left_spans, start, end);
                }
                if new_len > 0 {
                    let start = rranges[new_index].start;
                    let end = rranges[new_index + new_len - 1].end;
                    push_contiguous(&mut right_spans, start, end);
                }
            }
        }
    }
    (left_spans, right_spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::segment;

    fn roles(left: &str, right: &str) -> (Vec<Role>, Vec<Role>) {
        let l = tokenize(left);
        let r = tokenize(right);
        let (lr, rr) = assign_roles(&l, &r);
        // Governing invariant: role count aligns with canonical segmentation.
        assert_eq!(lr.len(), segment(left).len());
        assert_eq!(rr.len(), segment(right).len());
        (lr, rr)
    }

    #[test]
    fn tokenize_strips_single_trailing_cr_for_compare_only() {
        let toks = tokenize("a\r\n");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].content, b"a");
        assert!(toks[0].terminated);
    }

    #[test]
    fn tokenize_keeps_stray_cr_inside_segment() {
        let toks = tokenize("a\rb");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].content, b"a\rb");
        assert!(!toks[0].terminated);
    }

    #[test]
    fn tokenize_strips_single_trailing_cr_even_without_newline() {
        let toks = tokenize("abc\r");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].content, b"abc");
        assert!(!toks[0].terminated);
    }

    #[test]
    fn tokenize_records_termination() {
        let toks = tokenize("a\nb");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].content, b"a");
        assert!(toks[0].terminated);
        assert_eq!(toks[1].content, b"b");
        assert!(!toks[1].terminated);
    }

    #[test]
    fn identical_inputs_are_all_equal() {
        let (lr, rr) = roles("a\nb\nc", "a\nb\nc");
        assert_eq!(lr, vec![Role::Equal, Role::Equal, Role::Equal]);
        assert_eq!(rr, vec![Role::Equal, Role::Equal, Role::Equal]);
    }

    #[test]
    fn insert_only_marks_right_added() {
        let (lr, rr) = roles("a\nb", "a\nNEW\nb");
        assert_eq!(lr, vec![Role::Equal, Role::Equal]);
        assert_eq!(rr, vec![Role::Equal, Role::AddedOrRemoved, Role::Equal]);
    }

    #[test]
    fn delete_only_marks_left_removed() {
        let (lr, rr) = roles("a\nGONE\nb", "a\nb");
        assert_eq!(lr, vec![Role::Equal, Role::AddedOrRemoved, Role::Equal]);
        assert_eq!(rr, vec![Role::Equal, Role::Equal]);
    }

    #[test]
    fn paired_replace_marks_changed_on_both_sides() {
        let (lr, rr) = roles("a\nb", "a\nc");
        assert_eq!(lr, vec![Role::Equal, Role::Changed]);
        assert_eq!(rr, vec![Role::Equal, Role::Changed]);
    }

    #[test]
    fn unequal_replace_pairs_minimum_then_marks_remainder_added_or_removed() {
        // Left has 3 different middle lines, right has 2. Pair 2 as Changed,
        // remainder on left is AddedOrRemoved.
        let (lr, rr) = roles("top\nx\ny\nz\nbot", "top\nX\nY\nbot");
        assert_eq!(
            lr,
            vec![
                Role::Equal,
                Role::Changed,
                Role::Changed,
                Role::AddedOrRemoved,
                Role::Equal,
            ]
        );
        assert_eq!(
            rr,
            vec![Role::Equal, Role::Changed, Role::Changed, Role::Equal]
        );
    }

    #[test]
    fn trailing_newline_changed_case() {
        // "a\nb\n" vs "a\nb": first pair Equal; last pair Replace(1,1) →
        // Changed on both sides because `terminated` differs.
        let (lr, rr) = roles("a\nb\n", "a\nb");
        assert_eq!(lr, vec![Role::Equal, Role::Changed]);
        assert_eq!(rr, vec![Role::Equal, Role::Changed]);
    }

    #[test]
    fn trailing_newline_equal_case() {
        let (lr, rr) = roles("a\nb\n", "a\nb\n");
        assert_eq!(lr, vec![Role::Equal, Role::Equal]);
        assert_eq!(rr, vec![Role::Equal, Role::Equal]);
    }

    #[test]
    fn crlf_is_equivalent_to_lf() {
        let (lr, rr) = roles("a\r\nb\r\n", "a\nb\n");
        assert_eq!(lr, vec![Role::Equal, Role::Equal]);
        assert_eq!(rr, vec![Role::Equal, Role::Equal]);
    }

    #[test]
    fn stray_cr_is_not_a_separator_in_diff() {
        // "a\rb" stays a single 3-byte segment, equal to itself on both sides.
        let (lr, rr) = roles("a\rb", "a\rb");
        assert_eq!(lr, vec![Role::Equal]);
        assert_eq!(rr, vec![Role::Equal]);
    }

    #[test]
    fn empty_pane_rule_marks_every_non_empty_line_added_or_removed() {
        let (lr, rr) = roles("a\nb\nc", "");
        assert_eq!(
            lr,
            vec![
                Role::AddedOrRemoved,
                Role::AddedOrRemoved,
                Role::AddedOrRemoved
            ]
        );
        assert!(rr.is_empty());

        let (lr, rr) = roles("", "a\nb\nc");
        assert!(lr.is_empty());
        assert_eq!(
            rr,
            vec![
                Role::AddedOrRemoved,
                Role::AddedOrRemoved,
                Role::AddedOrRemoved
            ]
        );
    }

    #[test]
    fn line_roles_length_equals_segment_length_invariant() {
        // Spot-check a handful of shapes; the `roles` helper already asserts
        // this invariant internally on every call.
        for (l, r) in [
            ("", ""),
            ("only-left", ""),
            ("", "only-right"),
            ("a\nb\nc\n", "a\nb\nc"),
            ("a\r\nb\r\nc", "a\nb\nc"),
            ("x\ny\nz", "X\nY"),
        ] {
            let _ = roles(l, r);
        }
    }

    fn roles_emph(left: &str, right: &str) -> RolesAndEmphasis {
        let l = tokenize(left);
        let r = tokenize(right);
        let (lr, rr, le, re) = assign_roles_and_emphasis(&l, &r);
        // Plan invariant: emphasis vectors stay line-aligned with segments.
        assert_eq!(lr.len(), segment(left).len());
        assert_eq!(rr.len(), segment(right).len());
        assert_eq!(le.len(), lr.len());
        assert_eq!(re.len(), rr.len());
        (lr, rr, le, re)
    }

    #[test]
    fn inline_emphasis_insert_only_on_right_side_for_pure_append() {
        // Insert-only regression guard: left gets no emphasis.
        let (_, _, le, re) = roles_emph("alpha", "alpha changed");
        assert_eq!(le[0], Vec::<Range<usize>>::new());
        // Right emphasis covers the inserted " changed" run (bytes 5..13).
        assert_eq!(re[0], vec![5..13]);
    }

    #[test]
    fn inline_emphasis_delete_only_on_left_side_for_pure_trim() {
        // Symmetric case: `alpha changed` vs `alpha` — delete-only.
        let (_, _, le, re) = roles_emph("alpha changed", "alpha");
        assert_eq!(le[0], vec![5..13]);
        assert_eq!(re[0], Vec::<Range<usize>>::new());
    }

    #[test]
    fn inline_emphasis_replace_emphasizes_both_sides() {
        // `alpha beta` vs `alpha gamma` — `beta` deleted, `gamma` inserted.
        let (_, _, le, re) = roles_emph("alpha beta", "alpha gamma");
        assert_eq!(le[0], vec![6..10]);
        assert_eq!(re[0], vec![6..11]);
    }

    #[test]
    fn inline_emphasis_spike_regression_alxpha_vs_alpha() {
        // Spike-regression from residual risks: `alXpha` vs `alpha` must
        // emphasize left only on the left side (it was the delete-side
        // divergence) and right only on the right side — never the reverse.
        let (lr, rr, le, re) = roles_emph("alXpha", "alpha");
        // Both sides map to Changed because they're a paired Replace(1,1).
        assert_eq!(lr, vec![Role::Changed]);
        assert_eq!(rr, vec![Role::Changed]);
        // The whole word token is Replace, so each side emphasizes its own
        // full word — never the opposite side's bytes.
        assert_eq!(le[0], vec![0..6]);
        assert_eq!(re[0], vec![0..5]);
    }

    #[test]
    fn inline_emphasis_is_empty_for_pure_added_or_removed_lines() {
        // `a\nGONE\nb` vs `a\nb`: the middle line is a pure delete — no
        // pairing with any right-side line, so no inline emphasis.
        let (_, _, le, re) = roles_emph("a\nGONE\nb", "a\nb");
        assert_eq!(le, vec![vec![], vec![], vec![]]);
        assert_eq!(re, vec![vec![], vec![]]);
    }

    #[test]
    fn inline_emphasis_is_empty_for_equal_lines() {
        let (lr, rr, le, re) = roles_emph("a\nb\nc", "a\nb\nc");
        assert_eq!(lr, vec![Role::Equal; 3]);
        assert_eq!(rr, vec![Role::Equal; 3]);
        assert_eq!(le, vec![vec![]; 3]);
        assert_eq!(re, vec![vec![]; 3]);
    }

    #[test]
    fn inline_emphasis_is_empty_when_crlf_diff_forces_changed_but_content_matches() {
        // `a\nb\n` vs `a\nb`: second-line pair is Changed (terminated
        // differs) but token contents match → no word-level differences.
        let (lr, rr, le, re) = roles_emph("a\nb\n", "a\nb");
        assert_eq!(lr, vec![Role::Equal, Role::Changed]);
        assert_eq!(rr, vec![Role::Equal, Role::Changed]);
        assert_eq!(le, vec![vec![], vec![]]);
        assert_eq!(re, vec![vec![], vec![]]);
    }

    #[test]
    fn inline_emphasis_vectors_length_matches_segment_length_invariant() {
        // Governing invariant: `inline_emphasis.len() == segment(buffer).len()`.
        for (l, r) in [
            ("", ""),
            ("only-left", ""),
            ("", "only-right"),
            ("a\nb\nc\n", "a\nb\nc"),
            ("a\r\nb\r\nc", "a\nb\nc"),
            ("x\ny\nz", "X\nY"),
            ("alpha beta", "alpha gamma"),
        ] {
            let _ = roles_emph(l, r);
        }
    }

    #[test]
    fn compute_inline_emphasis_handles_multibyte_utf8_safely() {
        // Multi-byte UTF-8 characters must never get split. Here "café"
        // differs from "cafe" on the final character. UTF-8-safe token
        // runs should keep the "é" (0xC3 0xA9) together as a non-word
        // token, giving a byte range that is a valid UTF-8 slice.
        let (le, re) = compute_inline_emphasis("café".as_bytes(), "cafe".as_bytes());
        // Assert each range, on its side, indexes a valid UTF-8 slice.
        for r in &le {
            assert!(std::str::from_utf8(&"café".as_bytes()[r.clone()]).is_ok());
        }
        for r in &re {
            assert!(std::str::from_utf8(&"cafe".as_bytes()[r.clone()]).is_ok());
        }
        // And at least one side has a non-empty span (the characters differ).
        assert!(!le.is_empty() || !re.is_empty());
    }

    // Dedicated `tokenize` boundary tests.

    #[test]
    fn tokenize_empty_input_produces_zero_tokens() {
        let toks = tokenize("");
        assert!(toks.is_empty());
    }

    #[test]
    fn tokenize_single_unterminated_line() {
        let toks = tokenize("abc");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].content, b"abc");
        assert!(!toks[0].terminated);
    }

    #[test]
    fn tokenize_multi_line_input_splits_on_each_newline() {
        let toks = tokenize("a\nb\nc\n");
        assert_eq!(toks.len(), 3);
        for t in &toks {
            assert!(t.terminated, "every segment ends with newline");
        }
        assert_eq!(toks[0].content, b"a");
        assert_eq!(toks[1].content, b"b");
        assert_eq!(toks[2].content, b"c");
    }

    #[test]
    fn tokenize_multi_line_crlf_strips_trailing_cr_on_every_line() {
        // Every `\r\n`-terminated line must have its trailing `\r`
        // stripped from `content`, so CRLF and LF token pairs are
        // compare-equal.
        let crlf = tokenize("alpha\r\nbeta\r\ngamma\r\n");
        let lf = tokenize("alpha\nbeta\ngamma\n");
        assert_eq!(crlf.len(), 3);
        assert_eq!(crlf.len(), lf.len());
        for (c, l) in crlf.iter().zip(lf.iter()) {
            assert_eq!(c.content, l.content);
            assert_eq!(c.terminated, l.terminated);
        }
    }

    #[test]
    fn tokenize_utf8_multibyte_boundary_content_is_valid_utf8() {
        // Two segments containing non-ASCII code points must come back
        // with `content` byte slices that are still valid UTF-8 — i.e.
        // the segmenter never split a code point across segments.
        let toks = tokenize("café\nnaïve");
        assert_eq!(toks.len(), 2);
        assert!(std::str::from_utf8(toks[0].content).is_ok());
        assert!(std::str::from_utf8(toks[1].content).is_ok());
        assert_eq!(std::str::from_utf8(toks[0].content).unwrap(), "café");
        assert_eq!(std::str::from_utf8(toks[1].content).unwrap(), "naïve");
    }

    // Direct `compute_inline_emphasis` envelope tests.

    #[test]
    fn compute_inline_emphasis_identical_sequences_have_no_emphasis() {
        let (le, re) = compute_inline_emphasis(b"hello world", b"hello world");
        assert!(le.is_empty());
        assert!(re.is_empty());
    }

    #[test]
    fn compute_inline_emphasis_empty_pair_has_no_emphasis() {
        let (le, re) = compute_inline_emphasis(b"", b"");
        assert!(le.is_empty());
        assert!(re.is_empty());
    }

    fn spans_cover_entire_side(spans: &[Range<usize>], len: usize) -> bool {
        if spans.is_empty() {
            return len == 0;
        }
        let mut cursor = 0usize;
        for r in spans {
            if r.start != cursor {
                return false;
            }
            cursor = r.end;
        }
        cursor == len
    }

    #[test]
    fn compute_inline_emphasis_fully_disjoint_emphasizes_entire_sides() {
        // Single word tokens that share no bytes with each other must
        // produce one span per side covering the full byte range
        // (maximum emphasis envelope).
        let left = b"alpha";
        let right = b"omega";
        let (le, re) = compute_inline_emphasis(left, right);
        assert!(spans_cover_entire_side(&le, left.len()));
        assert!(spans_cover_entire_side(&re, right.len()));
    }

    #[test]
    fn compute_inline_emphasis_single_character_token_boundaries() {
        // Tokens: `a` (word) | ` ` (non-word) | `b` (word) vs
        //         `a` (word) | ` ` (non-word) | `c` (word).
        // Only the single-character word at the end diverges → exactly
        // one 1-byte span on each side, at byte offset 2.
        let (le, re) = compute_inline_emphasis(b"a b", b"a c");
        assert_eq!(le, vec![2..3]);
        assert_eq!(re, vec![2..3]);
    }

    // Performance regression guards. Gated behind both `bench`
    // (to stay out of the default test set) and `not(debug_assertions)`
    // (timing assertions are only meaningful under `--release`). Run
    // with `cargo test --features bench --release`.

    #[cfg(all(feature = "bench", not(debug_assertions)))]
    #[test]
    fn perf_assign_roles_2k_lines_under_budget() {
        use std::time::Instant;
        // Budget source: the `edit_latency` E2E gate targets a 42 ms
        // median on the same 2k-line fixtures. Using the
        // same fixtures here keeps this guard coherent with the E2E
        // signal; `assign_roles_and_emphasis` dominates that pipeline.
        const BUDGET_MS: u128 = 30;
        let a = std::fs::read_to_string("bench/2k_lines_a.txt")
            .expect("bench/2k_lines_a.txt must exist (run scripts/gen_fixtures.rs or cargo run --example gen_fixtures)");
        let b = std::fs::read_to_string("bench/2k_lines_b.txt")
            .expect("bench/2k_lines_b.txt must exist (run scripts/gen_fixtures.rs or cargo run --example gen_fixtures)");
        let la = tokenize(&a);
        let rb = tokenize(&b);
        let start = Instant::now();
        let (lr, rr, _, _) = assign_roles_and_emphasis(&la, &rb);
        let elapsed = start.elapsed();
        assert_eq!(lr.len(), la.len());
        assert_eq!(rr.len(), rb.len());
        assert!(
            elapsed.as_millis() <= BUDGET_MS,
            "assign_roles_and_emphasis(2k) took {:?}, budget {} ms",
            elapsed,
            BUDGET_MS
        );
    }

    #[cfg(all(feature = "bench", not(debug_assertions)))]
    #[test]
    fn perf_compute_inline_emphasis_2k_lines_under_budget() {
        use std::time::Instant;
        // Budget source: word-level emphasis is the hot-loop inside
        // `assign_roles_and_emphasis` for paired Changed lines. Splitting
        // the measurement isolates it from the outer Myers pass so the
        // budget can tighten without bumping the 30 ms aggregate gate.
        // 10 ms reflects observed per-call cost × paired-line count on
        // the reference workstation.
        const BUDGET_MS: u128 = 10;
        let a = std::fs::read_to_string("bench/2k_lines_a.txt").expect("2k fixture a");
        let b = std::fs::read_to_string("bench/2k_lines_b.txt").expect("2k fixture b");
        let la = tokenize(&a);
        let rb = tokenize(&b);
        let (lr, _, _, _) = assign_roles_and_emphasis(&la, &rb);
        // Gather every paired Changed line pair by replaying Myers: we
        // re-diff and tap into the Replace branches directly so the
        // measurement covers only `compute_inline_emphasis` calls.
        let ops = capture_diff_slices(Algorithm::Myers, &la, &rb);
        let mut pairs: Vec<(&[u8], &[u8])> = Vec::new();
        for op in ops {
            if let DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } = op
            {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    pairs.push((la[old_index + i].content, rb[new_index + i].content));
                }
            }
        }
        assert!(
            !pairs.is_empty(),
            "2k fixtures must exhibit at least one paired Changed line"
        );
        // Sanity: every pair would also have populated `lr` as Changed.
        let changed_count = lr.iter().filter(|&&r| r == Role::Changed).count();
        assert!(changed_count >= pairs.len());

        let start = Instant::now();
        for (l, r) in &pairs {
            let _ = compute_inline_emphasis(l, r);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() <= BUDGET_MS,
            "compute_inline_emphasis over {} pairs took {:?}, budget {} ms",
            pairs.len(),
            elapsed,
            BUDGET_MS
        );
    }
}
