//! Rust benchmark gate runner for Fast Diff Viewer.
//!
//! This launches the bench-enabled release binary, aggregates CSVs, and exits
//! nonzero when any repository benchmark gate is missed.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example bench_runner -- --scenario all
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const COLD_START_TRIALS: u32 = 10;
const EDIT_LATENCY_PASSES: u32 = 3;
const LARGE_PASTE_FRAMES: usize = 120;

const EDIT_LATENCY_MEDIAN_US: f64 = 42_000.0;
const EDIT_LATENCY_P95_US: f64 = 65_000.0;

type RunnerResult<T> = Result<T, RunnerError>;

#[derive(Debug)]
struct RunnerError {
    message: String,
}

impl RunnerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for RunnerError {}

impl From<std::io::Error> for RunnerError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<std::num::ParseFloatError> for RunnerError {
    fn from(value: std::num::ParseFloatError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    ColdStart,
    EditLatency,
    LargePaste,
    All,
}

impl Scenario {
    fn parse(value: &str) -> RunnerResult<Self> {
        match value {
            "cold_start" => Ok(Self::ColdStart),
            "edit_latency" => Ok(Self::EditLatency),
            "large_paste" => Ok(Self::LargePaste),
            "all" => Ok(Self::All),
            _ => Err(RunnerError::new(format!(
                "invalid scenario '{value}'; expected cold_start, edit_latency, large_paste, or all"
            ))),
        }
    }

    fn as_bench_arg(self) -> &'static str {
        match self {
            Self::ColdStart => "cold_start",
            Self::EditLatency => "edit_latency",
            Self::LargePaste => "large_paste",
            Self::All => "all",
        }
    }
}

#[derive(Debug)]
struct Config {
    scenario: Option<Scenario>,
    out_dir: PathBuf,
    exe: PathBuf,
    cold_start_median_ms: f64,
    cold_start_worst_ms: f64,
    edit_latency_p95_us: f64,
    edit_latency_median_us: f64,
    large_paste_frame_max_us: f64,
    large_paste_settle_ms: f64,
}

impl Default for Config {
    fn default() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            scenario: None,
            out_dir: manifest_dir.join("bench").join("results"),
            exe: manifest_dir
                .join("target")
                .join("release")
                .join(exe_name("fast-diff-viewer")),
            cold_start_median_ms: 500.0,
            cold_start_worst_ms: 750.0,
            edit_latency_p95_us: EDIT_LATENCY_P95_US,
            edit_latency_median_us: EDIT_LATENCY_MEDIAN_US,
            large_paste_frame_max_us: 160_000.0,
            large_paste_settle_ms: 500.0,
        }
    }
}

impl Config {
    fn from_args<I>(args: I) -> RunnerResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut config = Self::default();
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(RunnerError::new("help requested"));
                }
                "--scenario" => {
                    let value = parser.next_string("--scenario")?;
                    config.scenario = Some(Scenario::parse(&value)?);
                }
                "--out-dir" | "--out" => {
                    config.out_dir = parser.next_path(arg.as_ref())?;
                }
                "--exe" => {
                    config.exe = parser.next_path("--exe")?;
                }
                "--cold-start-median-ms" => {
                    config.cold_start_median_ms = parser.next_f64("--cold-start-median-ms")?;
                }
                "--cold-start-worst-ms" => {
                    config.cold_start_worst_ms = parser.next_f64("--cold-start-worst-ms")?;
                }
                "--edit-latency-p95-us" => {
                    config.edit_latency_p95_us = parser.next_f64("--edit-latency-p95-us")?;
                }
                "--edit-latency-median-us" => {
                    config.edit_latency_median_us = parser.next_f64("--edit-latency-median-us")?;
                }
                "--large-paste-frame-max-us" => {
                    config.large_paste_frame_max_us =
                        parser.next_f64("--large-paste-frame-max-us")?;
                }
                "--large-paste-settle-ms" => {
                    config.large_paste_settle_ms = parser.next_f64("--large-paste-settle-ms")?;
                }
                _ if arg.starts_with("--scenario=") => {
                    let value = split_inline_value(&arg, "--scenario=")?;
                    config.scenario = Some(Scenario::parse(value)?);
                }
                _ if arg.starts_with("--out-dir=") => {
                    config.out_dir = PathBuf::from(split_inline_value(&arg, "--out-dir=")?);
                }
                _ if arg.starts_with("--out=") => {
                    config.out_dir = PathBuf::from(split_inline_value(&arg, "--out=")?);
                }
                _ if arg.starts_with("--exe=") => {
                    config.exe = PathBuf::from(split_inline_value(&arg, "--exe=")?);
                }
                _ if arg.starts_with("--cold-start-median-ms=") => {
                    config.cold_start_median_ms =
                        split_inline_value(&arg, "--cold-start-median-ms=")?.parse()?;
                }
                _ if arg.starts_with("--cold-start-worst-ms=") => {
                    config.cold_start_worst_ms =
                        split_inline_value(&arg, "--cold-start-worst-ms=")?.parse()?;
                }
                _ if arg.starts_with("--edit-latency-p95-us=") => {
                    config.edit_latency_p95_us =
                        split_inline_value(&arg, "--edit-latency-p95-us=")?.parse()?;
                }
                _ if arg.starts_with("--edit-latency-median-us=") => {
                    config.edit_latency_median_us =
                        split_inline_value(&arg, "--edit-latency-median-us=")?.parse()?;
                }
                _ if arg.starts_with("--large-paste-frame-max-us=") => {
                    config.large_paste_frame_max_us =
                        split_inline_value(&arg, "--large-paste-frame-max-us=")?.parse()?;
                }
                _ if arg.starts_with("--large-paste-settle-ms=") => {
                    config.large_paste_settle_ms =
                        split_inline_value(&arg, "--large-paste-settle-ms=")?.parse()?;
                }
                _ => {
                    return Err(RunnerError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        if config.scenario.is_none() {
            return Err(RunnerError::new(
                "missing required --scenario <cold_start|edit_latency|large_paste|all>",
            ));
        }

        Ok(config)
    }
}

