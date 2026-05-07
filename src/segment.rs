use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineSeg {
    pub range: Range<usize>,
    pub ends_with_newline: bool,
}

/// Canonical line segmentation.
///
/// This is the single line-segmentation entry point in the codebase. Diffing,
/// role assignment, rendering, and tests must all go through this function.
///
/// Rules:
/// - Walks the buffer splitting on literal `\n`.
/// - A terminal `\n` does not produce an extra empty segment; the previous
///   segment simply has `ends_with_newline = true`.
/// - If the buffer ends without `\n`, the last segment has
///   `ends_with_newline = false`.
/// - An empty buffer produces zero segments.
/// - Stray isolated `\r` is not a line separator — it stays inside its segment.
///
/// CRLF normalization is *compare-only* and lives in `diff::tokenize`; this
/// function never strips bytes or changes segment counts.
pub fn segment(s: &str) -> Vec<LineSeg> {
    if s.is_empty() {
        return Vec::new();
    }
    let bytes = s.as_bytes();
    let mut segs = Vec::with_capacity(bytes.iter().filter(|&&b| b == b'\n').count() + 1);
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            segs.push(LineSeg {
                range: start..i,
                ends_with_newline: true,
            });
            start = i + 1;
        }
    }
    if start < bytes.len() {
        segs.push(LineSeg {
            range: start..bytes.len(),
            ends_with_newline: false,
        });
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_has_zero_segments() {
        assert!(segment("").is_empty());
    }

    #[test]
    fn single_unterminated_line() {
        let segs = segment("abc");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..3);
        assert!(!segs[0].ends_with_newline);
    }

    #[test]
    fn single_terminated_line_has_no_extra_empty_segment() {
        let segs = segment("abc\n");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..3);
        assert!(segs[0].ends_with_newline);
    }

    #[test]
    fn two_lines_last_terminated() {
        let segs = segment("a\nb\n");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].range, 0..1);
        assert!(segs[0].ends_with_newline);
        assert_eq!(segs[1].range, 2..3);
        assert!(segs[1].ends_with_newline);
    }

    #[test]
    fn two_lines_last_unterminated() {
        let segs = segment("a\nb");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].range, 0..1);
        assert!(segs[0].ends_with_newline);
        assert_eq!(segs[1].range, 2..3);
        assert!(!segs[1].ends_with_newline);
    }

    #[test]
    fn stray_carriage_return_is_not_a_separator() {
        let segs = segment("a\rb");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..3);
        assert!(!segs[0].ends_with_newline);
    }

    #[test]
    fn crlf_is_one_terminated_line_without_splitting_on_cr() {
        // The `\r` stays inside the segment's byte range; the `\n` still
        // terminates the line. Compare-only normalization happens in
        // diff::tokenize, not here.
        let segs = segment("a\r\n");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..2); // "a\r"
        assert!(segs[0].ends_with_newline);
    }

    #[test]
    fn blank_terminated_line_counts() {
        // A buffer that is just "\n" has one empty line terminated by newline.
        let segs = segment("\n");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..0);
        assert!(segs[0].ends_with_newline);
    }

    #[test]
    fn blank_lines_between_content() {
        let segs = segment("a\n\nb");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].range, 0..1);
        assert!(segs[0].ends_with_newline);
        assert_eq!(segs[1].range, 2..2); // empty line
        assert!(segs[1].ends_with_newline);
        assert_eq!(segs[2].range, 3..4);
        assert!(!segs[2].ends_with_newline);
    }

    // Performance regression guard. Gated behind both `bench`
    // (to stay out of the default test set) and `not(debug_assertions)`
    // (timing assertions are only meaningful under `--release`). Run
    // with `cargo test --features bench --release`.
    #[cfg(all(feature = "bench", not(debug_assertions)))]
    #[test]
    fn perf_10k_lines_under_budget() {
        use std::time::Instant;
        // Budget source: reference workstation observed < 0.5 ms on
        // `segment` over 10 000 lines; 10× headroom → 5 ms.
        const BUDGET_MS: u128 = 5;
        let mut s = String::with_capacity(10_000 * 32);
        for i in 0..10_000 {
            s.push_str(&format!("line {i:05}: content content content\n"));
        }
        let start = Instant::now();
        let segs = segment(&s);
        let elapsed = start.elapsed();
        assert_eq!(segs.len(), 10_000);
        assert!(
            elapsed.as_millis() <= BUDGET_MS,
            "segment(10k lines) took {:?}, budget {} ms",
            elapsed,
            BUDGET_MS
        );
    }
}
