# AGENTS.md - Fast Diff Viewer

Repository notes for automated coding assistants and maintainers.

## Ground Truth

Use these docs as task-scoped sources of truth.

- `docs/architecture.md` for product contract, runtime data flow, and
   invariants.
- `CONTRIBUTING.md` for local validation commands and CI expectations.
- `docs/benchmark-contract.md` for active benchmark gates.
- `bench/README.md` for benchmark mechanics and history workflow.
- `docs/release.md` for release workflow and package contents.

## Implementation Invariants

- Keep the app Rust-only with one native app executable per supported target.
- Supported release targets are Windows 10/11 x64, Ubuntu 22.04 x64,
  Ubuntu 24.04 x64, and macOS 26 arm64.
- Keep exactly two editable panes in a fixed 50/50 split.
- Do not add persistence for window state, buffers, settings, or caches.
- Do not intercept pane keyboard, pointer, clipboard, or scroll-wheel input.
- Keep diff coloring pane-scoped: left rose family, right green family,
  equal lines transparent.
- Use `segment()` as the single line-segmentation entry point.
- Keep CRLF normalization compare-only in `diff::tokenize()`.
- Paint styled output only through `is_styled_paint_valid()`. Stale or
  misaligned inputs must fall back to neutral paint.
- Preserve the latest-wins diff pipeline: at most one in-flight job and one
  queued job.
- Preserve the large-buffer right-pane stagger unless fresh benchmark
  evidence supports a change.

## Validation

CI parity. Set `RUSTFLAGS="-D warnings"` using your shell's
environment-variable syntax, then run:

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

Required benchmark gate on the reference workstation:

```shell
cargo run --release --example bench_runner -- --scenario all
```

After a benchmark run that should be preserved, record it under
`bench/history/`:

```shell
cargo run --release --example record_benchmark -- --release-gate-label --use-existing-results
```

## Git Discipline

- Keep commits logical and focused.
- Do not use `--no-verify`, force-push, or amend unless explicitly asked.
- Work with user changes already present in the tree; do not reset or revert
  unrelated work.
