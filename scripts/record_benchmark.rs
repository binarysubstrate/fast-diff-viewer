//! Record benchmark CSV output into `bench/history/`.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example record_benchmark -- \
//!   --label rust-recorder-smoke --use-existing-results
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

const REQUIRED_CSVS: &[&str] = &[
    "cold_start.csv",
    "edit_latency.csv",
    "large_paste.csv",
    "large_paste_settle.csv",
];

type RecordResult<T> = Result<T, RecordError>;

#[derive(Debug)]
struct RecordError {
    message: String,
}

impl RecordError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for RecordError {}

impl From<std::io::Error> for RecordError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<std::num::ParseFloatError> for RecordError {
    fn from(value: std::num::ParseFloatError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug)]
struct Config {
    label: String,
    runs: u32,
    use_existing_results: bool,
    force: bool,
    results_dir: PathBuf,
    history_root: PathBuf,
}

impl Config {
    fn from_args<I>(args: I) -> RecordResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let repo_root = repo_root();
        let mut label = None;
        let mut release_gate_label = false;
        let mut runs = 1;
        let mut use_existing_results = false;
        let mut force = false;
        let mut results_dir = repo_root.join("bench").join("results");
        let mut history_root = repo_root.join("bench").join("history");
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(RecordError::new("help requested"));
                }
                "--label" => label = Some(parser.next_string("--label")?),
                "--release-gate-label" => release_gate_label = true,
                "--runs" => runs = parser.next_u32("--runs")?,
                "--use-existing-results" => use_existing_results = true,
                "--force" => force = true,
                "--results-dir" => results_dir = parser.next_path("--results-dir")?,
                "--history-root" => history_root = parser.next_path("--history-root")?,
                _ if arg.starts_with("--label=") => {
                    label = Some(split_inline_value(&arg, "--label=")?.to_owned());
                }
                _ if arg.starts_with("--runs=") => {
                    runs = parse_u32_option("--runs", split_inline_value(&arg, "--runs=")?)?;
                }
                _ if arg.starts_with("--results-dir=") => {
                    results_dir = PathBuf::from(split_inline_value(&arg, "--results-dir=")?);
                }
                _ if arg.starts_with("--history-root=") => {
                    history_root = PathBuf::from(split_inline_value(&arg, "--history-root=")?);
                }
                _ => {
                    return Err(RecordError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        let label = match (label, release_gate_label) {
            (Some(_), true) => {
                return Err(RecordError::new(
                    "pass either --label <label> or --release-gate-label, not both",
                ));
            }
            (Some(label), false) => label,
            (None, true) => release_gate_label_from_manifest(&repo_root)?,
            (None, false) => {
                return Err(RecordError::new(
                    "missing required --label <label> or --release-gate-label",
                ));
            }
        };
        validate_label(&label)?;
        if !(1..=100).contains(&runs) {
            return Err(RecordError::new("--runs must be between 1 and 100"));
        }
        if use_existing_results && runs != 1 {
            return Err(RecordError::new(
                "--use-existing-results can only record one run",
            ));
        }

        Ok(Self {
            label,
            runs,
            use_existing_results,
            force,
            results_dir,
            history_root,
        })
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

    fn next_path(&mut self, name: &str) -> RecordResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| RecordError::new(format!("missing value for {name}")))
    }

    fn next_string(&mut self, name: &str) -> RecordResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| RecordError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| RecordError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_u32(&mut self, name: &str) -> RecordResult<u32> {
        parse_u32_option(name, &self.next_string(name)?)
    }
}

