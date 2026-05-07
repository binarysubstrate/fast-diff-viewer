# Heap Allocations in the Diff Pipeline

Companion to the bounds-checks anti-pattern in `SKILL.md`. Same shape: the Rust Performance Book's guidance is directionally correct for this repo, but any change to the hot surfaces below must ship with a benchmark delta, not a book citation.

Reference: [Rust Performance Book §Heap Allocations](https://nnethercote.github.io/perf-book/heap-allocations.html).

## Hot surfaces

These functions concentrate the diff/render pipeline's allocation pressure:

- `src/segment.rs::segment` — per-buffer line split. Pre-allocates via a newline count pre-pass; exemplar of the `Vec::with_capacity` pattern in this repo.
- `src/diff.rs::tokenize` — creates borrowed `DiffToken`s from canonical segments. Tokens must stay scoped to the worker pass and must not be stored in pipeline results or UI state.
- `src/diff.rs::assign_roles_and_emphasis` — four per-call vector allocations (roles x2, emphasis x2), no reusable buffer. Runs on every edit that changes either pane.
- `src/render.rs::build_layout_job` — styled `LayoutJob` assembly. Per-line `TextFormat` clones in the inner loop. Documented hot path for the `large_paste` and `edit_latency` bench gates.

## Rules

1. **Bench evidence beats book citation.** A large styled `TextEdit::multiline` Galley rebuild can dominate the UI frame budget. A Rust-side allocation micro-opt is only meaningful when the delta survives that ceiling and shows up in `bench/results/*.csv` under `cargo run --release --example bench_runner -- --scenario all`. A PR that cites §Heap Allocations without a bench delta should be rejected.

2. **`Vec::with_capacity` whenever final length is O(n) or cheaper to compute.** `segment()` is the template: a single pre-pass counts `\n`, then the real pass fills a correctly-sized vector. Do not pay for a pre-pass if the size is already available from the caller.

3. **Reusable buffers require a measurable fresh-alloc cost.** Before introducing a thread-local, a field on a struct, or an `&mut Vec` parameter to avoid allocation, demonstrate the fresh-alloc cost is above bench noise on the reference workstation. Lifetime and aliasing complexity are not free; they need a payoff.

4. **`TextFormat.clone()` in `build_layout_job` is structurally required today.** egui's `LayoutJob::append(_, _, TextFormat)` consumes the format by value. Cloning per section is not churn to eliminate — it is the API contract. Do not propose threading references through this path without an upstream egui API change.

5. **Equal-line run consolidation is the real render-path win.** `build_layout_job` already coalesces consecutive `Role::Equal` segments into a single `LayoutJob` section (dropping ~21k sections to ~1.5k on a 10k-line 5%-divergence buffer). That is the allocation/section-count win that survives the Galley ceiling. Any further render-path proposal should be compared against that baseline, not against the naive per-segment version.

## What this rules out

- "Replace `Vec::new()` with `Vec::with_capacity(0)`" cleanups — not a win.
- "Pool the four vectors in `assign_roles_and_emphasis`" without bench evidence — complexity without payoff until the diff worker's allocator shows up on a flamegraph.
- "Remove `TextFormat.clone()` from `build_layout_job`" — structurally blocked by egui's API.
- "Cache borrowed `DiffToken`s across jobs" — invalid ownership shape; cache by `BufferStamp` only if a future measured design stores owned data and pays for the invalidation complexity.
- Any PR whose motivation section is a Perf Book link and whose evidence section is empty.

## What this allows

- Sizing a new `Vec` via `with_capacity` when the length is known or cheaply computable.
- Replacing a `.to_vec()` / `.clone()` that is provably dead (the clone's result is never mutated and the source outlives the consumer) — still with a benchmark delta on the hot surfaces.
- New benchmark scenarios that expose an allocation hot spot the current gates miss. Add the scenario first, show the cost, then optimize.
