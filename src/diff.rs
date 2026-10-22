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

/// Intern complete line tokens for cheap comparisons inside the exact diff.
/// Hash collisions still compare the original bytes and termination flag.
/// The map borrows the current job's text and is discarded after this call.
fn capture_line_diff(left: &[DiffToken<'_>], right: &[DiffToken<'_>]) -> Vec<DiffOp> {
    let mut ids = ahash::AHashMap::with_capacity(left.len() + right.len());
    let mut next_id = 0usize;
    let all_ids: Vec<_> = left
        .iter()
        .chain(right.iter())
        .map(|token| {
            *ids.entry(*token).or_insert_with(|| {
                let id = next_id;
                next_id += 1;
                id
            })
        })
        .collect();
    let (left_ids, right_ids) = all_ids.split_at(left.len());
    // Similar 3.2 renamed the exact Myers behavior to RawMyers; retain
    // the same shortest-edit-script semantics across dependency upgrades.
    capture_diff_slices(Algorithm::RawMyers, left_ids, right_ids)
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
    let ops = capture_line_diff(left, right);
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
                    let li = old_index + i;
                    let ri = new_index + i;
                    let diff = paired_line_diff(left[li].content, right[ri].content);
                    if diff.is_unrelated_pairing() {
                        // The line-level diff paired two unrelated lines (a
                        // blank-line anchor spanning a paragraph boundary is
                        // the classic cause). Painting a word-level `Changed`
                        // here would scatter nonsense emphasis, so fall back
                        // to a plain removal + addition with no emphasis.
                        left_roles[li] = Role::AddedOrRemoved;
                        right_roles[ri] = Role::AddedOrRemoved;
                    } else {
                        left_roles[li] = Role::Changed;
                        right_roles[ri] = Role::Changed;
                        left_emphasis[li] = diff.left_spans;
                        right_emphasis[ri] = diff.right_spans;
                    }
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
/// `_`) versus non-alphanumeric bytes. Each run becomes one borrowed token. ASCII
/// classification is UTF-8-safe: a multi-byte UTF-8 character's bytes are
/// all classified as non-word and grouped together, so run boundaries
/// never split a UTF-8 code point.
fn word_tokens(s: &[u8]) -> (Vec<&[u8]>, Vec<Range<usize>>) {
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
            tokens.push(&s[start..i]);
            ranges.push(start..i);
            start = i;
            prev = cur;
        }
    }
    tokens.push(&s[start..]);
    ranges.push(start..s.len());
    (tokens, ranges)
}

/// Minimum alphanumeric word-token count required on *both* lines before
/// the similarity guard may demote a paired `Replace` line from `Changed`
/// to `AddedOrRemoved`. Below this, word-level similarity is unreliable
/// (a one-word typo like `alXpha` vs `alpha` shares no whole-word tokens
/// yet is a genuine close edit) and short lines cannot produce misleading
/// "confetti" emphasis anyway. Empirically, the mispaired unrelated
/// paragraphs this guard targets carry 26+ words per side.
const MIN_WORDS_FOR_SIMILARITY_GUARD: usize = 12;

/// A paired changed-line pair whose shared-content ratio is below this is
/// treated as *unrelated* — the line-level diff paired two different lines
/// only because it had no better anchor (classically an extra blank line
/// matched across a paragraph boundary). Measured separation on real
/// inputs: unrelated pairs land near 0.13–0.20, genuine edits near 0.92+,
/// so 0.4 sits well clear of both populations.
const MIN_SIMILARITY_FOR_CHANGED: f32 = 0.4;

