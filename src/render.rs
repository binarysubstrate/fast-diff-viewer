use std::hash::{Hash, Hasher};
use std::ops::Range;

use ahash::AHasher;
use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};

use crate::buffer_stamp::BufferStamp;
pub use crate::buffer_stamp::hash_text;
use crate::diff::Role;
use crate::segment::segment;
use crate::theme::AppTheme;

/// Identifies which of the two panes is being rendered. The palette is
/// pane-aware by design.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum PaneSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintIdentity {
    pub current_stamp: BufferStamp,
    pub source_stamp: BufferStamp,
}

impl PaintIdentity {
    pub fn new(current_stamp: BufferStamp, source_stamp: BufferStamp) -> Self {
        Self {
            current_stamp,
            source_stamp,
        }
    }
}

pub struct LayoutPaint<'a> {
    pub identity: PaintIdentity,
    pub pane: PaneSide,
    pub font_id: FontId,
    pub text_color: Color32,
    pub theme: &'a AppTheme,
}

fn line_tint(theme: &AppTheme, pane: PaneSide, role: Role) -> Color32 {
    match (pane, role) {
        (_, Role::Equal) => Color32::TRANSPARENT,
        (PaneSide::Left, Role::Changed) | (PaneSide::Left, Role::AddedOrRemoved) => {
            theme.removed_line
        }
        (PaneSide::Right, Role::Changed) | (PaneSide::Right, Role::AddedOrRemoved) => {
            theme.added_line
        }
    }
}

fn emphasis_tint(theme: &AppTheme, pane: PaneSide) -> Color32 {
    match pane {
        PaneSide::Left => theme.removed_inline,
        PaneSide::Right => theme.added_inline,
    }
}

/// Per-line gutter marker for the gutter surface. Returns the
/// character to paint in the pane's gutter column for a given role,
/// or `None` if no marker should render (equal lines).
///
/// The mapping is pane-scoped:
/// - Left pane paints `−` (U+2212 MINUS SIGN) for `Changed` or
///   `AddedOrRemoved` — the pane semantically represents
///   "previous/removed content," so any non-equal line is a removal
///   relative to the right pane.
/// - Right pane paints `+` (U+002B PLUS SIGN) for `Changed` or
///   `AddedOrRemoved` — mirror semantic on the right side.
/// - Equal lines in both panes return `None`.
pub fn gutter_marker_for_role(side: PaneSide, role: Role) -> Option<char> {
    match (side, role) {
        (_, Role::Equal) => None,
        (PaneSide::Left, _) => Some('−'),
        (PaneSide::Right, _) => Some('+'),
    }
}

/// Per-pane gutter marker color drawn from the active theme. Left
/// uses `removed_inline` (the red-family word-level emphasis token);
/// right uses `added_inline` (green-family). Equal-line gutters are
/// not painted, so no color is needed there.
pub fn gutter_marker_color(theme: &AppTheme, side: PaneSide) -> Color32 {
    match side {
        PaneSide::Left => theme.removed_inline,
        PaneSide::Right => theme.added_inline,
    }
}

