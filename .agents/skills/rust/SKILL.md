---
name: rust
description: "Fast Diff Viewer Rust workflow for the eframe/egui desktop diff viewer. Use when editing app.rs, render.rs, main.rs, diff.rs, segment.rs, pipeline.rs, theme.rs, buffer_stamp.rs, bench or visual scripts, Cargo/build configuration, platform build workflows, benchmark-sensitive code, or visual-contract behavior."
---

# Fast Diff Viewer Rust Skill

## Scope

This skill is repository-local. It is for the `eframe` / `egui` desktop diff viewer in this repo, not for generic Rust backend work.

Use it when working on:

- `eframe` and `egui` UI code
- diffing, segmentation, inline emphasis, and render alignment
- theme tokens and palette tests
- the async diff pipeline and frame-budget-sensitive code
- Cargo target configuration, build scripts, and platform CI
- bench and visual-contract validation

Do not paste in generic Tauri, Tokio, SQL, HTTP, web-backend, or broad security-checklist guidance unless the task genuinely introduces those concerns.

## Ground Truth

Read in this order:

1. `README.md`
2. `docs/architecture.md`
3. `docs/performance.md`
4. `docs/benchmark-contract.md`
5. `bench/README.md`
6. `bench/history/` and `scripts/record_benchmark.rs` when preserving or interpreting benchmark evidence
7. `AGENTS.md` for workflow discipline, with behavior/platform details resolved from the files above and the current Cargo/CI configuration

If a requested change conflicts with an architecture invariant, treat it as a product-design decision that needs explicit maintainer review.

## Product Contract

- Rust desktop app built on `eframe 0.37` / `egui 0.37`
- Supported release targets: Windows 10/11 x64, Ubuntu 22.04 x64, Ubuntu 24.04 x64, and macOS 26 arm64
- One release artifact per target platform, containing one native app executable
- Exactly two editable panes in a fixed 50/50 split
- Panes are stock `TextEdit::multiline` widgets
- No persistence: no saved buffers, window position, theme choice, or config
- Async diffing is always on; the UI must not block on recompute
- The app must not intercept pane keyboard, pointer, clipboard, or scroll-wheel events
- Keep platform-specific behavior explicit and target-gated; do not add accidental Windows-only assumptions to shared code
- Scroll sync reads `ScrollAreaOutput.state.clamped_offset()` and feeds the clamped offset into the opposite pane's next `ScrollArea::scroll_offset(...)`
- Diff styling is pane-scoped through `AppTheme`: left rose family, right green family, equal lines transparent

## Code Map

| File                           | Responsibility                                                                                                                                                                 |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/app.rs`                   | `App` and pane state, theme application, pane layout, title bar, gutter/crosshatch integration, diff dispatch/apply, `render_pane`, large-buffer stagger (`PendingRightApply`) |
| `src/main.rs`                  | Entrypoints, module wiring, `NativeOptions`/viewport setup, app launch, window icon, `persist_window: false`, bench-mode vsync setup                                           |
| `src/bench.rs`                 | `#[cfg(feature = "bench")]` harness used by the Rust benchmark runner                                                                                                          |
| `src/buffer_stamp.rs`          | `BufferStamp` and deterministic text hashing for styled-paint identity and layout-cache keys                                                                                   |
| `src/pipeline.rs`              | Latest-wins diff worker on `std::thread` + `mpsc`, queued/in-flight revision discipline                                                                                        |
| `src/segment.rs`               | Single canonical line segmentation function                                                                                                                                    |
| `src/diff.rs`                  | Compare-only tokenization, CRLF normalization, line roles, inline emphasis, perf guards                                                                                        |
| `src/render.rs`                | `build_layout_job`, line and emphasis tinting, gutter markers, neutral fallback, hash helpers                                                                                  |
| `src/theme.rs`                 | `AppTheme`, Graphite/Paper token bytes, palette/chrome pin tests                                                                                                               |
| `Cargo.toml`                   | Target-specific dependencies and release profile settings; keep platform features explicit                                                                                     |
| `build.rs`                     | Windows PE icon resource embedding; must remain a no-op on non-Windows targets                                                                                                 |
| `scripts/bench_runner.rs`      | Repository-native benchmark runner and gate enforcement                                                                                                                        |
| `scripts/record_benchmark.rs`  | Append-only benchmark history recorder for `bench/history/`                                                                                                                    |
| `scripts/visual_contract_*.rs` | Scripted GUI capture/classification helpers for rendering regressions                                                                                                          |

