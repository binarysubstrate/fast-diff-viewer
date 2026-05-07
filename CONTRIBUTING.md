# Contributing

Contributions are welcome. Issues and pull requests are both good ways to
help.

Fast Diff Viewer should stay simple and fast.

## Local Validation

Build, test, and benchmark commands assume they are run from the repo root. Use
your shell's environment-variable syntax when setting `RUSTFLAGS` for CI-parity
checks.

PowerShell:

```powershell
$env:RUSTFLAGS = "-D warnings"
```

POSIX shells:

```shell
export RUSTFLAGS="-D warnings"
```

```shell
# Install repo-local JavaScript tooling for Markdown linting.
pnpm install --frozen-lockfile

# Lint Markdown.
pnpm run lint:md

# Check formatting.
cargo fmt --all -- --check

# Run clippy with warnings denied.
cargo clippy --locked --all-targets --all-features -- -D warnings

# Run the test feature configurations enforced in CI.
cargo test --release --locked
cargo test --release --locked --features bench

# Build the native shipping binary for the current target.
cargo build --release --locked
cargo build --release --locked --features bench
```

Documentation-only changes should pass the Markdown lint check:

```shell
pnpm run lint:md
```

## Benchmarks

Pull requests that affect runtime behavior should pass the performance gate
documented in [`docs/benchmark-contract.md`](docs/benchmark-contract.md):

```shell
cargo run --release --example bench_runner -- --scenario all
```

Benchmark gates require the reference workstation and a clean shell, so they
are intentionally not part of CI. The default repository gate is
`cargo run --release --example bench_runner -- --scenario all`.

Benchmark mechanics and history workflow live in
[`bench/README.md`](bench/README.md).

## CI

Quality gates run on every pull request through separate target workflows for
Windows, Ubuntu 22.04, Ubuntu 24.04, and macOS 26. Each target runs fmt, clippy
with warnings denied, release-mode tests with and without the bench feature,
and locked release builds with and without the bench feature. Markdown linting
runs through a separate `markdownlint-cli2` workflow using the repo-local
`pnpm-lock.yaml`.

Tagged releases run the target CI workflows before publishing packages. Release
workflow and package contents are documented in
[`docs/release.md`](docs/release.md).

## Pull Requests

Changes that alter product constraints should line up with
[`docs/architecture.md`](docs/architecture.md) and the active benchmark gates.

## Roadmap

Improved diff quality is a non-trivial but welcome future direction.
