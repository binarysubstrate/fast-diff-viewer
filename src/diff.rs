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
/// - Exact nonblank matches anchor corresponding `Equal` segments.
/// - Pure `Delete` (left-only): left segments get `AddedOrRemoved`.
/// - Pure `Insert` (right-only): right segments get `AddedOrRemoved`.
/// - Changed regions spanning blank anchors use bounded content alignment,
///   including single-line splits/joins. Plausible pairs become `Changed`
///   with word emphasis; unrelated substantial pairs remain line-only.
/// - Unaligned residuals use positional pairing while skipping unmatched
///   blank lines. Excess lines stay `AddedOrRemoved`.
/// - Empty-pane rule: diffing anything against the empty pane degenerates to
///   a single `Delete` (or `Insert`), so every line on the non-empty side is
///   `AddedOrRemoved`.
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
    let mut output = (
        vec![Role::AddedOrRemoved; left.len()],
        vec![Role::AddedOrRemoved; right.len()],
        vec![Vec::new(); left.len()],
        vec![Vec::new(); right.len()],
    );
    let mut remaining_work = MAX_ALIGNMENT_WORK;
    let mut remaining_lookahead_bytes = left
        .iter()
        .chain(right)
        .fold(0usize, |total, line| {
            total.saturating_add(line.content.len().saturating_add(1))
        })
        .saturating_mul(4);
    let mut left_start = 0;
    let mut right_start = 0;
    for op in capture_line_diff(left, right) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..len {
                let li = old_index + offset;
                let ri = new_index + offset;
                // A blank separator is too weak an anchor to decide which
                // edited paragraphs correspond. Keep it inside the region.
                if is_blank(left[li]) {
                    continue;
                }
                align_region(
                    left,
                    right,
                    left_start..li,
                    right_start..ri,
                    &mut output,
                    &mut remaining_work,
                    &mut remaining_lookahead_bytes,
                );
                output.0[li] = Role::Equal;
                output.1[ri] = Role::Equal;
                left_start = li + 1;
                right_start = ri + 1;
            }
        }
    }
    align_region(
        left,
        right,
        left_start..left.len(),
        right_start..right.len(),
        &mut output,
        &mut remaining_work,
        &mut remaining_lookahead_bytes,
    );
    output
}

fn is_blank(token: DiffToken<'_>) -> bool {
    token.content.iter().all(u8::is_ascii_whitespace)
}

// Refinement runs only between exact nonblank anchors. Line/byte caps apply
// per region; the candidate word-comparison budget is shared by the whole job.
const MAX_ALIGNMENT_LINES: usize = 64;
const MAX_ALIGNMENT_BYTES: usize = 16 * 1024;
const MAX_ALIGNMENT_WORK: usize = 262_144;

#[derive(Clone, Copy, Default)]
enum AlignmentStep {
    #[default]
    SkipLeft,
    SkipRight,
    Pair(usize, usize),
}

struct BlockPair {
    left: Range<usize>,
    right: Range<usize>,
}

/// Concatenation is comparison-only; ranges map spans back to original lines.
fn block_text(lines: &[DiffToken<'_>]) -> (Vec<u8>, Vec<Range<usize>>) {
    let mut text = Vec::new();
    let mut ranges = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            // A hard wrap stands for a word separator during comparison.
            // Using a newline here would attach it to adjacent non-ASCII
            // runs and falsely emphasize an unchanged accent or CJK text.
            text.push(b' ');
        }
        let start = text.len();
        text.extend_from_slice(line.content);
        ranges.push(start..text.len());
    }
    (text, ranges)
}

fn block_diff(left: &[DiffToken<'_>], right: &[DiffToken<'_>]) -> PairedLineDiff {
    if left.len() == 1 && right.len() == 1 {
        paired_line_diff(left[0].content, right[0].content)
    } else {
        paired_line_diff(&block_text(left).0, &block_text(right).0)
    }
}