## Non-Negotiable Invariants

1. `segment()` is the only line segmentation entry point. Do not create a second segmentation rule in UI or diff code.
2. `tokenize()` owns compare-only CRLF normalization. Do not strip `\r` anywhere else.
3. `assign_roles_and_emphasis()` is the source of line roles and inline emphasis.
4. `is_styled_paint_valid()` is the single source of truth for styled vs neutral painting. If lengths or buffer stamps disagree, paint neutral-only output with no emphasis. Never paint half-styled or stale-styled.
5. Theme colors live on `AppTheme`. Do not scatter raw diff palette constants through the UI.
6. Left pane uses removed-family colors, right pane uses added-family colors, equal lines stay transparent.
7. `PendingRightApply` exists because two large `TextEdit::multiline` galley rebuilds in one frame blow the budget. Do not remove or bypass it without fresh benchmark evidence.
8. `persist_window: false` is intentional. Any persistence change is architectural, not incidental cleanup.
9. Pane resize, pane swap, custom hotkeys that intercept native input flow, user-facing theme toggles, or editor-surface replacement are product-design changes.
10. Preserve scroll-sync discipline: snapshot the shared offset at frame start, clamp it per pane before `ScrollArea::scroll_offset(...)`, then write the shared offset at most once per frame from the first pane whose captured output differs from the clamped input fed to that pane.

## Repo-Specific Patterns

### egui and styling

- Use `ctx.all_styles_mut`, not `style_mut`, when a style change must survive theme swaps.
- `visuals.text_edit_bg_color = Some(Color32::TRANSPARENT)` is intentional so the crosshatch can show through the editor surface.
- `visuals.selection.stroke` must keep width `0.0` but an opaque text color. Replacing it with `Stroke::NONE` can make selected text disappear.

### Diff and render correctness

- `Role::Changed` is the only case that may paint inline emphasis.
- `Role::AddedOrRemoved` lines tint at the line level only.
- `build_layout_job()` is performance-sensitive. Section count matters.
- Equal-line run consolidation is not cosmetic; it is part of staying within the large-paste budget.
- Stale same-line-count edits must be rejected by `BufferStamp` equality before styled paint.
- Invalid styled input uses transparent per-segment neutral sections; this is visually neutral while avoiding one huge egui word-wrap section.
- Gutter markers mirror styled-paint validity. Stale or misaligned frames paint a blank gutter.

### Concurrency and responsiveness

- The diff worker is a plain `std::thread` with bounded coalescing. Do not casually swap in Tokio or a richer runtime.
- The pipeline guarantees at most one in-flight job and one queued job. Preserve that latest-wins behavior when refactoring.
- `ctx.request_repaint()` calls around pending applies and worker results are part of correctness, not decoration.
- Results carry `BufferStamp`s for the exact text that produced them; do not store borrowed `DiffToken` data in pipeline results or UI state.

### Platform and build configuration

- `Cargo.toml` keeps `eframe` target-specific: Windows and macOS use `clipboard` + `default_fonts` + `glow`; Linux adds `wayland` + `x11`. Keep new platform features explicit instead of widening every target by accident.
- `build.rs` embeds the Windows shell icon through `winresource` only for Windows targets; non-Windows builds must not require Windows resource tooling.
- The custom title bar and `with_decorations(false)` behavior are part of the app contract, but resize/titlebar affordances can vary by platform. Gate OS-specific fixes tightly.
- GitHub workflows are split by OS target for CI and release builds. When touching target dependencies or platform code, check each supported target workflow instead of assuming coverage from one local OS.

