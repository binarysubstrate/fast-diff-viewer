use eframe::egui::Color32;

/// Single source of truth for every color the app paints.
///
/// Diff palette values live here so each theme selects its own pane-scoped
/// colors without scattered `const Color32` edits.
///
/// `graphite()` is the shipping default. `paper()` is retained as a
/// light-theme test fixture so palette unit tests can exercise the locked
/// public values.
#[derive(Clone, Copy, Debug)]
pub struct AppTheme {
    /// Outer "matting" color framing the window (bg_frame in the spec).
    /// Painted in the border region around the app surface when
    /// decorations are off; otherwise unused.
    pub bg_frame: Color32,
    pub bg_app: Color32,
    pub bg_pane: Color32,
    pub ink: Color32,
    pub ink_dim: Color32,
    pub rule: Color32,
    /// Ink color at 3.5-5% alpha — the crosshatch texture color.
    /// Stored pre-mixed so the painter can use it directly without
    /// computing alpha at render time.
    pub hatch_ink: Color32,
    pub caret: Color32,
    pub selection: Color32,
    pub removed_line: Color32,
    pub removed_inline: Color32,
    pub added_line: Color32,
    pub added_inline: Color32,
}

impl AppTheme {
    /// Graphite dark theme, used as the shipping default.
    pub fn graphite() -> Self {
        Self {
            bg_frame: Color32::from_rgb(0x0F, 0x10, 0x13),
            bg_app: Color32::from_rgb(0x17, 0x18, 0x1B),
            bg_pane: Color32::from_rgb(0x1C, 0x1D, 0x21),
            ink: Color32::from_rgb(0xE6, 0xE1, 0xD6),
            ink_dim: Color32::from_rgb(0x7A, 0x78, 0x70),
            rule: Color32::from_rgb(0x2A, 0x2B, 0x30),
            // ~4% ink opacity per spec (`--hatch-ink`). 0.04 × 255 ≈ 10.
            hatch_ink: Color32::from_rgba_unmultiplied(0xE6, 0xE1, 0xD6, 10),
            caret: Color32::from_rgb(0x7E, 0xA7, 0xD8),
            // Accent at ~24% opacity (midpoint of spec's 22-28% range).
            // 0.24 × 255 ≈ 61.
            selection: Color32::from_rgba_unmultiplied(0x7E, 0xA7, 0xD8, 61),
            // Pre-darkened diff palette for dark backgrounds. Inline
            // emphasis reads brighter than the line-level tint while
            // preserving the red-family / green-family identity and
            // body-text legibility. Ink `#E6E1D6` contrast over each
            // inline value clears WCAG AA for normal text (>= 4.5:1):
            // `removed_inline` is about 4.93:1 and `added_inline` is
            // about 4.79:1.
            removed_line: Color32::from_rgb(0x3A, 0x1F, 0x25),
            removed_inline: Color32::from_rgb(0xA0, 0x38, 0x4A),
            added_line: Color32::from_rgb(0x1F, 0x33, 0x27),
            added_inline: Color32::from_rgb(0x31, 0x6D, 0x4C),
        }
    }

    /// Paper light theme. Diff palette values are unchanged byte-for-byte
    /// from the original light palette.
    ///
    /// `paper()` is reachable only from `#[cfg(test)]` callers. Marking the
    /// non-test compilation path as `allow(dead_code)` keeps it available to
    /// release-mode tests without exposing a runtime theme switch.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn paper() -> Self {
        Self {
            bg_frame: Color32::from_rgb(0xEC, 0xEA, 0xE2),
            bg_app: Color32::from_rgb(0xF6, 0xF5, 0xF2),
            bg_pane: Color32::from_rgb(0xFB, 0xFA, 0xF7),
            ink: Color32::from_rgb(0x1F, 0x23, 0x28),
            ink_dim: Color32::from_rgb(0x6B, 0x70, 0x79),
            rule: Color32::from_rgb(0xE6, 0xE2, 0xDA),
            // Ink at ~4% opacity on a light pane.
            hatch_ink: Color32::from_rgba_unmultiplied(0x1F, 0x23, 0x28, 10),
            caret: Color32::from_rgb(0x2B, 0x6C, 0xB0),
            selection: Color32::from_rgba_unmultiplied(0x2B, 0x6C, 0xB0, 56),
            removed_line: Color32::from_rgb(0xF8, 0xE5, 0xE9),
            removed_inline: Color32::from_rgb(0xE9, 0xC2, 0xCB),
            added_line: Color32::from_rgb(0xE6, 0xF3, 0xEA),
            added_inline: Color32::from_rgb(0xBF, 0xE2, 0xC8),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_preserves_diff_palette_byte_for_byte() {
        let t = AppTheme::paper();
        assert_eq!(t.removed_line, Color32::from_rgb(0xF8, 0xE5, 0xE9));
        assert_eq!(t.removed_inline, Color32::from_rgb(0xE9, 0xC2, 0xCB));
        assert_eq!(t.added_line, Color32::from_rgb(0xE6, 0xF3, 0xEA));
        assert_eq!(t.added_inline, Color32::from_rgb(0xBF, 0xE2, 0xC8));
    }

    #[test]
    fn paper_chrome_tokens_match_design_spec() {
        let t = AppTheme::paper();
        assert_eq!(t.bg_app, Color32::from_rgb(0xF6, 0xF5, 0xF2));
        assert_eq!(t.bg_pane, Color32::from_rgb(0xFB, 0xFA, 0xF7));
        assert_eq!(t.ink, Color32::from_rgb(0x1F, 0x23, 0x28));
        assert_eq!(t.ink_dim, Color32::from_rgb(0x6B, 0x70, 0x79));
        assert_eq!(t.rule, Color32::from_rgb(0xE6, 0xE2, 0xDA));
        assert_eq!(t.caret, Color32::from_rgb(0x2B, 0x6C, 0xB0));
    }

    #[test]
    fn graphite_chrome_tokens_match_design_spec() {
        let t = AppTheme::graphite();
        assert_eq!(t.bg_frame, Color32::from_rgb(0x0F, 0x10, 0x13));
        assert_eq!(t.bg_app, Color32::from_rgb(0x17, 0x18, 0x1B));
        assert_eq!(t.bg_pane, Color32::from_rgb(0x1C, 0x1D, 0x21));
        assert_eq!(t.ink, Color32::from_rgb(0xE6, 0xE1, 0xD6));
        assert_eq!(t.ink_dim, Color32::from_rgb(0x7A, 0x78, 0x70));
        assert_eq!(t.rule, Color32::from_rgb(0x2A, 0x2B, 0x30));
        assert_eq!(t.caret, Color32::from_rgb(0x7E, 0xA7, 0xD8));
    }

    #[test]
    fn graphite_diff_palette_is_pre_darkened_for_dark_background() {
        let t = AppTheme::graphite();
        assert_eq!(t.removed_line, Color32::from_rgb(0x3A, 0x1F, 0x25));
        // Inline values are tuned for word-diff visibility on Graphite:
        // still red-family / green-family, still darker than ink
        // `#E6E1D6`, still brighter than `*_line`.
        assert_eq!(t.removed_inline, Color32::from_rgb(0xA0, 0x38, 0x4A));
        assert_eq!(t.added_line, Color32::from_rgb(0x1F, 0x33, 0x27));
        assert_eq!(t.added_inline, Color32::from_rgb(0x31, 0x6D, 0x4C));
    }
}
