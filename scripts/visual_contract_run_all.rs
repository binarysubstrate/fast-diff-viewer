//! Run visual-contract cases through the Rust case driver.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example visual_contract_run_all -- \
//!   --out-dir docs/visual-contract --suffix rust-smoke --only A1
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

#[allow(dead_code)]
#[path = "visual_contract_drive_case.rs"]
mod visual_contract_drive_case;

use visual_contract_drive_case::{DriveCaseRequest, default_exe, drive_and_write};

type RunAllResult<T> = Result<T, RunAllError>;

const DEFAULT_WINDOW_WIDTH: i32 = 1200;
const DEFAULT_WINDOW_HEIGHT: i32 = 720;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaseDef {
    id: &'static str,
    left: &'static str,
    right: &'static str,
}

const CASES: &[CaseDef] = &[
    CaseDef {
        id: "A1",
        left: "alpha\nbeta\ngamma",
        right: "alpha\nbeta\ngamma",
    },
    CaseDef {
        id: "A2",
        left: "alpha\ngamma",
        right: "alpha\nbeta\ngamma",
    },
    CaseDef {
        id: "A3",
        left: "alpha\nGONE\ngamma",
        right: "alpha\ngamma",
    },
    CaseDef {
        id: "A4",
        left: "alpha\ngamma",
        right: "alpha\n\ngamma",
    },
    CaseDef {
        id: "A5",
        left: "alpha\n\ngamma",
        right: "alpha\ngamma",
    },
    CaseDef {
        id: "A6",
        left: "hello",
        right: "",
    },
    CaseDef {
        id: "B1",
        left: "alpha",
        right: "alpha changed",
    },
    CaseDef {
        id: "B2",
        left: "alpha changed",
        right: "alpha",
    },
    CaseDef {
        id: "B3",
        left: "alpha beta",
        right: "alpha gamma",
    },
    CaseDef {
        id: "B4",
        left: "foo bar foo",
        right: "foo baz foo",
    },
    CaseDef {
        id: "B5",
        left: "alpha beta gamma delta",
        right: "alpha BETA gamma DELTA",
    },
    CaseDef {
        id: "C1",
        left: "alpha",
        right: "alpha2",
    },
    CaseDef {
        id: "C2",
        left: "alpha2",
        right: "alpha",
    },
    CaseDef {
        id: "C3",
        left: "abc",
        right: "adc",
    },
    CaseDef {
        id: "C4",
        left: "hello.",
        right: "hello!",
    },
    CaseDef {
        id: "C5",
        left: "port=8080",
        right: "port=8081",
    },
    CaseDef {
        id: "C6",
        left: "alXpha",
        right: "alpha",
    },
    CaseDef {
        id: "D1",
        left: "hello world",
        right: "hello  world",
    },
    CaseDef {
        id: "D2",
        left: "  alpha",
        right: "    alpha",
    },
    CaseDef {
        id: "D3",
        left: "alpha",
        right: "alpha ",
    },
    CaseDef {
        id: "D4",
        left: "alpha\n \nomega",
        right: "alpha\n\nomega",
    },
    CaseDef {
        id: "E1",
        left: "top\nx\ny\nz\nbot",
        right: "top\nX\nY\nbot",
    },
    CaseDef {
        id: "E2",
        left: "top\nx\ny\nbot",
        right: "top\nX\nY\nZ\nbot",
    },
    CaseDef {
        id: "E3",
        left: "alpha\nbeta\n",
        right: "alpha\nbeta",
    },
    CaseDef {
        id: "E4",
        left: "alpha\r\nbeta\r\n",
        right: "alpha\nbeta\n",
    },
    CaseDef {
        id: "F1",
        left: "café",
        right: "cafe",
    },
    CaseDef {
        id: "F2",
        left: "cat 🐱",
        right: "cat 🐶",
    },
];

#[derive(Debug)]
struct RunAllError {
    message: String,
}

impl RunAllError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RunAllError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for RunAllError {}

impl From<visual_contract_drive_case::DriverError> for RunAllError {
    fn from(value: visual_contract_drive_case::DriverError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug)]
struct Config {
    out_dir: PathBuf,
    suffix: String,
    only: Vec<String>,
}

