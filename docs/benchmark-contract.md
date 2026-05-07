# Fast Diff Viewer Benchmark Contract

This file is the authoritative source for the active benchmark gates.
Scenario mechanics, fixtures, benchmark history, and threshold rationale live
in [`bench/README.md`](../bench/README.md).

## Current Policy

- Required repository gate:
  `cargo run --release --example bench_runner -- --scenario all`
- The runner enforces one repository default threshold row.
- Benchmark gates require the reference workstation and a clean shell.

## Active Gates

| Scenario | Trials / Probes | Gate |
| --- | --- | --- |
| `cold_start` | 10 trials | median < 500 ms AND worst < 750 ms |
| `edit_latency` | 3 passes x 60 probes (pooled) | pooled median <= 42 ms AND pooled p95 <= 65 ms |
| `large_paste` | 1 trial x 120 frames | every `frame_us` <= 160 ms AND `settle_ms` < 500 ms |

## Gate Selection Evidence

The runner prints `gates: repository default` at start of run to identify the
single enforced contract row.

## Related Records

- [`bench/README.md`](../bench/README.md) - scenario mechanics, runner usage,
  benchmark history, and threshold rationale
- [`bench/history/`](../bench/history/) - committed benchmark evidence