#[derive(Debug)]
struct ArgParser {
    args: Vec<OsString>,
    index: usize,
}

impl ArgParser {
    fn new(args: Vec<OsString>) -> Self {
        Self { args, index: 0 }
    }

    fn next_arg(&mut self) -> Option<OsString> {
        let arg = self.args.get(self.index).cloned();
        self.index += usize::from(arg.is_some());
        arg
    }

    fn next_path(&mut self, name: &str) -> RunnerResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| RunnerError::new(format!("missing value for {name}")))
    }

    fn next_string(&mut self, name: &str) -> RunnerResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| RunnerError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| RunnerError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_f64(&mut self, name: &str) -> RunnerResult<f64> {
        self.next_string(name)?
            .parse()
            .map_err(|err| RunnerError::new(format!("invalid numeric value for {name}: {err}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> RunnerResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| RunnerError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(RunnerError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn print_usage() {
    let default_exe = PathBuf::from("target")
        .join("release")
        .join(exe_name("fast-diff-viewer"));
    println!(
        "Usage: cargo run --release --example bench_runner -- \\
  --scenario <cold_start|edit_latency|large_paste|all> [options]\n\
\n\
Options:\n\
  --out-dir <path>                    Results directory (default: bench/results)\n\
  --exe <path>                        Bench binary (default: {})\n\
  --cold-start-median-ms <n>          Default: 500\n\
  --cold-start-worst-ms <n>           Default: 750\n\
  --edit-latency-median-us <n>        Default: 42000\n\
  --edit-latency-p95-us <n>           Default: 65000\n\
  --large-paste-frame-max-us <n>      Default: 160000\n\
  --large-paste-settle-ms <n>         Default: 500",
        default_exe.display()
    );
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) if err.message == "help requested" => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> RunnerResult<bool> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let exe_path = resolve_exe(&config.exe)?;
    fs::create_dir_all(&config.out_dir)?;

    println!("gates: repository default");

    let mut failures = Vec::new();
    let scenario = config
        .scenario
        .ok_or_else(|| RunnerError::new("missing required --scenario"))?;
    match scenario {
        Scenario::ColdStart => invoke_cold_start(&config, &exe_path, &mut failures)?,
        Scenario::EditLatency => {
            invoke_edit_latency(&config, &exe_path, &mut failures)?;
        }
        Scenario::LargePaste => invoke_large_paste(&config, &exe_path, &mut failures)?,
        Scenario::All => {
            invoke_cold_start(&config, &exe_path, &mut failures)?;
            invoke_edit_latency(&config, &exe_path, &mut failures)?;
            invoke_large_paste(&config, &exe_path, &mut failures)?;
        }
    }

    if failures.is_empty() {
        println!();
        println!("All gates passed.");
        Ok(true)
    } else {
        println!();
        println!("GATE FAILURES:");
        for failure in failures {
            println!("  - {failure}");
        }
        Ok(false)
    }
}

fn resolve_exe(path: &Path) -> RunnerResult<PathBuf> {
    if !path.exists() {
        return Err(RunnerError::new(format!(
            "Bench binary not found at '{}'. Build it with: cargo build --release --features bench",
            path.display()
        )));
    }
    fs::canonicalize(path).map_err(|err| {
        RunnerError::new(format!(
            "failed to resolve bench binary '{}': {err}",
            path.display()
        ))
    })
}

fn invoke_trial(
    config: &Config,
    exe_path: &Path,
    scenario: Scenario,
    trial: u32,
) -> RunnerResult<()> {
    let status = Command::new(exe_path)
        .arg("--bench")
        .arg(scenario.as_bench_arg())
        .arg("--trial")
        .arg(trial.to_string())
        .arg("--out")
        .arg(&config.out_dir)
        .status()
        .map_err(|err| {
            RunnerError::new(format!(
                "failed to launch bench binary for {} trial {trial}: {err}",
                scenario.as_bench_arg()
            ))
        })?;

    if !status.success() {
        return Err(RunnerError::new(format!(
            "Bench binary exited {} on scenario {} trial {trial}",
            status.code().unwrap_or(-1),
            scenario.as_bench_arg()
        )));
    }

    Ok(())
}

fn invoke_cold_start(
    config: &Config,
    exe_path: &Path,
    failures: &mut Vec<String>,
) -> RunnerResult<()> {
    let csv = config.out_dir.join("cold_start.csv");
    clear_csv(&csv)?;
    for trial in 0..COLD_START_TRIALS {
        invoke_trial(config, exe_path, Scenario::ColdStart, trial)?;
    }

    let values = read_csv_column_f64(&csv, "ms")?;
    let median = median(&values);
    let worst = values.iter().copied().reduce(f64::max).unwrap_or(0.0);

    println!(
        "cold_start: {} trials, median={:.3} ms, worst={:.3} ms",
        values.len(),
        median,
        worst
    );
    if median >= config.cold_start_median_ms {
        failures.push(format!(
            "cold_start median {:.3} ms >= {} ms",
            median, config.cold_start_median_ms
        ));
    }
    if worst >= config.cold_start_worst_ms {
        failures.push(format!(
            "cold_start worst {:.3} ms >= {} ms",
            worst, config.cold_start_worst_ms
        ));
    }
    Ok(())
}

fn invoke_edit_latency(
    config: &Config,
    exe_path: &Path,
    failures: &mut Vec<String>,
) -> RunnerResult<()> {
    let csv = config.out_dir.join("edit_latency.csv");
    clear_csv(&csv)?;
    for trial in 0..EDIT_LATENCY_PASSES {
        invoke_trial(config, exe_path, Scenario::EditLatency, trial)?;
    }

    let values = read_csv_column_f64(&csv, "latency_us")?;
    let median = median(&values);
    let p95 = percentile_nearest_rank(&values, 95.0);

    println!(
        "edit_latency: {} probes across {} passes, median={} us, p95={} us",
        values.len(),
        EDIT_LATENCY_PASSES,
        format_integer(median),
        format_integer(p95)
    );
    if median > config.edit_latency_median_us {
        failures.push(format!(
            "edit_latency median {} us > {} us",
            format_integer(median),
            config.edit_latency_median_us
        ));
    }
    if p95 > config.edit_latency_p95_us {
        failures.push(format!(
            "edit_latency p95 {} us > {} us",
            format_integer(p95),
            config.edit_latency_p95_us
        ));
    }
    Ok(())
}

fn invoke_large_paste(
    config: &Config,
    exe_path: &Path,
    failures: &mut Vec<String>,
) -> RunnerResult<()> {
    let frames_csv = config.out_dir.join("large_paste.csv");
    let settle_csv = config.out_dir.join("large_paste_settle.csv");
    clear_csv(&frames_csv)?;
    clear_csv(&settle_csv)?;
    invoke_trial(config, exe_path, Scenario::LargePaste, 0)?;

    let frame_values = read_csv_column_f64(&frames_csv, "frame_us")?;
    let worst_frame_us = frame_values.iter().copied().reduce(f64::max).unwrap_or(0.0);
    let settle_ms = read_csv_first_f64(&settle_csv, "settle_ms")?;

    println!(
        "large_paste: {} frames, worst_frame={} us, settle={} ms",
        frame_values.len(),
        format_integer(worst_frame_us),
        format_integer(settle_ms)
    );
    if frame_values.len() != LARGE_PASTE_FRAMES {
        failures.push(format!(
            "large_paste frame count {} != {}",
            frame_values.len(),
            LARGE_PASTE_FRAMES
        ));
    }
    if worst_frame_us > config.large_paste_frame_max_us {
        failures.push(format!(
            "large_paste worst frame {} us > {} us",
            format_integer(worst_frame_us),
            config.large_paste_frame_max_us
        ));
    }
    if settle_ms >= config.large_paste_settle_ms {
        failures.push(format!(
            "large_paste settle {} ms >= {} ms",
            format_integer(settle_ms),
            config.large_paste_settle_ms
        ));
    }
    Ok(())
}

fn clear_csv(path: &Path) -> RunnerResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(RunnerError::new(format!(
            "failed to remove '{}': {err}",
            path.display()
        ))),
    }
}

