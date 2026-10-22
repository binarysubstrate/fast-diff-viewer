# 2026-10-22_a3d1800_v1.1.1-release-approved-exception

- Date: 2026-10-22
- Command: `cargo run --release --locked --example bench_runner -- --scenario all`
- Runs: 1
- Source: `a3d1800bca124cce8f40c2bdfcb5f3aa1d7433da`
- Disposition: maintainer-approved release exception; benchmark gate failed.

Large-paste frame 1 took **168,316 us**, exceeding the unchanged
**160,000 us** limit. All other gates passed. After reviewing the result,
the maintainer approved releasing v1.1.1 and reported background desktop
activity. This record does not claim a passing gate or an idle reference
workload. The original CSVs are retained intact, and no benchmark threshold,
workload, injection timing or machine setting was changed.

## Aggregate Metrics

| Metric | Value |
| --- | --- |
| Cold-start median of run medians | 232.490 ms |
| Cold-start worst across runs | 280.689 ms |
| Edit-latency median of run medians | 11,730 us |
| Edit-latency worst p95 across runs | 18,381 us |
| Large-paste worst frame across runs | 168,316 us |
| Large-paste median settle | 380 ms |

Machine posture, exact source and executable hashes, run timestamps and
separate native visual validation are recorded in `metadata.json`.
