# Scripts

Repository-local automation and helper utilities live under `scripts/`.

## Rust Generators

`gen_fixtures.rs` and `gen_icon.rs` are Cargo examples whose sources live in
this directory.

Regenerate benchmark fixtures:

```shell
cargo run --example gen_fixtures
```

Regenerate icon artifacts:

```shell
cargo run --example gen_icon --release
```

Generated icon artifacts are committed so everyday release builds do not need
to run the rasterizer.

## Benchmark Recording

`record_benchmark.rs` is the Cargo example that creates append-only
benchmark-history entries under `bench/history/`.

Record the latest `bench/results` CSVs:

```shell
cargo run --release --example record_benchmark -- --label v1.0.0-release-gate --use-existing-results
```

Run and record multiple batches:

```shell
cargo run --release --example record_benchmark -- --label release-candidate --runs 3
```

## Benchmark Plotter

`benchmark-plotter/cli.py` plots direct `run-*` folders or committed
benchmark history entries.

```shell
poetry install

poetry run python scripts/benchmark-plotter/cli.py
poetry run python scripts/benchmark-plotter/cli.py --timeline --metric all
poetry run python scripts/benchmark-plotter/cli.py --compare 2026-04-20_fe62e84_pre-app-shell-extraction 2026-04-20_fe62e84_post-app-shell-extraction
```

Validation:

```shell
poetry run python -m flake8 --config .flake8 scripts/benchmark-plotter/cli.py
poetry run python -m pyright --project pyrightconfig.json scripts/benchmark-plotter/cli.py
```

## Visual Capture

The Rust visual helpers provide scripted Windows screenshots and
visual-contract diagnostics for local visual checks. They launch the release
binary when needed, drive pane content, capture the app window, and close it.

```shell
cargo build --release
cargo run --release --example capture_app_screenshot -- --output-path docs/visual-contract/smoke.png
```

Analyze a captured visual-contract screenshot:

```shell
cargo run --release --example visual_contract_analyze -- --case A1 --in-dir docs/visual-contract
cargo run --release --example visual_contract_classify -- --case A1 --in-dir docs/visual-contract
cargo run --release --example visual_contract_drive_case -- --case A1 --left "alpha beta" --right "alpha beta" --out-dir docs/visual-contract --suffix rust-smoke
cargo run --release --example visual_contract_probe -- --case A1 --in-dir docs/visual-contract
cargo run --release --example visual_contract_run_all -- --out-dir docs/visual-contract --suffix rust-smoke --only A1
```