fn read_csv_column_f64(path: &Path, column: &str) -> RunnerResult<Vec<f64>> {
    let rows = read_csv_rows(path)?;
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let value = row.value(column)?.parse().map_err(|err| {
            RunnerError::new(format!(
                "invalid numeric value in '{}' column '{column}': {err}",
                path.display()
            ))
        })?;
        values.push(value);
    }
    Ok(values)
}

fn read_csv_first_f64(path: &Path, column: &str) -> RunnerResult<f64> {
    let mut values = read_csv_column_f64(path, column)?;
    values
        .drain(..)
        .next()
        .ok_or_else(|| RunnerError::new(format!("'{}' contains no rows", path.display())))
}

#[derive(Debug)]
struct CsvRow {
    headers: Vec<String>,
    values: Vec<String>,
}

impl CsvRow {
    fn value(&self, column: &str) -> RunnerResult<&str> {
        let index = self
            .headers
            .iter()
            .position(|header| header == column)
            .ok_or_else(|| RunnerError::new(format!("CSV column '{column}' not found")))?;
        self.values
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| RunnerError::new(format!("CSV column '{column}' is missing a value")))
    }
}

fn read_csv_rows(path: &Path) -> RunnerResult<Vec<CsvRow>> {
    let content = fs::read_to_string(path).map_err(|err| {
        RunnerError::new(format!("failed to read CSV '{}': {err}", path.display()))
    })?;
    let mut lines = content.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Err(RunnerError::new(format!(
            "'{}' is empty or missing a header",
            path.display()
        )));
    };
    let headers = split_csv_line(header_line);
    Ok(lines
        .map(|line| CsvRow {
            headers: headers.clone(),
            values: split_csv_line(line),
        })
        .collect())
}

