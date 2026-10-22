# Fast Diff Viewer Architecture

This document is a public map of the code and the product constraints that
shape it. The benchmark gates are defined in
[`docs/benchmark-contract.md`](benchmark-contract.md), and the
fast-path algorithms are documented in
[`docs/performance.md`](performance.md).

It opens as a single native executable with two editable panes, asynchronous
diffing, pane-scoped coloring, synchronized scrolling, and no persisted state.

## Product Contract

- Rust-only desktop app built on `eframe 0.37` / `egui 0.37`.
- Supported release targets: Windows 10/11 x64, Ubuntu 22.04 x64,
  Ubuntu 24.04 x64, and macOS 26 arm64.
- Single native executable per supported target: `fast-diff-viewer.exe` on
  Windows, `fast-diff-viewer` on Ubuntu, and a `Fast Diff Viewer.app` bundle
  containing `fast-diff-viewer` on macOS.
- WSL launches keep the Glow/OpenGL renderer but set Mesa software-rendering
  fallback variables before eframe initializes when those variables are unset.
  Regular Ubuntu desktop launches keep the default hardware-preferred path.
- Exactly two editable text panes in a fixed 50/50 layout.
- The panes are stock `TextEdit::multiline` widgets. The app does not
  intercept or consume keyboard, pointer, clipboard, or scroll-wheel events.
- Scroll offset is synchronized between panes by reading
  `ScrollAreaOutput.state.clamped_offset()` and feeding the clamped value into the
  opposite pane's next `ScrollArea::scroll_offset()` call.
- No persistence of window position, pane buffers, theme choice, or other
  cross-launch state.
- Graphite is the shipping theme.
- Pane-scoped diff palette: left pane carries the rose family, right pane
  carries the green family. Equal lines remain untinted.

## Runtime Data Flow

1. Pane buffers live in [`src/app.rs`](../src/app.rs).
2. `detect_and_dispatch()` compares the current buffers to the last submitted
   buffers and submits diff work when either side changes.
3. [`src/pipeline.rs`](../src/pipeline.rs) runs a background worker on
   `std::thread`, coalescing edits to at most one in-flight job plus one
   queued job.
4. The worker tokenizes both buffers, computes line roles and inline emphasis,
   sends the newest result back, and requests repaint.
5. `apply_diff_results()` installs the newest revision. For buffers at or
   above 200 KB, `PendingRightApply` defers the right pane's apply by one
   frame so only one styled galley rebuilds per frame.
6. `build_layout_job()` in [`src/render.rs`](../src/render.rs) paints styled
   output only when text, roles, emphasis, and buffer stamps are aligned.
   Otherwise it emits a neutral untinted job for the whole pane.

## Module Map

| Module | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Entrypoints, `NativeOptions`, viewport setup, window icon, and app launch. |
| [`src/app.rs`](../src/app.rs) | App shell, custom title bar, theme wiring, revision application, pane layout, scroll synchronization, and bench-mode hooks. |
| [`src/pipeline.rs`](../src/pipeline.rs) | Async diff pipeline, worker thread, job coalescing, revision tracking, and liveness reporting. |
| [`src/diff.rs`](../src/diff.rs) | Tokenization, line-role assignment, and inline-emphasis word diff. |
| [`src/segment.rs`](../src/segment.rs) | Canonical line segmentation. |
| [`src/buffer_stamp.rs`](../src/buffer_stamp.rs) | Buffer revision, byte length, and text hash used for styled-paint identity and layout-cache keys. |
| [`src/render.rs`](../src/render.rs) | Styled `LayoutJob` construction and neutral fallback. |
| [`src/theme.rs`](../src/theme.rs) | `AppTheme` chrome and diff-palette tokens. |
| [`src/bench.rs`](../src/bench.rs) | `#[cfg(feature = "bench")]` harness used by the Rust benchmark runner. |

## Implementation Invariants

- `segment()` is the only line segmentation entry point.
- CRLF normalization is compare-only and belongs in `diff::tokenize()`.
- `assign_roles_and_emphasis()` owns line roles and inline emphasis.
- `is_styled_paint_valid()` is the single gate between styled paint and the
  neutral fallback.
- Theme colors live on `AppTheme`; diff palette values are not scattered
  through UI code.
- The pipeline keeps latest-wins behavior with at most one in-flight job and
  one queued job.
- `persist_window: false` is intentional on every app entrypoint.

## Product-Design Decisions

The following are larger product choices rather than incidental code changes. They
require explicit discussion and justification before changing.

- Pane resize, pane swap, or custom hotkeys that intercept native input flow.
- Persisting window state, pane buffers, theme choice, or other user state.
- Operation-scoped diff colors such as red-for-removed and green-for-added.
- A user-facing theme toggle.
- Replacing `TextEdit::multiline` with a different editor surface or forking
  `egui` / `epaint` to change text layout behavior.