impl Config {
    fn from_args<I>(args: I) -> RunAllResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut out_dir = PathBuf::from("docs").join("visual-contract");
        let mut suffix = "before".to_owned();
        let mut only = Vec::new();
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(RunAllError::new("help requested"));
                }
                "--out-dir" => out_dir = parser.next_path("--out-dir")?,
                "--suffix" => suffix = parser.next_string("--suffix")?,
                "--only" => push_only(&mut only, &parser.next_string("--only")?)?,
                _ if arg.starts_with("--out-dir=") => {
                    out_dir = PathBuf::from(split_inline_value(&arg, "--out-dir=")?);
                }
                _ if arg.starts_with("--suffix=") => {
                    suffix = split_inline_value(&arg, "--suffix=")?.to_owned();
                }
                _ if arg.starts_with("--only=") => {
                    push_only(&mut only, split_inline_value(&arg, "--only=")?)?;
                }
                _ => {
                    return Err(RunAllError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        Ok(Self {
            out_dir,
            suffix,
            only,
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

    fn next_string(&mut self, name: &str) -> RunAllResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| RunAllError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| RunAllError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_path(&mut self, name: &str) -> RunAllResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| RunAllError::new(format!("missing value for {name}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> RunAllResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| RunAllError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(RunAllError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn push_only(only: &mut Vec<String>, value: &str) -> RunAllResult<()> {
    for case_id in value.split(',') {
        if case_id.is_empty() {
            return Err(RunAllError::new("missing case id for --only"));
        }
        only.push(case_id.to_owned());
    }
    Ok(())
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example visual_contract_run_all -- \
  [--out-dir docs/visual-contract] [--suffix before] [--only A1]"
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

fn run() -> RunAllResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let exe = default_exe();

    for case in selected_cases(&config.only) {
        println!("=== Case {} ===", case.id);
        let summary = drive_and_write(&DriveCaseRequest {
            case_id: case.id,
            left: case.left,
            right: case.right,
            out_dir: &config.out_dir,
            exe: &exe,
            window_width: DEFAULT_WINDOW_WIDTH,
            window_height: DEFAULT_WINDOW_HEIGHT,
            suffix: &config.suffix,
        })?;
        println!(
            "WINRECT: left={} top={} w={} h={}",
            summary.window_left, summary.window_top, summary.window_width, summary.window_height
        );
        println!("SCREENSHOT: {}", summary.output_path.display());
        thread::sleep(Duration::from_millis(400));
    }

    Ok(())
}

fn selected_cases(only: &[String]) -> Vec<&'static CaseDef> {
    CASES
        .iter()
        .filter(|case| only.is_empty() || only.iter().any(|selected| selected == case.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_defaults() {
        let config = Config::from_args(os_args(&[])).unwrap();

        assert_eq!(config.out_dir, PathBuf::from("docs/visual-contract"));
        assert_eq!(config.suffix, "before");
        assert!(config.only.is_empty());
    }

    #[test]
    fn parses_repeated_and_comma_only_filters() {
        let config = Config::from_args(os_args(&[
            "--out-dir",
            "out",
            "--suffix=rust-smoke",
            "--only",
            "A1,B5",
            "--only=F2",
        ]))
        .unwrap();

        assert_eq!(config.out_dir, PathBuf::from("out"));
        assert_eq!(config.suffix, "rust-smoke");
        assert_eq!(config.only, ["A1", "B5", "F2"]);
    }

    #[test]
    fn preserves_case_order_when_filtering() {
        let selected = selected_cases(&["B5".to_owned(), "A1".to_owned(), "F2".to_owned()]);
        let ids: Vec<_> = selected.iter().map(|case| case.id).collect();

        assert_eq!(ids, ["A1", "B5", "F2"]);
    }

    #[test]
    fn unmatched_filter_runs_no_cases() {
        assert!(selected_cases(&["Z9".to_owned()]).is_empty());
    }

    #[test]
    fn preserves_power_shell_case_list_edges() {
        assert_eq!(CASES.len(), 27);
        assert_eq!(CASES.first().unwrap().id, "A1");
        assert_eq!(CASES.last().unwrap().id, "F2");
        assert_eq!(CASES[24].left, "alpha\r\nbeta\r\n");
        assert_eq!(CASES[25].left, "café");
        assert_eq!(CASES[26].right, "cat 🐶");
    }
}