## Editing Strategy

### When touching `segment.rs` or `diff.rs`

- Add or adjust unit tests first.
- Preserve CRLF and trailing-newline behavior.
- Keep UTF-8 byte safety in token boundary logic.
- Recheck perf guards if you increase allocations or passes over the buffer.

### When touching `render.rs`

- Guard the neutral fallback first.
- Minimize new `LayoutJob` sections.
- Treat gutter alignment, line tinting, and inline emphasis as one contract, not separate paint features.

### When touching `app.rs`

- Assume `TextEdit::multiline` and `ScrollArea::both()` native behavior is part of the shipped design.
- Be careful with `apply_theme`, title bar code, pane layout, `detect_and_dispatch`, `apply_diff_results`, `render_pane`, and any change that touches frame pacing or repaint behavior.
- Avoid introducing new event interception for the panes.

### When touching `main.rs`

- Keep it focused on entrypoints, module wiring, `NativeOptions`, viewport setup, app launch, and the window icon.
- Preserve `persist_window: false`; persistence is architectural, not incidental cleanup.

### When touching `theme.rs`

- Keep palette and chrome tokens centralized on `AppTheme`.
- Preserve byte-pin tests for shipped themes.
- New themes need their own tests; ad hoc color changes do not.

## Quick Local Validation

Choose the smallest validation set that matches the risk. Quick local
validation is an in-progress confidence check, not CI parity.

- Logic-only change in `segment.rs`, `diff.rs`, or `render.rs`:
  `cargo fmt --all -- --check`; `cargo test --release`
- Theme/token change:
  `cargo fmt --all -- --check`; `cargo test --release`
- Pipeline, layout, or frame-timing change:
  `cargo fmt --all -- --check`; `cargo test --release`;
  `cargo test --release --features bench`
- Hot-path or benchmark-sensitive change:
  above plus `cargo build --release --features bench`;
  `cargo run --release --example bench_runner -- --scenario all`
- Visual behavior change:
  relevant tests plus visual-contract examples or equivalent screenshot-based
  validation; GUI-driving helpers currently require Windows at runtime

## CI-Parity Closeout

Run this before calling Rust implementation work development-complete,
asking for code review, or merging. This is the core repository quality gate;
it is not the benchmark gate. Set `RUSTFLAGS="-D warnings"` using your
shell's environment-variable syntax for the Rust commands, then run:

```shell
pnpm install --frozen-lockfile
pnpm run lint:md
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --release --locked
cargo test --release --locked --features bench
cargo build --release --locked
cargo build --release --locked --features bench
```

When touching `Cargo.toml`, `build.rs`, target-gated code, or workflows,
also check the target CI and release-build workflows under
`.github/workflows/`. Platform coverage is explicit per supported OS target.

## Benchmark Closeout

For hot-path, benchmark-sensitive, or release-signoff work, run the
benchmark gate after the bench-feature build:

```shell
cargo build --release --locked --features bench
cargo run --release --example bench_runner -- --scenario all
```

If CI parity was just run, the bench-feature build is already complete;
run the benchmark command immediately after it. Benchmark closeout is
separate from CI parity because it depends on the reference workstation
contract in `docs/benchmark-contract.md`.

## Bench Discipline

