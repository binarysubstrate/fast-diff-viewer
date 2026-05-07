# Profile-Guided Optimization (PGO) — When and How

Companion to the PGO anti-pattern bullet in `SKILL.md`. PGO is a real, supported rustc feature, but in this repo it is a final-release experiment — not a bench-fix tool, not a refactor substitute, not a first response to a failing gate.

References: [rustc PGO docs](https://doc.rust-lang.org/rustc/profile-guided-optimization.html), [Rust Performance Book §PGO](https://nnethercote.github.io/perf-book/build-configuration.html#profile-guided-optimization), [Cargo profiles](https://doc.rust-lang.org/stable/cargo/reference/profiles.html).

## What PGO actually is

1. Build instrumented binary with `-Cprofile-generate=/path/to/profdir`.
2. Run it against a representative workload so it emits `*.profraw` files.
3. Merge with `llvm-profdata merge` into a single `.profdata`.
4. Rebuild with `-Cprofile-use=/path/to/merged.profdata`.

The final binary's layout, inlining, and branch-prediction hints reflect whatever code paths the training run exercised. Paths that were *not* trained can end up **slower** than the non-PGO baseline.

## Why PGO is not a bench-fix tool in this repo

1. **`[profile.release]` is a calibrated baseline.** `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, `opt-level = 3` shape the numbers in `bench/results/*.csv` and every active threshold in `docs/benchmark-contract.md` (with supporting evidence in committed `bench/history/` entries). A PGO binary is a different binary; its numbers are not admissible against the current `cargo run --release --example bench_runner -- --scenario all` gate unless the PGO step itself is pinned and reproducible.
2. **Training-workload sensitivity.** The shipped app ranges from tiny diffs to ~310 KB styled `Galley` rebuilds. A PGO profile trained on one scenario can regress another. `cold_start`, `edit_latency`, and `large_paste` exercise different code paths — one training run will not equally serve all three.
3. **Reproducibility cost.** For PGO numbers to be credible, the training corpus, the run count, and the merged `.profdata` must all be committed or regeneratable. Otherwise the next person who rebuilds gets a binary with different inlining than the tagged release.
4. **It does not fix correctness or algorithmic regressions.** If a bench gate fails, the cause is almost always a new allocation, a layout-job section explosion, an extra pass over the buffer, or a Galley rebuild. PGO does not address any of those; it reshuffles code the compiler already emitted.

## When PGO is legitimately on the table

All three must hold:

1. **Correctness is settled.** No open visual-contract regressions, no pending changes to `segment()`, `assign_roles_and_emphasis()`, `is_styled_paint_valid()`, or `build_layout_job()`.
2. **Bench is stable.** Current repository gate runs sit comfortably inside gate headroom across at least 10 runs. You are not trying to clear a gate; you are trying to bank headroom.
3. **A representative training workload exists.** At minimum: a cold-start run, a scripted edit sequence exercising the `edit_latency` path, and a `large_paste` scenario. Ideally one the bench runner itself can emit.

If any of those is not true, stop. Fix the underlying thing first.

## If you do run a PGO experiment

1. **Do not mutate `[profile.release]`.** Add a sibling `[profile.release-pgo]` that inherits from release and carries only the PGO flags via `RUSTFLAGS`. Artifacts land under `target/release-pgo/` so they do not shadow `target/release/`.
2. **Commit the training recipe, not just the result.** The PR must show how to regenerate the `.profdata` from a clean checkout. A PGO binary whose profile cannot be reproduced is an unreviewable artifact.
3. **Compare against release under identical conditions.** Same host, same repository gate, same N. Publish both distributions, not just the PGO median.
4. **Do not relax gates to fit PGO numbers.** If PGO helps, the gates get tighter (with the documented multi-run evidence process). If PGO doesn't help enough to justify the build-system complexity, abandon the experiment — do not commit it as "free headroom."
5. **Record it under `bench/history/`.** PGO adoption is a shipped-profile change in everything but name. It needs the same evidence bar as an edit to `[profile.release]`.

## What this rules out

- Proposing PGO as a response to a failing or tight bench gate.
- Committing a PGO training recipe that is not runnable from a clean checkout.
- Citing PGO-build numbers against non-PGO gates (or vice versa).
- Treating PGO as a substitute for fixing a real allocation, section-count, or Galley-rebuild regression.

## What this allows

- Local, uncommitted PGO experiments to answer "is there headroom to bank?" after correctness and bench stability are settled.
- A proposal to adopt PGO as the shipped build, if it carries: reproducible training recipe, paired bench distributions under identical conditions, and a committed benchmark history entry treating it as a shipped-profile change.
- Using PGO findings to motivate targeted, non-PGO optimizations (e.g., "PGO reshuffled X so it inlines; let's mark the non-PGO version `#[inline]` and check the delta").
