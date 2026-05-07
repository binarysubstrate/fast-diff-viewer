# 2026-05-04_431c258_post-pbi-final-verification-3run

- Date: 2026-05-04
- Command: `cargo run --release --example bench_runner -- --scenario all --out-dir bench/results --skip-rammap`
- Runs: 3

## Aggregate Metrics

| Metric | Value |
| --- | --- |
| Cold-start median of run medians | 370.243 ms |
| Cold-start worst across runs | 592.300 ms |
| Edit-latency median of run medians | 22,134 us |
| Edit-latency worst p95 across runs | 60,590 us |
| Large-paste worst frame across runs | 106,870 us |
| Large-paste median settle | 276 ms |