/// Pure `LayoutJob` builder. No `egui::Ui` dependency, so the
/// styled-render alignment contract is directly unit-testable.
///
/// Alignment contract:
/// - A styled paint is valid only when
///   `line_roles.len() == inline_emphasis.len() == segment(text).len()`
///   and the current buffer stamp matches the stamp that produced the
///   roles and inline emphasis.
/// - If any of those disagree, the whole buffer is emitted as a single
///   neutral (untinted) section with no inline emphasis — the plan's
///   explicit whole-pane fallback — so the renderer never mis-colors or
///   emphasizes during the window between an edit and the arrival of its
///   matching diff result.
/// - An empty buffer produces an empty job with zero sections.
///
/// Emphasis overlay rule:
/// - Inline emphasis is only applied on `Role::Changed` lines. Pure
///   add-only and remove-only lines (`Role::AddedOrRemoved`) render with
///   the line tint only, never inline emphasis, even if `inline_emphasis`
///   carries ranges for them.
/// - Each emphasis range is expected to lie within `0..segment.range.len()`
///   and to be non-overlapping, in ascending order. Out-of-bounds or
///   overlapping ranges for a given line cause that line to degrade to
///   "tint only" rather than corrupting section byte ranges.
///
/// Single source of truth for the styled-render alignment invariant.
///
/// Returns `true` when `build_layout_job` may paint per-line styling
/// (role tints + optional emphasis overlays) for `text`. Returns `false`
/// when the whole-pane neutral fallback will be used instead because
/// `line_roles.len()` / `inline_emphasis.len()` disagree with
/// `segment(text).len()` or because the current buffer stamp does not
/// match the stamp that produced those roles and emphasis. An empty
/// buffer with empty vectors and matching stamps is valid but emits no
/// sections.
///
/// Used by `build_layout_job` internally; also exposed for the benchmark
/// harness and the layouter so a caller can record
/// `LastPaintedStyledRevision` only on identity-aligned styled paints
/// without duplicating the validity logic.
pub fn is_styled_paint_valid(
    text: &str,
    line_roles: &[Role],
    inline_emphasis: &[Vec<Range<usize>>],
    identity: PaintIdentity,
) -> bool {
    let seg_count = if text.is_empty() {
        0
    } else {
        segment(text).len()
    };
    is_styled_paint_valid_for_segment_count(
        seg_count,
        text.len(),
        line_roles,
        inline_emphasis,
        identity,
    )
}

fn is_styled_paint_valid_for_segment_count(
    seg_count: usize,
    text_len: usize,
    line_roles: &[Role],
    inline_emphasis: &[Vec<Range<usize>>],
    identity: PaintIdentity,
) -> bool {
    identity.current_stamp.byte_len == text_len
        && identity.current_stamp == identity.source_stamp
        && seg_count == line_roles.len()
        && seg_count == inline_emphasis.len()
}

pub fn build_layout_job(
    text: &str,
    line_roles: &[Role],
    inline_emphasis: &[Vec<Range<usize>>],
    paint: LayoutPaint<'_>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.is_empty() {
        return job;
    }
    let segs = segment(text);
    let neutral_fmt = TextFormat {
        font_id: paint.font_id.clone(),
        color: paint.text_color,
        ..Default::default()
    };
    if !is_styled_paint_valid_for_segment_count(
        segs.len(),
        text.len(),
        line_roles,
        inline_emphasis,
        paint.identity,
    ) {
        // Whole-pane neutral fallback. Emit one section per segment
        // (line + trailing '\n' when present) instead of a single giant
        // section: egui's font layout runs a word-wrap pass across each
        // section, and feeding it the full buffer as one section is
        // ~3x slower than feeding it pre-split per-line sections on
        // large buffers. The visual output is identical.
        for seg in &segs {
            let end = if seg.ends_with_newline {
                seg.range.end + 1
            } else {
                seg.range.end
            };
            job.append(&text[seg.range.start..end], 0.0, neutral_fmt.clone());
        }
        return job;
    }

    // Run-length consolidation: consecutive Role::Equal segments all share
    // the neutral (transparent-tint, no-emphasis) TextFormat, so we coalesce
    // any such run — including interior and trailing '\n' bytes — into a
    // single LayoutJob section. For a 10k-line buffer with 5% divergence
    // this drops the section count from ~21k to ~1.5k, which is the hot
    // path for the large_paste and edit_latency benchmark gates.
    let mut i = 0usize;
    while i < segs.len() {
        if line_roles[i] == Role::Equal {
            let run_start = i;
            while i < segs.len() && line_roles[i] == Role::Equal {
                i += 1;
            }
            let start = segs[run_start].range.start;
            let last = &segs[i - 1];
            let end = if last.ends_with_newline {
                last.range.end + 1
            } else {
                last.range.end
            };
            job.append(&text[start..end], 0.0, neutral_fmt.clone());
            continue;
        }

        let seg = &segs[i];
        let role = line_roles[i];
        let emph_spans = &inline_emphasis[i];
        // Slice including the trailing '\n' (when present): egui's
        // `add_row_backgrounds` lays out background rects per glyph's
        // logical_rect and a '\n' is a zero-advance glyph at end of row,
        // so folding the newline into the tinted section neither bleeds
        // onto the next row nor changes the tint's visible width. This
        // shaves one section per line — ~500 sections on large_paste's
        // 10k-line buffer — off the layouter's hot path.
        let line_end = if seg.ends_with_newline {
            seg.range.end + 1
        } else {
            seg.range.end
        };
        let line_with_newline = &text[seg.range.start..line_end];
        let line_slice = &text[seg.range.clone()];
        let tint = line_tint(paint.theme, paint.pane, role);
        let base_fmt = TextFormat {
            font_id: paint.font_id.clone(),
            color: paint.text_color,
            background: tint,
            ..Default::default()
        };

        let mut use_overlay = role == Role::Changed && !emph_spans.is_empty();
        if use_overlay && !ranges_in_bounds_and_ordered(emph_spans, line_slice.len()) {
            use_overlay = false;
        }

        if !use_overlay {
            job.append(line_with_newline, 0.0, base_fmt);
        } else {
            let emph_fmt = TextFormat {
                font_id: paint.font_id.clone(),
                color: paint.text_color,
                background: emphasis_tint(paint.theme, paint.pane),
                ..Default::default()
            };
            let mut cursor = 0usize;
            for r in emph_spans {
                if cursor < r.start {
                    job.append(&line_slice[cursor..r.start], 0.0, base_fmt.clone());
                }
                job.append(&line_slice[r.start..r.end], 0.0, emph_fmt.clone());
                cursor = r.end;
            }
            // Tail of the line (after the last emphasis run) plus the
            // trailing '\n' (if any) share the base tint format, so fold
            // them into a single section.
            let tail_start = cursor;
            if tail_start < line_slice.len() || seg.ends_with_newline {
                let tail_slice = &text[seg.range.start + tail_start..line_end];
                job.append(tail_slice, 0.0, base_fmt);
            }
        }
        i += 1;
    }
    job
}