/// Word diff of a paired changed-line pair: the inline emphasis spans plus
/// the signals used to detect a mispairing of unrelated lines.
struct PairedLineDiff {
    /// Contiguous byte ranges inside the left line covering deleted tokens.
    left_spans: Vec<Range<usize>>,
    /// Contiguous byte ranges inside the right line covering inserted tokens.
    right_spans: Vec<Range<usize>>,
    /// Count of alphanumeric ("real word") tokens on each side.
    left_words: usize,
    right_words: usize,
    /// Shared-content ratio in `0.0..=1.0`, `2 * matched / (llen + rlen)`
    /// where `matched` is the total byte length of word-tokens that compare
    /// equal. 1.0 means byte-identical content.
    similarity: f32,
}

impl PairedLineDiff {
    /// True when both lines are substantial yet share almost no content, so
    /// pairing them as a word-level `Changed` would emit nonsense emphasis.
    /// Such pairs are painted as plain removal + addition instead.
    fn is_unrelated_pairing(&self) -> bool {
        self.left_words >= MIN_WORDS_FOR_SIMILARITY_GUARD
            && self.right_words >= MIN_WORDS_FOR_SIMILARITY_GUARD
            && self.similarity < MIN_SIMILARITY_FOR_CHANGED
    }
}

fn is_word_token(t: &[u8]) -> bool {
    t.iter().any(u8::is_ascii_alphanumeric)
}