fn parse_u32_option(name: &str, value: &str) -> RecordResult<u32> {
    value
        .parse()
        .map_err(|err| RecordError::new(format!("invalid numeric value for {name}: {err}")))
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> RecordResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| RecordError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(RecordError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example record_benchmark -- \\
  (--label <label> | --release-gate-label) [--runs <1-100>] \\
  [--use-existing-results] [--force] [--results-dir <path>] \\
  [--history-root <path>]"
    );
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.message == "help requested" => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> RecordResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let repo_root = repo_root();
    let results_dir = normalize_existing_or_absolute(&config.results_dir);
    let history_root = normalize_existing_or_absolute(&config.history_root);
    fs::create_dir_all(&history_root)?;

    let git = GitInfo::load(&repo_root)?;
    let safe_label = safe_label(&config.label);
    let date = current_date_string();
    let entry_name = format!("{}_{}_{}", date, git.short_commit, safe_label);
    let entry_dir = history_root.join(&entry_name);

    prepare_entry_dir(&entry_dir, &history_root, config.force)?;

    let source_path = source_path_for_metadata(&repo_root, &results_dir);
    let command = benchmark_command_string(&source_path);
    if config.use_existing_results {
        copy_result_csvs(&results_dir, &entry_dir.join("run-1"))?;
    } else {
        for run_id in 1..=config.runs {
            invoke_benchmark_gate(&repo_root, &results_dir, run_id)?;
            copy_result_csvs(&results_dir, &entry_dir.join(format!("run-{run_id}")))?;
        }
    }

    let metadata = Metadata::new(&safe_label, &date, &git, &command, &source_path, &repo_root)?;
    fs::write(entry_dir.join("metadata.json"), metadata.to_json())?;
    write_summary(&entry_dir, &entry_name, &date, &command)?;

    println!("Recorded benchmark history entry: {entry_name}");
    Ok(())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn normalize_existing_or_absolute(path: &Path) -> PathBuf {
    if path.exists() {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root().join(path)
    }
}

fn validate_label(label: &str) -> RecordResult<()> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(RecordError::new("label must match ^[A-Za-z0-9._+-]+$"));
    }
    Ok(())
}

fn safe_label(value: &str) -> String {
    let mut safe = String::new();
    let mut last_was_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+') {
            safe.push(ch);
            last_was_dash = ch == '-';
        } else if !last_was_dash {
            safe.push('-');
            last_was_dash = true;
        }
    }
    safe.trim_matches('-').to_owned()
}

fn release_gate_label_from_manifest(repo_root: &Path) -> RecordResult<String> {
    let version = package_version(repo_root);
    if version == "unknown" {
        return Err(RecordError::new(
            "failed to read package version from Cargo.toml",
        ));
    }
    Ok(safe_label(&format!("v{version}-release-gate")))
}

fn prepare_entry_dir(entry_dir: &Path, history_root: &Path, force: bool) -> RecordResult<()> {
    if entry_dir.exists() && !force {
        return Err(RecordError::new(format!(
            "History entry already exists: {} (pass --force to replace it)",
            entry_dir.display()
        )));
    }

    if entry_dir.exists() {
        let canonical_entry = fs::canonicalize(entry_dir)?;
        let canonical_root = fs::canonicalize(history_root)?;
        if !canonical_entry.starts_with(&canonical_root) || canonical_entry == canonical_root {
            return Err(RecordError::new(format!(
                "refusing to replace directory outside history root: {}",
                canonical_entry.display()
            )));
        }
        fs::remove_dir_all(&canonical_entry)?;
    }

    fs::create_dir_all(entry_dir)?;
    Ok(())
}

fn copy_result_csvs(source: &Path, destination: &Path) -> RecordResult<()> {
    assert_results_complete(source)?;
    fs::create_dir_all(destination)?;
    for csv in REQUIRED_CSVS {
        fs::copy(source.join(csv), destination.join(csv)).map_err(|err| {
            RecordError::new(format!(
                "failed to copy '{}' to '{}': {err}",
                source.join(csv).display(),
                destination.join(csv).display()
            ))
        })?;
    }
    Ok(())
}

fn assert_results_complete(path: &Path) -> RecordResult<()> {
    for csv in REQUIRED_CSVS {
        let full = path.join(csv);
        if !full.exists() {
            return Err(RecordError::new(format!(
                "Missing benchmark CSV: {}",
                full.display()
            )));
        }
    }
    Ok(())
}

