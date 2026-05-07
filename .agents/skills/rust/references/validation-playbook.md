# Fast Diff Viewer Validation Playbook

## Quick Local Validation

Run the smallest useful set first. Quick local validation is an
in-progress confidence check, not CI parity:

```shell
cargo fmt --all -- --check
cargo test --release
```

Use this for pure logic changes in `src/segment.rs`, `src/diff.rs`, `src/render.rs`, or `src/theme.rs` when the edit does not alter frame pacing.

For changes that can affect layout work, repaint timing, or the diff
pipeline, add:

```shell
cargo test --release --features bench
```

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

When touching `Cargo.toml`, `build.rs`, target-gated code, or CI workflow
files, also check the target CI and release-build workflows under
`.github/workflows/`. Platform coverage is explicit per supported OS target,
so do not infer coverage from one local OS.

## Benchmark Closeout

For hot-path, benchmark-sensitive, or release-signoff work, run the
benchmark gate after the bench-feature build:

```shell
cargo build --release --locked --features bench
cargo run --release --example bench_runner -- --scenario all
```

Examples:

- `build_layout_job()` logic or section counts
- `render_pane()` or gutter painting
- `apply_diff_results()` or `PendingRightApply`
- pipeline scheduling, revision handling, or repaint timing
- anything that can increase galley rebuild frequency or cost

If CI parity was just run, the bench-feature build is already complete;
run the benchmark command immediately after it.

## Benchmark Gates

The current release-blocking contract lives in
`docs/benchmark-contract.md`.

- Required repo gate: `cargo run --release --example bench_runner -- --scenario all`

Do not change thresholds casually. Threshold changes require multi-run evidence and documented rationale, not optimism.

## Bench Runner Gotchas

- Use the repository-native Rust runner: `cargo run --release --example bench_runner`.
- Parse CSVs under `bench/results/`, not console output.
- If you need to propose a new threshold, use the repo's documented distribution-based process rather than a single lucky run.

## Visual-Contract Validation

When rendered output changes, tests may not be enough.

Prefer one of these:

- Run the `scripts/visual_contract_*.rs` Cargo examples
- Capture before and after screenshots with clear case IDs
- Record whether the change is logical, visual, or both

The GUI-driving visual helpers (`capture_app_screenshot`,
`visual_contract_drive_case`, and `visual_contract_run_all`) currently use
Win32 APIs for window discovery, input, clipboard, and capture. They compile
on non-Windows targets but return a clear unsupported-platform error there;
use equivalent manual screenshots or analyze existing captures on other
platforms.

Use the visual-contract suite especially when a change touches:

- line tinting
- inline emphasis
- gutter markers
- pane chrome
- crosshatch, title bar, or other layered paint behavior

## Usually Safe to Skip Full Benches

You can usually skip the benchmark runner for:

- doc-only edits
- isolated markdown or backlog changes
- color-byte swaps in `src/theme.rs` that do not affect geometry, section counts, or paint complexity
- dead-code cleanup with no runtime effect

If you are unsure whether a change affects frame cost, assume it does until you prove otherwise.

## Practical Review Checklist

- Did the change preserve the no-persistence contract?
- Did it preserve fixed two-pane behavior?
- Did it keep `segment()` as the only segmentation path?
- Did it keep styled rendering aligned through `is_styled_paint_valid()`?
- Did it avoid adding new runtime or dependency complexity without a measurable win?
- Did it run the right level of validation for the risk?