fn ranges_in_bounds_and_ordered(ranges: &[Range<usize>], line_len: usize) -> bool {
    let mut last_end = 0usize;
    for r in ranges {
        if r.start < last_end || r.start > r.end || r.end > line_len {
            return false;
        }
        last_end = r.end;
    }
    true
}

/// Deterministic `line_roles` hash for the layout cache key.
pub fn hash_roles(roles: &[Role]) -> u64 {
    let mut h = AHasher::default();
    roles.hash(&mut h);
    h.finish()
}

/// Deterministic `inline_emphasis` hash for the layout cache key.
/// `Range<T>` does not implement `Hash`, so we hash `start` and `end` by
/// hand.
pub fn hash_inline_emphasis(emphasis: &[Vec<Range<usize>>]) -> u64 {
    let mut h = AHasher::default();
    emphasis.len().hash(&mut h);
    for line in emphasis {
        line.len().hash(&mut h);
        for r in line {
            r.start.hash(&mut h);
            r.end.hash(&mut h);
        }
    }
    h.finish()
}

/// Deterministic hash of paint inputs that can change `LayoutJob` formats.
pub fn hash_paint_inputs(theme: &AppTheme, pane: PaneSide, text_color: Color32) -> u64 {
    let mut h = AHasher::default();
    pane.hash(&mut h);
    hash_color(&mut h, text_color);
    match pane {
        PaneSide::Left => {
            hash_color(&mut h, theme.removed_line);
            hash_color(&mut h, theme.removed_inline);
        }
        PaneSide::Right => {
            hash_color(&mut h, theme.added_line);
            hash_color(&mut h, theme.added_inline);
        }
    }
    h.finish()
}