fn split_csv_line(line: &str) -> Vec<String> {
    line.split(',').map(|part| part.trim().to_owned()).collect()
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let len = sorted.len();
    if len % 2 == 1 {
        sorted[len / 2]
    } else {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    }
}

fn percentile_nearest_rank(values: &[f64], pct: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

fn format_integer(value: f64) -> String {
    let mut digits = format!("{:.0}", value);
    let sign = digits.starts_with('-');
    if sign {
        digits.remove(0);
    }

    let mut grouped = String::new();
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let mut grouped = grouped.chars().rev().collect::<String>();
    if sign {
        grouped.insert(0, '-');
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn median_matches_powershell_even_and_odd_cases() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile_nearest_rank(&values, 95.0), 5.0);
        assert_eq!(percentile_nearest_rank(&values, 50.0), 3.0);
        assert_eq!(percentile_nearest_rank(&values, 1.0), 1.0);
    }

    #[test]
    fn parses_threshold_overrides_and_paths() {
        let config = Config::from_args(os_args(&[
            "--scenario",
            "cold_start",
            "--out-dir",
            "tmp/results",
            "--exe=target/release/custom.exe",
            "--cold-start-median-ms",
            "1",
            "--edit-latency-p95-us=2",
        ]))
        .unwrap();

        assert_eq!(config.scenario, Some(Scenario::ColdStart));
        assert_eq!(config.out_dir, PathBuf::from("tmp/results"));
        assert_eq!(config.exe, PathBuf::from("target/release/custom.exe"));
        assert_eq!(config.cold_start_median_ms, 1.0);
        assert_eq!(config.edit_latency_p95_us, 2.0);
    }

    #[test]
    fn uses_single_default_edit_latency_threshold_row() {
        let config = Config::from_args(os_args(&[
            "--scenario",
            "edit_latency",
            "--edit-latency-p95-us",
            "123",
        ]))
        .unwrap();

        assert_eq!(config.edit_latency_median_us, EDIT_LATENCY_MEDIAN_US);
        assert_eq!(config.edit_latency_p95_us, 123.0);
    }

    #[test]
    fn removed_legacy_flags_are_rejected() {
        for flag in [
            concat!("--ra", "mmap"),
            concat!("--ra", "mmap=tool.exe"),
            concat!("--skip-", "ra", "mmap"),
        ] {
            let error = Config::from_args(os_args(&["--scenario", "all", flag])).unwrap_err();
            assert!(error.to_string().contains("unknown option"));
        }
    }

    #[test]
    fn integer_format_matches_benchmark_output_shape() {
        assert_eq!(format_integer(65000.0), "65,000");
        assert_eq!(format_integer(42.4), "42");
        assert_eq!(format_integer(-1200.0), "-1,200");
    }

    #[test]
    fn scenario_is_required_for_powershell_parity() {
        let error = Config::from_args(os_args(&[])).unwrap_err();

        assert!(error.to_string().contains("missing required --scenario"));
    }
}