fn candidate_allowed(left: &[DiffToken<'_>], right: &[DiffToken<'_>]) -> bool {
    // Splits/joins never swallow a blank paragraph separator. Blank lines
    // are paired separately after meaningful content establishes alignment.
    left.iter().chain(right).all(|line| !is_blank(*line))
}

fn refined_pairs(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
    remaining_work: &mut usize,
) -> Option<Vec<BlockPair>> {
    if left.len() > MAX_ALIGNMENT_LINES
        || right.len() > MAX_ALIGNMENT_LINES
        || left
            .iter()
            .chain(right)
            .map(|line| line.content.len())
            .sum::<usize>()
            > MAX_ALIGNMENT_BYTES
    {
        return None;
    }
    let left_words: Vec<_> = left
        .iter()
        .map(|line| word_tokens(line.content).0.len())
        .collect();
    let right_words: Vec<_> = right
        .iter()
        .map(|line| word_tokens(line.content).0.len())
        .collect();
    let mut work = 0usize;
    // Charge before running any candidate diff, so oversized regions take
    // the fallback without paying for a partially completed search.
    for i in 1..=left.len() {
        for j in 1..=right.len() {
            for (ln, rn) in [(1, 1), (1, 2), (2, 1)] {
                if i < ln || j < rn || !candidate_allowed(&left[i - ln..i], &right[j - rn..j]) {
                    continue;
                }
                let tokens = left_words[i - ln..i].iter().sum::<usize>()
                    + right_words[j - rn..j].iter().sum::<usize>()
                    + ln
                    + rn;
                work = work.saturating_add(tokens.saturating_mul(tokens));
                if work > *remaining_work {
                    return None;
                }
            }
        }
    }
    *remaining_work -= work;
    let width = right.len() + 1;
    let mut scores = vec![0i64; (left.len() + 1) * width];
    let mut steps = vec![AlignmentStep::SkipLeft; scores.len()];
    for step in &mut steps[1..width] {
        *step = AlignmentStep::SkipRight;
    }
    for i in 1..=left.len() {
        for j in 1..=right.len() {
            let index = i * width + j;
            scores[index] = scores[index - width];
            if scores[index - 1] > scores[index] {
                scores[index] = scores[index - 1];
                steps[index] = AlignmentStep::SkipRight;
            }
            for (ln, rn) in [(1, 1), (1, 2), (2, 1)] {
                if i < ln || j < rn || !candidate_allowed(&left[i - ln..i], &right[j - rn..j]) {
                    continue;
                }
                let diff = block_diff(&left[i - ln..i], &right[j - rn..j]);
                if diff.alignment_score <= 0 || diff.is_unrelated_pairing() {
                    continue;
                }
                let score = scores[(i - ln) * width + j - rn] + diff.alignment_score;
                // Strict comparison gives deterministic earliest matches;
                // equal-score gaps cannot displace an existing match.
                if score > scores[index] {
                    scores[index] = score;
                    steps[index] = AlignmentStep::Pair(ln, rn);
                }
            }
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (left.len(), right.len());
    while i > 0 || j > 0 {
        match steps[i * width + j] {
            AlignmentStep::SkipLeft => i -= 1,
            AlignmentStep::SkipRight => j -= 1,
            AlignmentStep::Pair(ln, rn) => {
                pairs.push(BlockPair {
                    left: i - ln..i,
                    right: j - rn..j,
                });
                i -= ln;
                j -= rn;
            }
        }
    }
    pairs.reverse();
    Some(pairs)
}

fn project_spans(spans: &[Range<usize>], lines: &[Range<usize>]) -> Vec<Vec<Range<usize>>> {
    lines
        .iter()
        .map(|line| {
            spans
                .iter()
                .filter_map(|span| {
                    let start = span.start.max(line.start);
                    let end = span.end.min(line.end);
                    (start < end)
                        .then_some(start.saturating_sub(line.start)..end.saturating_sub(line.start))
                })
                .collect()
        })
        .collect()
}

fn apply_pair(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
    lr: Range<usize>,
    rr: Range<usize>,
    output: &mut RolesAndEmphasis,
) {
    if lr.len() == 1 && rr.len() == 1 && left[lr.start] == right[rr.start] {
        output.0[lr.start] = Role::Equal;
        output.1[rr.start] = Role::Equal;
        return;
    }
    let diff = block_diff(&left[lr.clone()], &right[rr.clone()]);
    if diff.is_unrelated_pairing() {
        return;
    }
    output.0[lr.clone()].fill(Role::Changed);
    output.1[rr.clone()].fill(Role::Changed);
    if lr.len() == 1 && rr.len() == 1 {
        output.2[lr.start] = diff.left_spans;
        output.3[rr.start] = diff.right_spans;
    } else {
        for (index, spans) in project_spans(&diff.left_spans, &block_text(&left[lr.clone()]).1)
            .into_iter()
            .enumerate()
        {
            output.2[lr.start + index] = spans;
        }
        for (index, spans) in project_spans(&diff.right_spans, &block_text(&right[rr.clone()]).1)
            .into_iter()
            .enumerate()
        {
            output.3[rr.start + index] = spans;
        }
    }
}

/// Cheap, collision-safe content overlap for the bounded fallback. This
/// chooses a local shape only; the chosen pair still uses the exact word diff.
fn content_overlap_score(left: &[u8], right: &[u8]) -> i64 {
    let (left_tokens, _) = word_tokens(left);
    let mut counts = ahash::AHashMap::<&[u8], usize>::with_capacity(left_tokens.len());
    for token in left_tokens {
        if meaningful_bytes(token) > 0 {
            *counts.entry(token).or_default() += 1;
        }
    }
    let (right_tokens, _) = word_tokens(right);
    let mut matched = 0usize;
    for token in right_tokens {
        if let Some(count) = counts.get_mut(token)
            && *count > 0
        {
            *count -= 1;
            matched += meaningful_bytes(token);
        }
    }
    10 * matched as i64 - 2 * (meaningful_bytes(left) + meaningful_bytes(right)) as i64
}

fn fallback_shape(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
    remaining_bytes: &mut usize,
) -> (usize, usize) {
    if left.len() == 1 && right.len() == 1 {
        return (1, 1);
    }
    let mut best = (1, 1);
    let mut best_score = i64::MIN;
    for (ln, rn) in [(1, 1), (1, 2), (2, 1)] {
        if left.len() < ln || right.len() < rn || !candidate_allowed(&left[..ln], &right[..rn]) {
            continue;
        }
        let bytes = left[..ln]
            .iter()
            .chain(&right[..rn])
            .map(|line| line.content.len() + 1)
            .sum::<usize>();
        // Each candidate is bounded independently, and all candidates share
        // a linear per-job byte budget. Charge before allocating block text.
        if bytes > MAX_ALIGNMENT_BYTES || bytes > *remaining_bytes {
            if ln == 1 && rn == 1 {
                return best;
            }
            continue;
        }
        *remaining_bytes -= bytes;
        let score = content_overlap_score(&block_text(&left[..ln]).0, &block_text(&right[..rn]).0);
        if ln == 1 && rn == 1 {
            best_score = score;
        } else if score > 0 && score > best_score {
            best_score = score;
            best = (ln, rn);
        }
    }
    best
}

fn pair_positionally(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
    lr: Range<usize>,
    rr: Range<usize>,
    output: &mut RolesAndEmphasis,
    remaining_lookahead_bytes: &mut usize,
) {
    let (mut i, mut j) = (lr.start, rr.start);
    while i < lr.end && j < rr.end {
        if is_blank(left[i]) && !is_blank(right[j]) {
            i += 1;
        } else if !is_blank(left[i]) && is_blank(right[j]) {
            j += 1;
        } else {
            let (ln, rn) = fallback_shape(
                &left[i..lr.end],
                &right[j..rr.end],
                remaining_lookahead_bytes,
            );
            apply_pair(left, right, i..i + ln, j..j + rn, output);
            i += ln;
            j += rn;
        }
    }
}

fn align_region(
    left: &[DiffToken<'_>],
    right: &[DiffToken<'_>],
    lr: Range<usize>,
    rr: Range<usize>,
    output: &mut RolesAndEmphasis,
    remaining_work: &mut usize,
    remaining_lookahead_bytes: &mut usize,
) {
    if lr.is_empty() || rr.is_empty() {
        return;
    }
    if left[lr.clone()] == right[rr.clone()] {
        output.0[lr].fill(Role::Equal);
        output.1[rr].fill(Role::Equal);
        return;
    }
    // Most edit jobs consist of singleton replacements between exact lines.
    // Preserve their direct word-diff path without a refinement allocation.
    if lr.len() == 1 && rr.len() == 1 {
        pair_positionally(left, right, lr, rr, output, remaining_lookahead_bytes);
        return;
    }
    let Some(pairs) = refined_pairs(&left[lr.clone()], &right[rr.clone()], remaining_work) else {
        pair_positionally(left, right, lr, rr, output, remaining_lookahead_bytes);
        return;
    };
    let (mut i, mut j) = (lr.start, rr.start);
    for pair in pairs {
        let pl = lr.start + pair.left.start..lr.start + pair.left.end;
        let pr = rr.start + pair.right.start..rr.start + pair.right.end;
        pair_positionally(
            left,
            right,
            i..pl.start,
            j..pr.start,
            output,
            remaining_lookahead_bytes,
        );
        i = pl.end;
        j = pr.end;
        apply_pair(left, right, pl, pr, output);
    }
    pair_positionally(
        left,
        right,
        i..lr.end,
        j..rr.end,
        output,
        remaining_lookahead_bytes,
    );
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
    /// Positive only when shared word content exceeds 40%; whitespace and
    /// punctuation cannot create a correspondence between unrelated blocks.
    alignment_score: i64,
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

fn meaningful_bytes(text: &[u8]) -> usize {
    text.iter()
        .filter(|&&byte| byte.is_ascii_alphanumeric() || byte >= 128)
        .count()
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
            alignment_score: 6 * meaningful_bytes(left_content) as i64,
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
    let mut matched_content_bytes = 0usize;
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
                    matched_content_bytes += meaningful_bytes(tok);
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
        alignment_score: 10 * matched_content_bytes as i64
            - 2 * (meaningful_bytes(left_content) + meaningful_bytes(right_content)) as i64,
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

    fn emphasized_text(text: &str, spans: &[Vec<Range<usize>>]) -> Vec<String> {
        let segments = segment(text);
        assert_eq!(segments.len(), spans.len());
        segments
            .iter()
            .zip(spans)
            .flat_map(|(line, ranges)| {
                let content = &text[line.range.clone()];
                ranges
                    .iter()
                    .map(move |range| content[range.clone()].to_owned())
            })
            .collect()
    }

    #[test]
    fn extra_blank_lines_preserve_both_paragraph_word_edits() {
        let pa =
            "The quarterly revenue report shows steady growth across every regional market segment";
        let pb = "My favorite weeknight recipe combines fresh basil ripe tomatoes minced garlic and olive oil";
        let pa2 =
            "The quarterly revenue report shows steady growth across each regional market segment";
        let pb2 = "My favorite weeknight recipe blends fresh basil ripe tomatoes minced garlic and olive oil";
        let left = format!("{pa}\n\n{pb}");
        for right in [
            format!("\n{pa2}\n\n{pb2}"),
            format!("{pa2}\n\n\n{pb2}"),
            format!("{pa2}\n\n{pb2}\n\n"),
        ] {
            for (a, b, expected_a, expected_b) in [
                (
                    &left,
                    &right,
                    vec!["every", "combines"],
                    vec!["each", "blends"],
                ),
                (
                    &right,
                    &left,
                    vec!["each", "blends"],
                    vec!["every", "combines"],
                ),
            ] {
                let (_, _, le, re) = roles_emph(a, b);
                assert_eq!(emphasized_text(a, &le), expected_a);
                assert_eq!(emphasized_text(b, &re), expected_b);
            }
        }
    }

    #[test]
    fn split_and_join_preserve_word_edits_without_emphasizing_continuations() {
        let left = "Café deployment report includes approved changes, current risks, release dates and test results.\n\nThe migration guide describes recovery procedures for the production application.";
        let right = "Café deployment report includes pending changes, current risks,\nrelease dates and test results.\n\nThe migration guide describes recovery procedures for the staging application.";
        for (a, b, expected_a, expected_b) in [
            (
                left,
                right,
                vec!["approved", "production"],
                vec!["pending", "staging"],
            ),
            (
                right,
                left,
                vec!["pending", "staging"],
                vec!["approved", "production"],
            ),
        ] {
            let (_, _, le, re) = roles_emph(a, b);
            let meaningful = |text: &str, spans: &[Vec<Range<usize>>]| {
                emphasized_text(text, spans)
                    .into_iter()
                    .filter(|span| span.chars().any(char::is_alphanumeric))
                    .collect::<Vec<_>>()
            };
            assert_eq!(meaningful(a, &le), expected_a);
            assert_eq!(meaningful(b, &re), expected_b);
        }
    }

    #[test]
    fn split_and_join_at_an_accent_do_not_emphasize_unchanged_text() {
        for replacement in ["new", "old"] {
            let left = "Before café old value\nfooter";
            let right = format!("Before café\n{replacement} value\nfooter");
            for (a, b, ea, eb) in [
                (left, right.as_str(), "old", replacement),
                (right.as_str(), left, replacement, "old"),
            ] {
                let (_, _, le, re) = roles_emph(a, b);
                let expected = |word: &str| {
                    if replacement == "old" {
                        Vec::<String>::new()
                    } else {
                        vec![word.to_owned()]
                    }
                };
                assert_eq!(emphasized_text(a, &le), expected(ea));
                assert_eq!(emphasized_text(b, &re), expected(eb));
            }
        }
    }

    #[test]
    fn exhausted_alignment_budget_does_not_cascade_after_a_split_or_join() {
        let left = (0..20).map(|i| format!("Section {i:02} records approved changes, current risks, release dates, test results, and the owner responsible for every follow-up action in the production application.")).collect::<Vec<_>>().join("\n\n");
        let right = left.replace("approved", "pending").replacen(
            "current risks, release dates",
            "current risks,\nrelease dates",
            1,
        );
        for (a, b, expected_a, expected_b) in [
            (&left, &right, "approved", "pending"),
            (&right, &left, "pending", "approved"),
        ] {
            let la = tokenize(a);
            let rb = tokenize(b);
            let mut refinement_budget = MAX_ALIGNMENT_WORK;
            assert!(refined_pairs(&la, &rb, &mut refinement_budget).is_none());
            let mut output = (
                vec![Role::AddedOrRemoved; la.len()],
                vec![Role::AddedOrRemoved; rb.len()],
                vec![Vec::new(); la.len()],
                vec![Vec::new(); rb.len()],
            );
            let mut remaining_bytes = 4 * (a.len() + b.len());
            align_region(
                &la,
                &rb,
                0..la.len(),
                0..rb.len(),
                &mut output,
                &mut 0,
                &mut remaining_bytes,
            );
            assert_eq!(emphasized_text(a, &output.2), vec![expected_a; 20]);
            assert_eq!(emphasized_text(b, &output.3), vec![expected_b; 20]);
            assert!(
                remaining_bytes > 0,
                "fixed lookahead must stay within a linear byte budget"
            );
        }
    }

    #[test]
    fn fallback_candidate_limits_are_checked_before_scoring() {
        let left = tokenize("color red\ncolor blue");
        let right = tokenize("color green\ncolor yellow");
        assert_eq!(fallback_shape(&left, &right, &mut 0), (1, 1));
        let long = "a".repeat(MAX_ALIGNMENT_BYTES + 1);
        let large_left = tokenize(&long);
        let mut budget = usize::MAX;
        assert_eq!(fallback_shape(&large_left, &right, &mut budget), (1, 1));
        assert_eq!(budget, usize::MAX);

        // An extra punctuation-only line adds no meaningful content. Equal
        // scores must preserve the singleton instead of swallowing it.
        let same = tokenize("same content");
        let inserted = tokenize("same content\n!!!");
        assert_eq!(fallback_shape(&same, &inserted, &mut budget), (1, 1));
        assert_eq!(fallback_shape(&inserted, &same, &mut budget), (1, 1));
    }

    #[test]
    fn moved_line_does_not_steal_the_following_paragraph_pair() {
        let left = "Shared heading\nOld review records\nBuild screenshots and test transcripts\nThe deployment record preserves financial security and operational acceptance evidence.";
        let right = "Shared heading\nBuild screenshots and test transcripts\nOld review records\nThe deployment record preserves financial security and operational acceptance evidence, and identity.";
        let (_, _, le, re) = roles_emph(left, right);
        assert!(emphasized_text(left, &le).is_empty());
        assert_eq!(emphasized_text(right, &re), vec![", and identity"]);
    }

    #[test]
    fn unrelated_long_paragraphs_keep_line_only_changes() {
        let left = "The engineering team documented network settings firewall rules service accounts deployment logs and database credentials for the production environment.";
        let right = "A village bakery offered fresh bread seasonal pastries roasted coffee homemade jam and friendly conversation for travelers arriving early in the morning.";
        let (lr, rr, le, re) = roles_emph(left, right);
        assert_eq!(lr, vec![Role::AddedOrRemoved]);
        assert_eq!(rr, vec![Role::AddedOrRemoved]);
        assert!(emphasized_text(left, &le).is_empty());
        assert!(emphasized_text(right, &re).is_empty());
    }

    #[test]
    fn capped_alignment_still_skips_extra_blank_lines() {
        let left = (0..70)
            .map(|i| format!("Report {i} records approved changes\n"))
            .collect::<String>();
        let right = format!("\n{}", left.replace("approved", "pending"));
        for (a, b) in [(&left, &right), (&right, &left)] {
            let (_, _, le, re) = roles_emph(a, b);
            assert_eq!(emphasized_text(a, &le).len(), 70);
            assert_eq!(emphasized_text(b, &re).len(), 70);
            assert!(
                emphasized_text(a, &le)
                    .iter()
                    .all(|word| word == "approved" || word == "pending")
            );
            assert!(
                emphasized_text(b, &re)
                    .iter()
                    .all(|word| word == "approved" || word == "pending")
            );
        }
        let mut exhausted = 0;
        assert!(
            refined_pairs(
                &tokenize("alpha red\nbeta blue"),
                &tokenize("alpha green\nbeta yellow"),
                &mut exhausted
            )
            .is_none()
        );
    }

    #[test]
    fn alignment_ties_are_deterministic_and_budget_is_shared() {
        let left = tokenize("color red\ncolor red");
        let right = tokenize("color blue\n");
        let mut budget = MAX_ALIGNMENT_WORK;
        let first = refined_pairs(&left, &right, &mut budget).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].left, 0..1);
        let spent = MAX_ALIGNMENT_WORK - budget;
        assert!(spent > 0);
        for _ in 0..8 {
            let repeated = refined_pairs(&left, &right, &mut budget).unwrap();
            assert_eq!(repeated[0].left, first[0].left);
            assert_eq!(repeated[0].right, first[0].right);
        }
        assert_eq!(budget, MAX_ALIGNMENT_WORK - 9 * spent);
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