- The benchmark gates are release blockers, not stretch goals.
- The active benchmark contract lives in `docs/benchmark-contract.md`.
- The required repo gate is `cargo run --release --example bench_runner -- --scenario all`.
- Parse CSVs under `bench/results/`, not console output.
- Do not "fix" a perf regression by relaxing thresholds without the documented multi-run evidence process.
- When a bench regression needs a profiler, use `samply` against a
  release-with-debug-info build, not the shipped stripped binary. Add a
  dedicated `profiling` profile to `Cargo.toml` (`inherits = "release"`,
  `debug = "line-tables-only"`, `strip = false`) and build with
  `--profile profiling --features bench` before `samply record`. Use
  `debug = true` only when local-variable inspection is required. Do not
  enable debug info on the `release` profile — it would change the
  shipping artifact and invalidate bench baselines. Source:
  <https://github.com/mstange/samply>.

## Anti-Patterns

- Do not paste in Tauri command patterns, Tokio runtime patterns, SQL or web-backend scaffolding, or generic app-validation frameworks unless the task actually needs them.
- Do not add persistence, settings files, or hidden caches.
- Do not bypass `segment()` or `is_styled_paint_valid()`.
- Do not replace repo-specific tests with generic smoke tests.
- Do not add dependencies to solve problems already handled by std, `egui`, or existing code unless the win is concrete.
- Do not treat this like a CRUD app. It is a frame-budgeted desktop renderer with strict visual contracts.
- Do not eliminate bounds-check panics as a default optimization pass. `get_unchecked` / `unsafe` indexing is a last lever, not a first one. Prefer iterator or slice patterns that let LLVM remove the check, or hoist a single `assert!` above a hot loop. Reach for unchecked access only with benchmark evidence from `cargo run --release --example bench_runner -- --scenario all` showing the check is on the hot path and that iterator/assert rewrites did not close the gap. Rationale: [Rust Performance Book §Bounds Checks](https://nnethercote.github.io/perf-book/bounds-checks.html), [Rust Book ch.13.4](https://doc.rust-lang.org/book/ch13-04-performance.html). Any `unsafe` that does land must ship with a `// SAFETY:` comment stating the invariant.
- Do not propose allocation micro-opts (`Vec::with_capacity` retrofits, reusable buffers, clone elision) on `segment()`, `tokenize()`, `assign_roles_and_emphasis()`, or `build_layout_job()` without a benchmark delta under the repository gate. Large styled Galley rebuilds dominate the per-frame budget; Rust-side allocation wins only matter when they survive that UI-thread ceiling. Rationale: [Rust Performance Book §Heap Allocations](https://nnethercote.github.io/perf-book/heap-allocations.html). See `references/heap-allocations.md`.
- Do not weaken `[profile.release]` in `Cargo.toml` for profiling. `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, `opt-level = 3` are the shipped settings and the bench baseline; flipping any of them produces a binary that does not represent what ships. For symbol-backed profiling, add a sibling `[profile.profiling]` that `inherits = "release"` and only sets `debug = "line-tables-only"` and `strip = false`. Any edit to `[profile.release]` itself requires a fresh bench sweep and a committed benchmark history entry under `bench/history/`. Rationale: [Cargo profiles](https://doc.rust-lang.org/stable/cargo/reference/profiles.html), [Rust Performance Book §Profiling](https://nnethercote.github.io/perf-book/profiling.html). See `references/profiling.md`.
- Do not propose PGO (`-Cprofile-generate` / `-Cprofile-use`) as a response to a failing or tight bench gate. PGO is real and rustc-supported, but it is a final-release experiment gated on settled correctness, a stable bench baseline, and a representative, reproducible training workload — not a fix for a regressed hot path. A PGO binary is a different binary whose numbers are not admissible against the current gates unless the training recipe is committed and the result is paired against release under identical conditions, with committed evidence under `bench/history/` treating it as a shipped-profile change. Rationale: [rustc PGO docs](https://doc.rust-lang.org/rustc/profile-guided-optimization.html), [Rust Performance Book §PGO](https://nnethercote.github.io/perf-book/build-configuration.html#profile-guided-optimization). See `references/pgo.md`.

## References

- `references/repo-architecture.md`
- `references/validation-playbook.md`
- `references/heap-allocations.md`
- `references/profiling.md`
- `references/pgo.md`
