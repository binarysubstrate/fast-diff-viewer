# Fast Diff Viewer Benchmarks

This directory holds deterministic fixtures, the benchmark runner, scratch
CSV output, and committed benchmark history.

## Measurement Boundary

The measured boundary is app-controlled, not GPU presentation:

- `cold_start` captures `T1` at the end of the first `App::ui()` that
  paints both text panes.
- `edit_latency` resolves a probe at the end of `App::ui()` once both
  panes have painted the revision created by the injected edit.
- `large_paste` records `App::ui()` wall time for 120 frames after the
  paste and records settle time when the paste revision first paints styled.

Display, compositor, and monitor-sync costs are real but intentionally outside
these release gates.

## Reference Posture

The required repository gate is the single default row documented in
[`docs/benchmark-contract.md`](../docs/benchmark-contract.md). The
repository-native runner is `cargo run --release --example bench_runner`.

## Fixtures

Committed fixture files are byte-identical outputs of the SplitMix64-seeded
generator in `scripts/gen_fixtures.rs`:

| File | Lines | Seed | Divergence |
| --- | --- | --- | --- |
| `bench/2k_lines_a.txt` | 2000 | `0x2BADF00DDEADBEEF` | none |
| `bench/2k_lines_b.txt` | 2000 | `0x2BADF00DDEADBEEF` | every 20th |
| `bench/10k_lines_a.txt` | 10000 | `0xC0DECAFE20260419` | none |
| `bench/10k_lines_b.txt` | 10000 | `0xC0DECAFE20260419` | every 20th |

Regenerate with:

```shell
cargo run --example gen_fixtures
```

## CSV Output

The bench binary writes scratch CSVs to `bench/results/`; this directory is
ignored and should be treated as the latest local run only.

- `cold_start.csv` - `trial_id,t0_ns,t1_ns,ms`
- `edit_latency.csv` - `probe_id,revision,t0_ns,t1_ns,latency_us`
- `large_paste.csv` - `frame_id,frame_us`
- `large_paste_settle.csv` - `t_paste_ns,t_applied_ns,settle_ms`

## Running The Benchmarks

From the repo root:

```shell
cargo build --release --features bench

cargo run --release --example bench_runner -- --scenario cold_start
cargo run --release --example bench_runner -- --scenario edit_latency
cargo run --release --example bench_runner -- --scenario large_paste
cargo run --release --example bench_runner -- --scenario all
```

The runner launches the platform release binary, such as
`target/release/fast-diff-viewer.exe` on Windows or
`target/release/fast-diff-viewer` on Unix-like targets, with `--bench <name>
--trial <n> --out <path>`. It aggregates the emitted CSV rows, prints the
repository default gate at start of run, and exits nonzero if a gate is
missed.

Forced-failure sanity check:

```shell
cargo run --release --example bench_runner -- --scenario cold_start --cold-start-median-ms 1
```

## Benchmark History

Committed benchmark evidence lives under `bench/history/`:

```text
bench/history/
  YYYY-MM-DD_<short-sha>_<label>/
    metadata.json
    summary.md
    run-1/
      cold_start.csv
      edit_latency.csv
      large_paste.csv
      large_paste_settle.csv
```

Some migrated legacy entries contain additional edit-latency-only runs. Tools
skip sparse runs for metrics whose CSV is absent and fail with a clear error
when no run in an entry contains the requested metric.

Record a new history entry from the current `bench/results` directory:

```shell
cargo run --release --example record_benchmark -- --release-gate-label --use-existing-results
```

Run and record several benchmark batches:

```shell
cargo run --release --example record_benchmark -- --label release-candidate --runs 3
```

## Plotting

The plotter reads both direct `run-*` folders and committed history entries.

```shell
poetry install

# Default comparison between the migrated app-shell baseline entries.
poetry run python scripts/benchmark-plotter/cli.py

# Compare two history entries.
poetry run python scripts/benchmark-plotter/cli.py --compare 2026-04-20_fe62e84_pre-app-shell-extraction 2026-04-20_fe62e84_post-app-shell-extraction

# Plot every history entry chronologically.
poetry run python scripts/benchmark-plotter/cli.py --timeline --metric all
```

Generated plots under `bench/history/*.png` are ignored unless explicitly
added for a release note or investigation.

## Threshold Rationale

The active contract is intentionally centralized in
[`docs/benchmark-contract.md`](../docs/benchmark-contract.md), including the
current gate values.

The large-paste frame gate reflects the cost of rebuilding a styled egui
galley for a 10k-line buffer on the reference workstation. Tightening that
gate requires fresh benchmark evidence or a material renderer change.
