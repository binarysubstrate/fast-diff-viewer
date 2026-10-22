# Fast Diff Viewer Architecture Reference

## Product Shape

- Rust desktop app built with `eframe 0.37` and `egui 0.37`
- One release artifact per target platform, containing one native app executable
- Exactly two editable panes with a fixed 50/50 layout
- Panes are stock `TextEdit::multiline` widgets
- No persistence of window state, buffers, theme choice, or config
- Custom title bar, but no custom interception of pane typing, selection, pointer, clipboard, or scroll-wheel behavior
- Platform-specific behavior should be explicit and target-gated; shared code should avoid accidental Windows-only assumptions
- Scroll synchronization reads `ScrollAreaOutput.state.clamped_offset()` and feeds clamped offsets back through `ScrollArea::scroll_offset(...)`
- Pane-scoped diff palette: left rose family, right green family, equal lines transparent
- `src/main.rs` owns entrypoint/`NativeOptions` setup; `src/app.rs` owns `App`, pane state, and the eframe `App::ui()` render callback.
- `Cargo.toml` owns target-specific `eframe` feature sets; `build.rs` owns Windows PE icon embedding and must stay non-Windows safe.

## Core Data Flow

1. Pane buffers live in `src/app.rs` on `App`'s pane state.
2. `detect_and_dispatch()` in `src/app.rs` compares the current buffers to the previous revision and submits work when either side changes.
3. `DiffPipeline` in `src/pipeline.rs` runs a background worker on `std::thread` and coalesces edits to at most one in-flight job and one queued job.
4. `worker_loop()` tokenizes both buffers, computes roles and inline emphasis, stamps the exact source buffers, sends the newest result back, and requests repaint.
5. `apply_diff_results()` in `src/app.rs` installs the newest revision and may defer the right pane by one frame through `PendingRightApply` for large buffers.
6. `build_layout_job()` in `src/render.rs` paints styled output only when text, roles, emphasis, and buffer stamps are aligned; otherwise it emits neutral-only output.

## Landmark Functions

| Function | Why it matters |
| --- | --- |
| `segment()` | Canonical line segmentation. Do not create alternate line-splitting logic. |
| `tokenize()` | Owns compare-only CRLF normalization. |
| `assign_roles_and_emphasis()` | Produces both panes' line roles and inline emphasis. |
| `paired_line_diff()` | Word-level token diff and similarity signals for candidate changed-line pairs. |
| `BufferStamp::from_text()` | Exact pane-buffer identity: revision, byte length, and deterministic text hash. |
| `is_styled_paint_valid()` | Single source of truth for styled vs neutral fallback. |
| `build_layout_job()` | Hot path for line tinting, emphasis, section count, and performance. |
| `apply_theme()` | Centralized egui visual overrides and styling gotchas in `src/app.rs`. |
| `detect_and_dispatch()` | App-layer change detection in `src/app.rs`; compares pane buffers and submits latest work. |
| `apply_diff_results()` | Revision application and large-buffer staggering in `src/app.rs`. |
| `render_pane()` | Pane rendering in `src/app.rs`; ties `TextEdit`, `ScrollArea`, gutter, crosshatch, and layout jobs together. |

## Editing Tripwires

### Newline and token behavior

- Trailing newline semantics matter.
- CRLF normalization is compare-only and must not change segmentation counts.
- UTF-8 byte safety matters in inline-emphasis token boundaries.

### Styled rendering

- If line counts or buffer stamps disagree, the renderer must fall back to neutral-only output.
- The invalid-input fallback emits transparent per-segment sections for performance; do not reintroduce one huge whole-buffer section.
- Equal lines are transparent.
- Inline emphasis only applies to `Role::Changed`, never to pure add-only or remove-only lines.
- Gutter, line tint, and emphasis are a single contract.

### Theme and visual state

- Palette values live on `AppTheme`, not as free-floating constants.
- `ctx.all_styles_mut` is required for style changes that must survive theme swaps.
- `visuals.selection.stroke` controls selected glyph color even when width is zero.
- `visuals.text_edit_bg_color = TRANSPARENT` is intentional for the crosshatch treatment.

### Platform and build state

- Windows and macOS builds use `eframe` with `clipboard`, `default_fonts`, and `glow`; Linux builds add `wayland` and `x11`.
- `build.rs` uses `winresource` only for Windows targets.
- CI runs locked release build coverage per supported OS target. Keep target-specific dependencies in the narrowest applicable table.
- GUI-driving visual helpers currently use Win32 APIs and should be treated as Windows validation tools until equivalent drivers exist for other platforms.

### Frame budget and responsiveness

- `build_layout_job()` section count matters.
- Equal-line run consolidation is a real performance optimization.
- Two large `TextEdit::multiline` galley rebuilds in one frame can blow the `large_paste` budget.
- `PendingRightApply` exists to keep large-buffer work to one pane per frame.
- Each pane memoizes one `Galley`; the cache key includes paint identity, role hash, inline-emphasis hash, theme/pane paint hash, and wrap width bits.

## Architecture-Change Territory

These are product-design decisions, not routine implementation edits:

- Pane resize
- Pane swap
- Pane hotkeys that intercept native input flow
- Persistence of window state or buffers
- User-facing theme toggles or theme model expansion that changes the fixed shipped behavior
- Replacing `TextEdit::multiline` with a different editor surface

## Practical Edit Recipes

### Diff bug

Start in `src/segment.rs` and `src/diff.rs`. Add or adjust pure-function tests before touching UI code.

### Render mismatch or stale paint

Check `src/render.rs` first, then `src/app.rs` revision-application flow. The neutral fallback is usually the first thing to verify.

### Theme or styling bug

Check `src/theme.rs` for token changes and `src/app.rs` `apply_theme()` for egui wiring. Do not fix styling drift by scattering color literals.

### Performance regression

Inspect `src/render.rs` section counts, `src/app.rs` stagger logic, and `src/pipeline.rs` coalescing behavior before reaching for new crates or runtime changes.

### Visual validation

Use the `scripts/visual_contract_*.rs` Cargo examples when a change affects rendered output and screenshots are more trustworthy than theory. The current GUI-driving examples are Windows-only at runtime; on other platforms, use equivalent screenshots or analyze existing captures.