fn hash_color(h: &mut AHasher, color: Color32) {
    color.to_array().hash(h);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::text::ByteIndex;

    fn mono() -> FontId {
        FontId::monospace(12.0)
    }

    fn fg() -> Color32 {
        Color32::BLACK
    }

    fn theme() -> AppTheme {
        AppTheme::paper()
    }

    fn stamp(revision: u64, text: &str) -> BufferStamp {
        BufferStamp::from_text(revision, text)
    }

    fn identity(current_stamp: BufferStamp, source_stamp: BufferStamp) -> PaintIdentity {
        PaintIdentity::new(current_stamp, source_stamp)
    }

    fn build_job(
        text: &str,
        roles: &[Role],
        emph: &[Vec<Range<usize>>],
        pane: PaneSide,
    ) -> LayoutJob {
        let source_stamp = stamp(1, text);
        build_layout_job(
            text,
            roles,
            emph,
            LayoutPaint {
                identity: identity(source_stamp, source_stamp),
                pane,
                font_id: mono(),
                text_color: fg(),
                theme: &theme(),
            },
        )
    }

    fn build_job_with_stamps(
        text: &str,
        roles: &[Role],
        emph: &[Vec<Range<usize>>],
        current_stamp: BufferStamp,
        source_stamp: BufferStamp,
        pane: PaneSide,
    ) -> LayoutJob {
        build_layout_job(
            text,
            roles,
            emph,
            LayoutPaint {
                identity: identity(current_stamp, source_stamp),
                pane,
                font_id: mono(),
                text_color: fg(),
                theme: &theme(),
            },
        )
    }

    fn changed_vectors_for(text: &str) -> (Vec<Role>, Vec<Vec<Range<usize>>>) {
        let count = segment(text).len();
        (vec![Role::Changed; count], vec![Vec::new(); count])
    }

    #[test]
    fn empty_buffer_produces_empty_job() {
        let job = build_job("", &[], &[], PaneSide::Left);
        assert!(job.is_empty());
    }

    // Exhaustive coverage of the (PaneSide, Role) -> Option<char> mapping.
    #[test]
    fn gutter_marker_for_role_covers_all_combinations() {
        assert_eq!(gutter_marker_for_role(PaneSide::Left, Role::Equal), None);
        assert_eq!(
            gutter_marker_for_role(PaneSide::Left, Role::Changed),
            Some('−')
        );
        assert_eq!(
            gutter_marker_for_role(PaneSide::Left, Role::AddedOrRemoved),
            Some('−')
        );
        assert_eq!(gutter_marker_for_role(PaneSide::Right, Role::Equal), None);
        assert_eq!(
            gutter_marker_for_role(PaneSide::Right, Role::Changed),
            Some('+')
        );
        assert_eq!(
            gutter_marker_for_role(PaneSide::Right, Role::AddedOrRemoved),
            Some('+')
        );
    }

    // The color selector pulls the word-level emphasis tokens of the
    // appropriate hue family from the theme.
    #[test]
    fn gutter_marker_color_picks_per_theme_inline_tokens() {
        let t = theme();
        assert_eq!(gutter_marker_color(&t, PaneSide::Left), t.removed_inline);
        assert_eq!(gutter_marker_color(&t, PaneSide::Right), t.added_inline);
    }

    #[test]
    fn length_mismatch_roles_longer_falls_back_to_whole_buffer_neutral() {
        let roles = vec![Role::Changed, Role::AddedOrRemoved];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![], vec![]];
        let job = build_job("abc", &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn length_mismatch_emphasis_different_length_from_roles_falls_back() {
        // Roles and segs agree (1 each) but inline_emphasis has 0 entries:
        // the styled-render invariant fails → whole-pane neutral.
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![];
        let job = build_job("abc", &roles, &emph, PaneSide::Left);
        assert_eq!(
            job.sections.len(),
            1,
            "mismatched emphasis length must force whole-pane neutral"
        );
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn neutral_fallback_emits_per_segment_neutral_sections() {
        // Two segments, mismatched roles/emphasis → neutral fallback.
        // Each segment is emitted as its own neutral section so egui's
        // per-section word-wrap pass operates on short slices instead of
        // the whole buffer. All sections carry Color32::TRANSPARENT.
        let text = "abc\ndef";
        let roles = vec![Role::Changed, Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..1]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(4));
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
        assert_eq!(job.sections[1].byte_range, ByteIndex(4)..ByteIndex(7));
        assert_eq!(job.sections[1].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn stale_inline_emphasis_suppresses_emphasis_overlay_when_lengths_match() {
        // Single-segment buffer with mismatched emphasis length → neutral
        // fallback. The per-segment neutral emitter produces exactly one
        // section here since the buffer is a single line.
        let text = "abc";
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn left_pane_changed_line_uses_rose_tint_and_rose_emphasis() {
        // One Changed line with emphasis [1..3] on "abc" → split into
        // "a" (tint) + "bc" (emphasis). No trailing newline.
        let text = "abc";
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![1..3]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(1));
        assert_eq!(job.sections[0].format.background, theme().removed_line);
        assert_eq!(job.sections[1].byte_range, ByteIndex(1)..ByteIndex(3));
        assert_eq!(job.sections[1].format.background, theme().removed_inline);
    }

    #[test]
    fn right_pane_uses_green_family_and_differs_from_left() {
        let text = "abc";
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..3]];
        let left = build_job(text, &roles, &emph, PaneSide::Left);
        let right = build_job(text, &roles, &emph, PaneSide::Right);
        assert_eq!(left.sections[0].format.background, theme().removed_inline);
        assert_eq!(right.sections[0].format.background, theme().added_inline);
        assert_ne!(theme().removed_line, theme().added_line);
        assert_ne!(theme().removed_inline, theme().added_inline);
    }

    #[test]
    fn added_or_removed_lines_carry_line_tint_but_no_emphasis_overlay() {
        // Even if emphasis vector has ranges for a pure AddedOrRemoved
        // line, the renderer ignores them: unmatched lines get line tint
        // only.
        let text = "abc";
        let roles = vec![Role::AddedOrRemoved];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![1..2]];
        let job = build_job(text, &roles, &emph, PaneSide::Right);
        // A single section for the whole line carrying only the line tint.
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(3));
        assert_eq!(job.sections[0].format.background, theme().added_line);
    }

    #[test]
    fn equal_lines_stay_transparent_and_unemphasized_even_with_stray_emphasis() {
        let text = "abc";
        let roles = vec![Role::Equal];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..3]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn emphasis_followed_by_tail_emits_three_sections() {
        let text = "abcdef";
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![2..4]];
        let job = build_job(text, &roles, &emph, PaneSide::Right);
        // "ab" tinted + "cd" emphasized + "ef" tinted = 3 sections.
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(2));
        assert_eq!(job.sections[0].format.background, theme().added_line);
        assert_eq!(job.sections[1].byte_range, ByteIndex(2)..ByteIndex(4));
        assert_eq!(job.sections[1].format.background, theme().added_inline);
        assert_eq!(job.sections[2].byte_range, ByteIndex(4)..ByteIndex(6));
        assert_eq!(job.sections[2].format.background, theme().added_line);
    }

    #[test]
    fn out_of_bounds_emphasis_degrades_to_tint_only_without_corrupting_sections() {
        let text = "abc";
        let roles = vec![Role::Changed];
        // 0..99 is out of bounds for "abc" → degrade to tint only.
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..99]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(3));
        assert_eq!(job.sections[0].format.background, theme().removed_line);
    }

    #[test]
    fn terminated_changed_line_folds_newline_into_tint_section() {
        // Previously the trailing '\n' was emitted as its own neutral
        // section; it is folded into the tinted tail section because a
        // '\n' glyph has zero advance
        // and cannot bleed tint onto the next row.
        let text = "a\n";
        let roles = vec![Role::Changed];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..1]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        // "a" (emphasis) + "\n" (tinted tail folded with the line).
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(1));
        assert_eq!(job.sections[0].format.background, theme().removed_inline);
        assert_eq!(job.sections[1].byte_range, ByteIndex(1)..ByteIndex(2));
        assert_eq!(job.sections[1].format.background, theme().removed_line);
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "test fixture deliberately uses single-element Range vecs; vec![0..1] is the emphasis shape under test"
    )]
    fn hash_inline_emphasis_changes_when_ranges_change() {
        let a = hash_inline_emphasis(&[vec![0..1]]);
        let b = hash_inline_emphasis(&[vec![0..2]]);
        let c = hash_inline_emphasis(&[vec![0..1]]);
        assert_ne!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn is_styled_paint_valid_matches_build_layout_job_fallback_decision() {
        // Aligned with segment count → valid.
        let text = "ab\ncd";
        let roles = vec![Role::Changed, Role::Equal];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![0..2], vec![]];
        let matching_stamp = stamp(1, text);
        assert!(is_styled_paint_valid(
            text,
            &roles,
            &emph,
            identity(matching_stamp, matching_stamp)
        ));

        // Mismatched roles → invalid.
        let bad_roles = vec![Role::Changed];
        assert!(!is_styled_paint_valid(
            text,
            &bad_roles,
            &emph,
            identity(matching_stamp, matching_stamp)
        ));

        // Mismatched emphasis → invalid.
        let bad_emph: Vec<Vec<Range<usize>>> = vec![vec![]];
        assert!(!is_styled_paint_valid(
            text,
            &roles,
            &bad_emph,
            identity(matching_stamp, matching_stamp)
        ));

        // Empty buffer with empty vectors is aligned (all zero lengths);
        // the invariant holds vacuously. This is the initial empty-pane
        // state — no sections are emitted, but it is not a neutral
        // fallback either.
        let empty_stamp = stamp(0, "");
        assert!(is_styled_paint_valid(
            "",
            &[],
            &[],
            identity(empty_stamp, empty_stamp)
        ));

        // Empty buffer with non-empty vectors is a mismatch.
        assert!(!is_styled_paint_valid(
            "",
            &[Role::Equal],
            &[vec![]],
            identity(empty_stamp, empty_stamp)
        ));
    }

    #[test]
    fn same_segment_count_different_text_is_invalid_styled_paint() {
        let source = "alpha beta\nsame tail";
        let current = "alpha BETA\nsame tail";
        let (roles, emph) = changed_vectors_for(source);
        assert_eq!(segment(source).len(), segment(current).len());

        assert!(!is_styled_paint_valid(
            current,
            &roles,
            &emph,
            identity(stamp(2, current), stamp(1, source))
        ));
    }

    #[test]
    fn same_segment_count_same_text_and_stamp_is_valid_styled_paint() {
        let text = "alpha beta\nsame tail";
        let (roles, emph) = changed_vectors_for(text);
        let matching_stamp = stamp(7, text);

        assert!(is_styled_paint_valid(
            text,
            &roles,
            &emph,
            identity(matching_stamp, matching_stamp)
        ));
    }

    #[test]
    fn same_text_with_stale_revision_is_invalid_styled_paint() {
        let text = "alpha beta\nsame tail";
        let (roles, emph) = changed_vectors_for(text);

        assert!(!is_styled_paint_valid(
            text,
            &roles,
            &emph,
            identity(stamp(8, text), stamp(7, text))
        ));
    }

    #[test]
    fn stale_same_count_layout_falls_back_to_neutral() {
        let source = "alpha beta\nsame tail";
        let current = "alpha BETA\nsame tail";
        let (roles, emph) = changed_vectors_for(source);

        let job = build_job_with_stamps(
            current,
            &roles,
            &emph,
            stamp(2, current),
            stamp(1, source),
            PaneSide::Left,
        );

        assert_eq!(job.sections.len(), segment(current).len());
        assert!(
            job.sections
                .iter()
                .all(|section| section.format.background == Color32::TRANSPARENT)
        );
    }

    #[test]
    fn crlf_lf_raw_identity_mismatch_is_invalid_despite_equal_segments() {
        let source = "a\r\nb";
        let current = "a\nb";
        let (roles, emph) = changed_vectors_for(source);
        assert_eq!(segment(source).len(), segment(current).len());

        assert!(!is_styled_paint_valid(
            current,
            &roles,
            &emph,
            identity(stamp(2, current), stamp(1, source))
        ));
    }

    #[test]
    fn unicode_same_count_stale_text_is_invalid() {
        let source = "unicode cafe Δ\nsame tail";
        let current = "unicode cafe Ω\nsame tail";
        let (roles, emph) = changed_vectors_for(source);
        assert_eq!(segment(source).len(), segment(current).len());

        assert!(!is_styled_paint_valid(
            current,
            &roles,
            &emph,
            identity(stamp(2, current), stamp(1, source))
        ));
    }

    #[test]
    fn tab_containing_same_count_stale_text_is_invalid() {
        let source = "tab\tleft\tvalue\nsame tail";
        let current = "tab\tright\tvalue\nsame tail";
        let (roles, emph) = changed_vectors_for(source);
        assert_eq!(segment(source).len(), segment(current).len());

        assert!(!is_styled_paint_valid(
            current,
            &roles,
            &emph,
            identity(stamp(2, current), stamp(1, source))
        ));
    }

    #[test]
    fn large_same_line_count_replacement_is_invalid_until_matching_stamp_lands() {
        let mut source = String::with_capacity(1_024 * 48);
        let mut current = String::with_capacity(1_024 * 48);
        for i in 0..1_024 {
            source.push_str(&format!("line {i:04}: left payload stays same-count\n"));
            current.push_str(&format!("line {i:04}: right payload stays same-count\n"));
        }
        let (roles, emph) = changed_vectors_for(&source);
        assert_eq!(segment(&source).len(), segment(&current).len());

        assert!(!is_styled_paint_valid(
            &current,
            &roles,
            &emph,
            identity(stamp(2, &current), stamp(1, &source))
        ));

        let (current_roles, current_emph) = changed_vectors_for(&current);
        let matching_stamp = stamp(2, &current);
        assert!(is_styled_paint_valid(
            &current,
            &current_roles,
            &current_emph,
            identity(matching_stamp, matching_stamp)
        ));
    }

    #[test]
    fn consecutive_equal_lines_collapse_into_single_section() {
        // Five consecutive Equal lines with terminal newlines. Before
        // run-length consolidation this produced 10 sections (5 line
        // bodies + 5 newline separators). After consolidation it must
        // produce exactly 1 neutral section spanning the full buffer.
        let text = "a\nb\nc\nd\ne\n";
        let roles = vec![Role::Equal; 5];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![]; 5];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(
            job.sections[0].byte_range,
            ByteIndex(0)..ByteIndex(text.len())
        );
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn equal_run_consolidates_around_changed_lines() {
        // Pattern: 2 Equal, 1 Changed with emphasis, 2 Equal (final line
        // without trailing newline). The two Equal runs each collapse to
        // one section; the Changed line still splits into tint/emphasis
        // sections plus its own neutral newline separator. Exact section
        // layout: [Equal run "a\nb\n"] + [Changed "c" tinted] +
        // [Changed emphasis "d"] + ["\n" neutral] + [Equal run "e\nf"] = 5.
        let text = "a\nb\ncd\ne\nf";
        let roles = vec![
            Role::Equal,
            Role::Equal,
            Role::Changed,
            Role::Equal,
            Role::Equal,
        ];
        let emph: Vec<Vec<Range<usize>>> = vec![vec![], vec![], vec![1..2], vec![], vec![]];
        let job = build_job(text, &roles, &emph, PaneSide::Left);
        assert_eq!(job.sections.len(), 5);
        // First Equal run spans "a\nb\n".
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(4));
        assert_eq!(job.sections[0].format.background, Color32::TRANSPARENT);
        // Changed line tint + emphasis.
        assert_eq!(job.sections[1].byte_range, ByteIndex(4)..ByteIndex(5));
        assert_eq!(job.sections[1].format.background, theme().removed_line);
        assert_eq!(job.sections[2].byte_range, ByteIndex(5)..ByteIndex(6));
        assert_eq!(job.sections[2].format.background, theme().removed_inline);
        // Trailing '\n' of the Changed line is folded into the tinted tail
        // section (previously emitted as a separate neutral section).
        assert_eq!(job.sections[3].byte_range, ByteIndex(6)..ByteIndex(7));
        assert_eq!(job.sections[3].format.background, theme().removed_line);
        // Trailing Equal run spans "e\nf".
        assert_eq!(job.sections[4].byte_range, ByteIndex(7)..ByteIndex(10));
        assert_eq!(job.sections[4].format.background, Color32::TRANSPARENT);
    }

    #[test]
    fn hash_roles_and_hash_text_still_stable() {
        assert_eq!(hash_text("abc"), hash_text("abc"));
        assert_ne!(hash_text("abc"), hash_text("abd"));
        assert_eq!(hash_roles(&[Role::Equal]), hash_roles(&[Role::Equal]));
        assert_ne!(hash_roles(&[Role::Equal]), hash_roles(&[Role::Changed]));
    }

    #[test]
    fn hash_paint_inputs_changes_with_theme_or_text_color() {
        let paper = AppTheme::paper();
        let graphite = AppTheme::graphite();
        let paper_hash = hash_paint_inputs(&paper, PaneSide::Left, Color32::BLACK);

        assert_ne!(
            paper_hash,
            hash_paint_inputs(&graphite, PaneSide::Left, Color32::BLACK)
        );
        assert_ne!(
            paper_hash,
            hash_paint_inputs(&paper, PaneSide::Left, Color32::WHITE)
        );
    }

    // Direct tests for the private `ranges_in_bounds_and_ordered`
    // predicate that gates the emphasis
    // overlay in `build_layout_job`. Access is via the same-module
    // `tests` submodule, so no visibility change is needed.

    #[test]
    fn ranges_in_bounds_empty_slice_is_valid() {
        assert!(ranges_in_bounds_and_ordered(&[], 0));
        assert!(ranges_in_bounds_and_ordered(&[], 10));
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "test fixture deliberately passes a slice holding one in-bounds Range"
    )]
    fn ranges_in_bounds_single_in_bounds_range_is_valid() {
        assert!(ranges_in_bounds_and_ordered(&[1..3], 5));
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "test fixture deliberately passes a slice holding one Range touching the upper bound"
    )]
    fn ranges_in_bounds_range_touching_last_byte_is_valid() {
        assert!(ranges_in_bounds_and_ordered(&[0..5], 5));
    }

    #[test]
    fn ranges_in_bounds_two_disjoint_ranges_in_order_are_valid() {
        assert!(ranges_in_bounds_and_ordered(&[0..2, 3..5], 5));
    }

    #[test]
    fn ranges_in_bounds_overlapping_ranges_are_rejected() {
        assert!(!ranges_in_bounds_and_ordered(&[0..3, 2..5], 5));
    }

    #[test]
    fn ranges_in_bounds_out_of_order_ranges_are_rejected() {
        assert!(!ranges_in_bounds_and_ordered(&[3..5, 0..2], 5));
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "test fixture deliberately passes a slice holding one out-of-bounds Range"
    )]
    fn ranges_in_bounds_out_of_bounds_end_is_rejected() {
        assert!(!ranges_in_bounds_and_ordered(&[0..6], 5));
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        clippy::reversed_empty_ranges,
        reason = "test fixture deliberately passes a reversed (start > end) Range to verify the predicate rejects the degenerate case"
    )]
    fn ranges_in_bounds_inverted_range_is_rejected() {
        // Degenerate `start > end` must be rejected; callers should
        // never pass one but the predicate has to be robust.
        assert!(!ranges_in_bounds_and_ordered(&[3..1], 5));
    }

    // Performance regression guard. Gated behind both `bench`
    // (to stay out of the default test set) and `not(debug_assertions)`
    // (timing assertions are only meaningful under `--release`). Run
    // with `cargo test --features bench --release`.
    #[cfg(all(feature = "bench", not(debug_assertions)))]
    #[test]
    fn perf_build_layout_job_10k_lines_under_budget() {
        use std::time::Instant;
        // Budget source: the `large_paste` E2E gate is 160 ms for the
        // full render frame. Isolating `build_layout_job` from egui
        // overhead allows a tighter guard; benchmark evidence shows this
        // function accounts for most of that frame time on the reference
        // workstation. 200 ms budget leaves
        // modest headroom above empirical.
        const BUDGET_MS: u128 = 200;
        // Build a 10k-line styled buffer with ~5% divergence so the
        // role vector exercises tint runs, emphasis overlays, and
        // Equal-run consolidation simultaneously.
        let mut text = String::with_capacity(10_000 * 32);
        let mut roles: Vec<Role> = Vec::with_capacity(10_000);
        let mut emph: Vec<Vec<Range<usize>>> = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            let line = format!("line {i:05}: content content content");
            text.push_str(&line);
            text.push('\n');
            if i % 20 == 0 {
                roles.push(Role::Changed);
                // Emphasis spans a short word inside the line.
                emph.push(vec![11..18]);
            } else {
                roles.push(Role::Equal);
                emph.push(Vec::new());
            }
        }
        let start = Instant::now();
        let job = build_job(&text, &roles, &emph, PaneSide::Left);
        let elapsed = start.elapsed();
        assert!(!job.sections.is_empty());
        assert!(
            elapsed.as_millis() <= BUDGET_MS,
            "build_layout_job(10k lines, 5% divergence) took {:?}, budget {} ms",
            elapsed,
            BUDGET_MS
        );
    }
}
