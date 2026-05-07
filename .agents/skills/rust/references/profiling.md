# Profiling the Release Binary Without Weakening the Release Profile

Companion to the profiling anti-pattern in `SKILL.md`. The shipping `[profile.release]` is calibrated against the bench gates. Do not edit it to get symbols for a profiling run — add a sibling profile.

References: [Cargo profiles](https://doc.rust-lang.org/stable/cargo/reference/profiles.html), [Rust Performance Book §Profiling](https://nnethercote.github.io/perf-book/profiling.html).

## The shipping profile is a bench baseline

`Cargo.toml` pins `[profile.release]` at:

- `lto = "thin"`
- `codegen-units = 1`
- `panic = "abort"`
- `strip = true`
- `opt-level = 3`

These settings together shape the numbers in `bench/results/*.csv` and the gates in `SKILL.md` (`cold_start`, `edit_latency`, `large_paste`). Flipping `strip = false`, widening `codegen-units`, or dropping `lto` for "easier profiling" produces a binary that does not represent the shipped artifact, and any bench run under that binary is noise — including a run that happens to look good.

## Use a sibling profiling profile

To profile with symbols, add a profile that inherits from release and only adds debug info. Do not mutate `[profile.release]` itself:

```toml
[profile.profiling]
inherits = "release"
debug = "line-tables-only"  # or `true` for full DWARF if you need locals
strip = false
```

Build and run with `cargo build --profile profiling` / `cargo run --profile profiling`. Artifacts land under `target/profiling/` so they do not shadow `target/release/`.

`line-tables-only` is usually enough for flamegraphs and sample-based profilers, keeps binary size and link time lower than full DWARF, and avoids perturbing inlining decisions more than necessary. Upgrade to `debug = true` only when you need local-variable inspection.

Leave `lto`, `codegen-units`, `panic`, and `opt-level` inherited. Symbols are the only thing you need; optimization shape should stay identical to release so the profile you capture reflects the shipped binary.

## Rules

1. **Never commit a change to `[profile.release]` as part of a profiling task.** If a PR's diff touches those five keys, it is a shipping-profile change, not a profiling change, and it needs a fresh bench sweep plus a committed benchmark history entry under `bench/history/`.
2. **Profiling runs do not satisfy bench gates.** Numbers from `target/profiling/` are not admissible as gate evidence. Gates run against `cargo build --release --features bench` followed by `cargo run --release --example bench_runner -- --scenario all`.
3. **Symbols-only is the default.** Prefer `debug = "line-tables-only"` over `debug = true` unless you have a specific reason to need full DWARF. It keeps the profiling binary closer to release in size and inlining.
4. **Do not disable LTO or raise `codegen-units` to "make the profile more readable."** That produces a different binary. If inlining is hiding the hot frame you care about, annotate the specific function with `#[inline(never)]` locally for the profiling session and revert before committing.
5. **`panic = "abort"` stays.** Switching to `unwind` for profiling changes code-gen around every potential panic site on the hot path. If you need backtraces, set `RUST_BACKTRACE=1` on the abort build; the OS-level crash dump is enough for the cases this repo cares about.

## What this rules out

- Editing `Cargo.toml` to set `strip = false` or `debug = true` on `[profile.release]` for a local profiling session.
- PRs whose only `Cargo.toml` change is loosening `[profile.release]`, with no committed benchmark history entry and no fresh bench sweep.
- Citing profiling-build numbers (flamegraph timings, per-function %) as evidence a bench gate moved.

## What this allows

- Adding `[profile.profiling]` as shown above, using it freely, and not committing the edit when the investigation ends — or committing it as a standing profile if the team decides it is worth keeping (it does not affect the release artifact).
- Local, uncommitted `#[inline(never)]` annotations on a function under investigation, reverted before the PR lands.
- New bench scenarios motivated by something a profiling run revealed — add the scenario under `bench/`, confirm the cost against release, then optimize.