/// Operation-oriented word diff for a paired changed-line pair.
///
/// Reuses `similar` for word-level diffing (no new diff dependency). Each
/// emphasis span is a contiguous byte range inside its side's line content
/// covering tokens that were deleted (left side) or inserted (right side);
/// Replace ops contribute both sides, Equal tokens contribute nothing. The
/// returned `similarity` and per-side word counts let the caller reject a
/// pairing of two unrelated lines (see `is_unrelated_pairing`).
fn paired_line_diff(left_content: &[u8], right_content: &[u8]) -> PairedLineDiff {
    if left_content == right_content {
        // Identical content (e.g. a pair that is `Changed` only because the
        // trailing-newline / CRLF state differs): no emphasis, maximally
        // similar. Word counts are irrelevant here because a similarity of
        // 1.0 already precludes demotion, so we skip tokenizing.
        return PairedLineDiff {
            left_spans: Vec::new(),
            right_spans: Vec::new(),
            left_words: 0,
            right_words: 0,
            similarity: 1.0,
        };
    }
    let (ltok, lranges) = word_tokens(left_content);
    let (rtok, rranges) = word_tokens(right_content);
    let left_words = ltok.iter().filter(|t| is_word_token(t)).count();
    let right_words = rtok.iter().filter(|t| is_word_token(t)).count();
    let ops = capture_diff_slices(Algorithm::RawMyers, &ltok, &rtok);
    let mut left_spans: Vec<Range<usize>> = Vec::new();
    let mut right_spans: Vec<Range<usize>> = Vec::new();
    let mut matched_bytes = 0usize;
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
            DiffOp::Equal { old_index, len, .. } => {
                for tok in &ltok[old_index..old_index + len] {
                    matched_bytes += tok.len();
                }
            }
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
    let denom = left_content.len() + right_content.len();
    let similarity = if denom == 0 {
        1.0
    } else {
        2.0 * matched_bytes as f32 / denom as f32
    };
    PairedLineDiff {
        left_spans,
        right_spans,
        left_words,
        right_words,
        similarity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::segment;

    /// Test shim preserving the historical `compute_inline_emphasis`
    /// contract (emphasis spans only) on top of `paired_line_diff`.
    fn compute_inline_emphasis(
        left_content: &[u8],
        right_content: &[u8],
    ) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
        let diff = paired_line_diff(left_content, right_content);
        (diff.left_spans, diff.right_spans)
    }

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
    fn interned_line_diff_matches_direct_raw_myers() {
        fn next(seed: &mut u64) -> usize {
            *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (*seed >> 32) as usize
        }

        let lines = [
            "alpha",
            "beta",
            "alpha",
            "",
            "é猫",
            "e\u{301}",
            "a_b c+d",
            "\r",
            "mid\rcarriage",
        ];
        let mut seed = 234_567u64;
        for _ in 0..2_048 {
            let mut buffers = [String::new(), String::new()];
            for buffer in &mut buffers {
                let count = next(&mut seed) % 40;
                for index in 0..count {
                    buffer.push_str(lines[next(&mut seed) % lines.len()]);
                    if index + 1 < count || next(&mut seed).is_multiple_of(2) {
                        buffer.push_str(if next(&mut seed).is_multiple_of(3) {
                            "\r\n"
                        } else {
                            "\n"
                        });
                    }
                }
            }
            let left = tokenize(&buffers[0]);
            let right = tokenize(&buffers[1]);
            assert_eq!(
                capture_line_diff(&left, &right),
                capture_diff_slices(Algorithm::RawMyers, &left, &right),
                "interning changed operations for {buffers:?}"
            );
        }
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
    fn expensive_repetitive_diff_preserves_exact_equal_lines() {
        // Similar 3.2's heuristic Myers path keeps only 141 equal lines in
        // this fixture. Preserve the previous shortest edit script (144)
        // so a dependency update does not add spurious changed lines.
        let mut state = 15_690_471_396_791_370_040_u64;
        let mut buffer = || {
            (0..512)
                .map(|_| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    format!("{}\n", (state >> 32) % 32)
                })
                .collect::<String>()
        };
        let left = buffer();
        let right = buffer();
        let (left_roles, right_roles) = roles(&left, &right);
        for pane_roles in [left_roles, right_roles] {
            assert_eq!(
                pane_roles.iter().filter(|&&r| r == Role::Equal).count(),
                144
            );
        }
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
    fn blank_line_anchor_mispairing_produces_no_stray_emphasis() {
        // Regression: when one pane has an extra blank line, the line-level
        // Myers diff anchors `Equal` on the blank and cross-pairs two
        // unrelated paragraphs as a `Replace`. Word-diffing those unrelated
        // lines yields scattered nonsense emphasis. The similarity guard must
        // demote such a pair to `AddedOrRemoved` (line-level only), so no
        // inline emphasis is emitted.
        let pa =
            "The quarterly revenue report shows steady growth across every regional market segment";
        let pb = "My favorite weeknight recipe combines fresh basil ripe tomatoes minced garlic and olive oil";
        let pa2 =
            "The quarterly revenue report shows steady growth across each regional market segment";
        let pb2 = "My favorite weeknight recipe blends fresh basil ripe tomatoes minced garlic and olive oil";
        let left = format!("{pa}\n\n{pb}");
        // Leading blank line on the right is the anchor that mispairs the
        // paragraphs (left para 2 with right para 1).
        let right = format!("\n{pa2}\n\n{pb2}");
        let (_, _, le, re) = roles_emph(&left, &right);
        assert!(
            le.iter().all(|v| v.is_empty()),
            "left carried stray emphasis from an unrelated pairing: {le:?}"
        );
        assert!(
            re.iter().all(|v| v.is_empty()),
            "right carried stray emphasis from an unrelated pairing: {re:?}"
        );
    }

    #[test]
    fn long_line_small_edit_still_emphasized_guard_does_not_overfire() {
        // Guard against over-firing: a long line (>= the word-count gate)
        // with a single-word edit is a genuine `Changed` pair and must keep
        // its word-level emphasis. Only truly dissimilar long pairs demote.
        let left = "the quick brown fox jumps over the lazy dog every single morning without fail";
        let right = "the quick brown fox leaps over the lazy dog every single morning without fail";
        let (lr, rr, le, re) = roles_emph(left, right);
        assert_eq!(lr, vec![Role::Changed]);
        assert_eq!(rr, vec![Role::Changed]);
        assert!(!le[0].is_empty(), "left emphasis should be present: {le:?}");
        assert!(
            !re[0].is_empty(),
            "right emphasis should be present: {re:?}"
        );
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
        let ops = capture_diff_slices(Algorithm::RawMyers, &la, &rb);
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