fn invoke_benchmark_gate(repo_root: &Path, results_dir: &Path, run_id: u32) -> RecordResult<()> {
    let mut command = Command::new("cargo");
    command
        .current_dir(repo_root)
        .arg("run")
        .arg("--release")
        .arg("--example")
        .arg("bench_runner")
        .arg("--")
        .arg("--scenario")
        .arg("all")
        .arg("--out-dir")
        .arg(results_dir);

    let status = command.status().map_err(|err| {
        RecordError::new(format!("failed to launch Rust benchmark runner: {err}"))
    })?;
    if !status.success() {
        return Err(RecordError::new(format!(
            "Benchmark run {run_id} failed with exit code {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

fn benchmark_command_string(source_path: &str) -> String {
    format!(
        "cargo run --release --example bench_runner -- --scenario all --out-dir {}",
        shell_arg(source_path)
    )
}

fn source_path_for_metadata(repo_root: &Path, results_dir: &Path) -> String {
    let absolute = if results_dir.is_absolute() {
        results_dir.to_path_buf()
    } else {
        repo_root.join(results_dir)
    };
    let absolute = normalize_path_string(&absolute);
    let repo_root = normalize_path_string(repo_root);
    let prefix = format!("{repo_root}/");

    if absolute.eq_ignore_ascii_case(&repo_root) {
        ".".to_owned()
    } else if absolute
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        absolute[prefix.len()..].to_owned()
    } else {
        absolute
    }
}

fn normalize_path_string(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.trim_start_matches(r"\\?\").replace('\\', "/")
}

fn shell_arg(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':' | b'\\'))
    {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

#[derive(Debug)]
struct GitInfo {
    short_commit: String,
    metadata_commit: String,
    metadata_short_commit: String,
    notes: String,
}

impl GitInfo {
    fn load(repo_root: &Path) -> RecordResult<Self> {
        let commit = command_stdout(
            "git",
            [
                "-C".into(),
                repo_root.into(),
                "rev-parse".into(),
                "HEAD".into(),
            ],
        )?;
        let short_commit = command_stdout(
            "git",
            [
                "-C".into(),
                repo_root.into(),
                "rev-parse".into(),
                "--short=7".into(),
                "HEAD".into(),
            ],
        )?;
        let dirty_state = command_stdout(
            "git",
            [
                "-C".into(),
                repo_root.into(),
                "status".into(),
                "--porcelain".into(),
            ],
        )?;
        let dirty = !dirty_state.trim().is_empty();
        let (metadata_commit, metadata_short_commit, notes) = if dirty {
            (
                format!("{commit}-dirty"),
                format!("{short_commit}-dirty"),
                "Recorded from a dirty working tree; the enclosing commit preserves the exact sources."
                    .to_owned(),
            )
        } else {
            (
                commit.clone(),
                short_commit.clone(),
                "Recorded benchmark history entry".to_owned(),
            )
        };

        Ok(Self {
            short_commit,
            metadata_commit,
            metadata_short_commit,
            notes,
        })
    }
}

fn command_stdout(program: &str, args: impl IntoIterator<Item = OsString>) -> RecordResult<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| RecordError::new(format!("failed to run {program}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RecordError::new(format!(
            "{program} exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[derive(Debug)]
struct Metadata {
    label: String,
    date: String,
    commit: String,
    short_commit: String,
    version: String,
    rust_version: String,
    command: String,
    source_path: String,
    gate_mode: String,
    notes: String,
}

impl Metadata {
    fn new(
        label: &str,
        date: &str,
        git: &GitInfo,
        command: &str,
        source_path: &str,
        repo_root: &Path,
    ) -> RecordResult<Self> {
        Ok(Self {
            label: label.to_owned(),
            date: date.to_owned(),
            commit: git.metadata_commit.clone(),
            short_commit: git.metadata_short_commit.clone(),
            version: package_version(repo_root),
            rust_version: rust_version()?,
            command: command.to_owned(),
            source_path: source_path.to_owned(),
            gate_mode: "auto".to_owned(),
            notes: git.notes.clone(),
        })
    }

    fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\n",
                "  \"schema_version\": 1,\n",
                "  \"label\": \"{}\",\n",
                "  \"date\": \"{}\",\n",
                "  \"date_source\": \"benchmark_run_timestamp\",\n",
                "  \"commit\": \"{}\",\n",
                "  \"short_commit\": \"{}\",\n",
                "  \"source_commit\": \"{}\",\n",
                "  \"source_path\": \"{}\",\n",
                "  \"migrated_from\": null,\n",
                "  \"version\": \"{}\",\n",
                "  \"rust_version\": \"{}\",\n",
                "  \"target\": \"x86_64-pc-windows-msvc\",\n",
                "  \"command\": \"{}\",\n",
                "  \"gate_mode\": \"{}\",\n",
                "  \"machine_note\": \"reference workstation\",\n",
                "  \"notes\": \"{}\"\n",
                "}}\n"
            ),
            json_escape(&self.label),
            json_escape(&self.date),
            json_escape(&self.commit),
            json_escape(&self.short_commit),
            json_escape(&self.commit),
            json_escape(&self.source_path),
            json_escape(&self.version),
            json_escape(&self.rust_version),
            json_escape(&self.command),
            json_escape(&self.gate_mode),
            json_escape(&self.notes)
        )
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch < ' ' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn package_version(repo_root: &Path) -> String {
    let cargo_toml = fs::read_to_string(repo_root.join("Cargo.toml")).unwrap_or_default();
    for line in cargo_toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version")
            && let Some(value) = rest.split('"').nth(1)
        {
            return value.to_owned();
        }
    }
    "unknown".to_owned()
}

fn rust_version() -> RecordResult<String> {
    let output = Command::new("rustc")
        .arg("-Vv")
        .output()
        .map_err(|err| RecordError::new(format!("failed to run rustc -Vv: {err}")))?;
    if !output.status.success() {
        return Err(RecordError::new(format!(
            "rustc -Vv exited {}",
            output.status.code().unwrap_or(-1)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[derive(Debug)]
struct RunMetrics {
    cold_median_ms: f64,
    cold_worst_ms: f64,
    edit_median_us: f64,
    edit_p95_us: f64,
    large_worst_frame_us: f64,
    large_settle_ms: f64,
}

fn get_run_metrics(run_dir: &Path) -> RecordResult<RunMetrics> {
    let cold_ms = read_csv_column_f64(&run_dir.join("cold_start.csv"), "ms")?;
    let edit_us = read_csv_column_f64(&run_dir.join("edit_latency.csv"), "latency_us")?;
    let large_us = read_csv_column_f64(&run_dir.join("large_paste.csv"), "frame_us")?;
    let large_settle_ms = read_csv_first_f64(&run_dir.join("large_paste_settle.csv"), "settle_ms")?;

    Ok(RunMetrics {
        cold_median_ms: median(&cold_ms).unwrap_or(0.0),
        cold_worst_ms: cold_ms.iter().copied().reduce(f64::max).unwrap_or(0.0),
        edit_median_us: median(&edit_us).unwrap_or(0.0),
        edit_p95_us: percentile_nearest_rank(&edit_us, 95.0).unwrap_or(0.0),
        large_worst_frame_us: large_us.iter().copied().reduce(f64::max).unwrap_or(0.0),
        large_settle_ms,
    })
}

fn write_summary(
    entry_dir: &Path,
    entry_name: &str,
    date: &str,
    command: &str,
) -> RecordResult<()> {
    let run_dirs = sorted_run_dirs(entry_dir)?;
    let metrics = run_dirs
        .iter()
        .map(|run_dir| get_run_metrics(run_dir))
        .collect::<RecordResult<Vec<_>>>()?;

    let cold_medians = metrics.iter().map(|m| m.cold_median_ms).collect::<Vec<_>>();
    let edit_medians = metrics.iter().map(|m| m.edit_median_us).collect::<Vec<_>>();
    let settle_values = metrics
        .iter()
        .map(|m| m.large_settle_ms)
        .collect::<Vec<_>>();

    let cold_median = median(&cold_medians).unwrap_or(0.0);
    let cold_worst = metrics
        .iter()
        .map(|m| m.cold_worst_ms)
        .reduce(f64::max)
        .unwrap_or(0.0);
    let edit_median = median(&edit_medians).unwrap_or(0.0);
    let edit_p95 = metrics
        .iter()
        .map(|m| m.edit_p95_us)
        .reduce(f64::max)
        .unwrap_or(0.0);
    let large_worst = metrics
        .iter()
        .map(|m| m.large_worst_frame_us)
        .reduce(f64::max)
        .unwrap_or(0.0);
    let large_settle = median(&settle_values).unwrap_or(0.0);

    let summary = [
        format!("# {entry_name}"),
        String::new(),
        format!("- Date: {date}"),
        format!("- Command: `{command}`"),
        format!("- Runs: {}", metrics.len()),
        String::new(),
        "## Aggregate Metrics".to_owned(),
        String::new(),
        "| Metric | Value |".to_owned(),
        "| --- | --- |".to_owned(),
        format!(
            "| Cold-start median of run medians | {} ms |",
            format_number(cold_median, 3)
        ),
        format!(
            "| Cold-start worst across runs | {} ms |",
            format_number(cold_worst, 3)
        ),
        format!(
            "| Edit-latency median of run medians | {} us |",
            format_number(edit_median, 0)
        ),
        format!(
            "| Edit-latency worst p95 across runs | {} us |",
            format_number(edit_p95, 0)
        ),
        format!(
            "| Large-paste worst frame across runs | {} us |",
            format_number(large_worst, 0)
        ),
        format!(
            "| Large-paste median settle | {} ms |",
            format_number(large_settle, 0)
        ),
    ]
    .join("\n");

    fs::write(entry_dir.join("summary.md"), format!("{summary}\n"))?;
    Ok(())
}

fn sorted_run_dirs(entry_dir: &Path) -> RecordResult<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(entry_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("run-") {
            dirs.push(entry.path());
        }
    }
    dirs.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("run-"))
            .and_then(|suffix| suffix.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    if dirs.is_empty() {
        return Err(RecordError::new(format!(
            "No run directories found under {}",
            entry_dir.display()
        )));
    }
    Ok(dirs)
}

#[derive(Debug)]
struct CsvRow {
    headers: Vec<String>,
    values: Vec<String>,
}

impl CsvRow {
    fn value(&self, column: &str) -> RecordResult<&str> {
        let index = self
            .headers
            .iter()
            .position(|header| header == column)
            .ok_or_else(|| RecordError::new(format!("CSV column '{column}' not found")))?;
        self.values
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| RecordError::new(format!("CSV column '{column}' is missing a value")))
    }
}

fn read_csv_column_f64(path: &Path, column: &str) -> RecordResult<Vec<f64>> {
    let rows = read_csv_rows(path)?;
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(row.value(column)?.parse()?);
    }
    Ok(values)
}

fn read_csv_first_f64(path: &Path, column: &str) -> RecordResult<f64> {
    read_csv_column_f64(path, column)?
        .into_iter()
        .next()
        .ok_or_else(|| RecordError::new(format!("'{}' contains no rows", path.display())))
}

fn read_csv_rows(path: &Path) -> RecordResult<Vec<CsvRow>> {
    let content = fs::read_to_string(path).map_err(|err| {
        RecordError::new(format!("failed to read CSV '{}': {err}", path.display()))
    })?;
    let mut lines = content.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Err(RecordError::new(format!(
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

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let len = sorted.len();
    Some(if len % 2 == 1 {
        sorted[len / 2]
    } else {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    })
}

fn percentile_nearest_rank(values: &[f64], pct: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    Some(sorted[rank - 1])
}

fn format_number(value: f64, decimals: usize) -> String {
    let raw = format!("{value:.decimals$}");
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = whole.starts_with('-');
    let digits = if sign { &whole[1..] } else { whole };
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
    if decimals > 0 {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}

#[cfg(windows)]
fn current_date_string() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut time = SYSTEMTIME::default();
    // SAFETY: GetLocalTime writes to the provided SYSTEMTIME pointer.
    unsafe {
        GetLocalTime(&mut time);
    }
    format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay)
}

#[cfg(not(windows))]
fn current_date_string() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(not(windows))]
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn manifest_root_with_version(version: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = repo_root()
            .join("target")
            .join("record-benchmark-tests")
            .join(format!("{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"test-package\"\nversion = \"{version}\"\n"),
        )
        .unwrap();
        root
    }

    #[test]
    fn parses_required_options_and_flags() {
        let config = Config::from_args(os_args(&[
            "--label",
            "Release_1.0",
            "--runs=3",
            "--force",
            "--results-dir",
            "tmp/results",
            "--history-root=tmp/history",
        ]))
        .unwrap();

        assert_eq!(config.label, "Release_1.0");
        assert_eq!(config.runs, 3);
        assert!(config.force);
        assert_eq!(config.results_dir, PathBuf::from("tmp/results"));
        assert_eq!(config.history_root, PathBuf::from("tmp/history"));
    }

    #[test]
    fn release_gate_label_uses_package_version() {
        let root = manifest_root_with_version("1.2.3");

        assert_eq!(
            release_gate_label_from_manifest(&root).unwrap(),
            "v1.2.3-release-gate"
        );
    }

    #[test]
    fn release_gate_label_preserves_semver_build_metadata() {
        let root = manifest_root_with_version("1.2.3+build.4");

        assert_eq!(
            release_gate_label_from_manifest(&root).unwrap(),
            "v1.2.3+build.4-release-gate"
        );
    }

    #[test]
    fn release_gate_label_config_derives_from_manifest() {
        let config =
            Config::from_args(os_args(&["--release-gate-label", "--use-existing-results"]))
                .unwrap();

        assert_eq!(
            config.label,
            release_gate_label_from_manifest(&repo_root()).unwrap()
        );
        assert!(config.use_existing_results);
    }

    #[test]
    fn rejects_label_and_release_gate_label_together() {
        let error =
            Config::from_args(os_args(&["--label", "custom", "--release-gate-label"])).unwrap_err();

        assert!(error.to_string().contains("pass either --label"));
    }

    #[test]
    fn removed_legacy_flag_is_rejected() {
        let error = Config::from_args(os_args(&[
            "--label",
            "Release_1.0",
            concat!("--skip-", "ra", "mmap"),
        ]))
        .unwrap_err();

        assert!(error.to_string().contains("unknown option"));
    }

    #[test]
    fn rejects_invalid_label_like_powershell_pattern() {
        let error = Config::from_args(os_args(&["--label", "bad label"])).unwrap_err();

        assert!(error.to_string().contains("label must match"));
    }

    #[test]
    fn use_existing_results_requires_one_run() {
        let error = Config::from_args(os_args(&[
            "--label",
            "smoke",
            "--runs",
            "2",
            "--use-existing-results",
        ]))
        .unwrap_err();

        assert!(error.to_string().contains("can only record one run"));
    }

    #[test]
    fn safe_label_matches_powershell_normalization() {
        assert_eq!(safe_label("Release_1.0+Build-RC"), "release_1.0+build-rc");
        assert_eq!(safe_label("---A B---"), "a-b");
    }

    #[test]
    fn summary_statistics_match_runner_calculations() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(
            percentile_nearest_rank(&[1.0, 2.0, 3.0, 4.0, 5.0], 95.0),
            Some(5.0)
        );
    }

    #[test]
    fn number_formatting_uses_grouping_and_fixed_decimals() {
        assert_eq!(format_number(1234.5, 3), "1,234.500");
        assert_eq!(format_number(65000.0, 0), "65,000");
    }

    #[test]
    fn metadata_source_path_is_relative_when_results_dir_is_inside_repo() {
        let repo_root = repo_root();
        let source_path =
            source_path_for_metadata(&repo_root, &repo_root.join("target/custom results"));

        assert_eq!(source_path, "target/custom results");
    }

    #[test]
    fn recorded_command_includes_actual_out_dir() {
        let command = benchmark_command_string("target/custom results");

        assert_eq!(
            command,
            "cargo run --release --example bench_runner -- --scenario all --out-dir \"target/custom results\""
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_epoch_date_fallback_is_stable() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
