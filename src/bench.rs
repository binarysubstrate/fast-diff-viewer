//! Benchmark harness for the fast-diff-viewer.
//!
//! Gated behind the `bench` Cargo feature so normal builds stay lean.
//!
//! Scenarios:
//! - `cold_start`: captures `T0` at first line of `main()` (in a
//!   `OnceLock<Instant>`) and `T1` at the end of the first
//!   `App::ui()` that painted both panes; emits `cold_start.csv`.
//! - `edit_latency`: loads the 2k-line fixtures via direct buffer
//!   assignment, establishes focus on the left pane, then injects
//!   `Event::Text("x")` on every other frame and records probes keyed
//!   by `revision_after_event`. Each probe resolves at end-of-update
//!   when `LastPaintedStyledRevision >= probe.revision`. Emits
//!   `edit_latency.csv` after 60 probes have resolved.
//! - `large_paste`: loads the 10k-line fixtures, establishes focus on
//!   the right pane, injects a single `Event::Paste(<content>)`,
//!   records per-frame `update()` durations for 120 frames, and
//!   captures the settle timestamp when the paste revision is first
//!   painted styled. Emits `large_paste.csv` and
//!   `large_paste_settle.csv`.
//!
//! CLI contract (only recognized when built with `--features bench`):
//! `fast-diff-viewer --bench <cold_start|edit_latency|large_paste>
//!          --trial <n> --out <dir>`

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use eframe::egui;

/// Global T0 for cold-start measurement. Set on the first line of
/// `main()` so the measurement captures process init cost.
pub static BENCH_T0: OnceLock<Instant> = OnceLock::new();

/// Stable egui IDs for the two TextEdit widgets. The bench harness
/// references these directly when calling `request_focus` / asserting
/// `memory.focused()`, so the widgets must be constructed with
/// `.id(LEFT_EDIT_ID)` / `.id(RIGHT_EDIT_ID)` to bypass the id-stack
/// XOR.
pub fn left_edit_id() -> egui::Id {
    egui::Id::unique("pane_left_edit")
}
pub fn right_edit_id() -> egui::Id {
    egui::Id::unique("pane_right_edit")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    ColdStart,
    EditLatency,
    LargePaste,
}

impl Scenario {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "cold_start" => Some(Self::ColdStart),
            "edit_latency" => Some(Self::EditLatency),
            "large_paste" => Some(Self::LargePaste),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BenchConfig {
    pub scenario: Scenario,
    pub trial: u32,
    pub out_dir: PathBuf,
}

/// Parse `--bench <name> --trial <n> --out <dir>` out of the argv
/// tail. Returns `None` if `--bench` is absent (the binary should run
/// as the normal GUI app in that case). Panics on malformed argv so
/// bench failures surface immediately rather than fall through to
/// normal launch.
pub fn parse_args<I, S>(args: I) -> Option<BenchConfig>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let argv: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
    let mut scenario: Option<Scenario> = None;
    let mut trial: Option<u32> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--bench" => {
                let val = argv.get(i + 1).expect("--bench requires a scenario name");
                scenario = Some(
                    Scenario::parse(val).unwrap_or_else(|| panic!("unknown bench scenario: {val}")),
                );
                i += 2;
            }
            "--trial" => {
                let val = argv.get(i + 1).expect("--trial requires a number");
                trial = Some(val.parse().expect("--trial must be a u32"));
                i += 2;
            }
            "--out" => {
                let val = argv.get(i + 1).expect("--out requires a directory path");
                out_dir = Some(PathBuf::from(val));
                i += 2;
            }
            _ => i += 1,
        }
    }
    let scenario = scenario?;
    Some(BenchConfig {
        scenario,
        trial: trial.unwrap_or(0),
        out_dir: out_dir.unwrap_or_else(|| PathBuf::from("bench/results")),
    })
}

/// A probe for the edit-latency scenario. `t0` is captured the instant
/// the `Event::Text` is pushed into `ctx.input_mut`; `t1` is filled in
/// when `LastPaintedStyledRevision >= revision` at end of update.
#[derive(Clone, Debug)]
pub struct Probe {
    pub id: u64,
    pub revision: u64,
    pub t0: Instant,
    pub t1: Option<Instant>,
}

pub enum BenchState {
    ColdStart {
        /// Frames observed so far (paint attempts).
        frames_painted: u32,
        /// End-of-first-painted-frame timestamp (None until captured).
        t1: Option<Instant>,
        /// Guards against re-writing the cold-start CSV. The close-viewport
        /// request can take several frames to take effect; without this
        /// flag the writer would append duplicate rows each frame.
        finished: bool,
    },
    EditLatency(EditLatencyState),
    LargePaste(LargePasteState),
}

pub struct EditLatencyState {
    pub phase: EditPhase,
    /// Frames since fixtures were assigned. Used to wait for settle.
    pub settle_frames: u32,
    pub probes: Vec<Probe>,
    pub next_probe_id: u64,
    /// Toggle used so injection happens every other frame.
    pub inject_this_frame: bool,
    /// Target number of resolved probes before writing the CSV.
    pub target_probe_count: u32,
    /// Has the final CSV been written already? Finalize can run across
    /// multiple frames before the viewport actually closes; without this
    /// guard the writer appends duplicate rows.
    pub written: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditPhase {
    /// Frame 0: direct-buffer-assign fixtures.
    LoadFixtures,
    /// Wait for the pipeline to settle on the fixture revision.
    WaitSettle,
    /// Call request_focus; one no-op frame will follow.
    RequestFocus,
    /// Assert focus and begin injecting.
    Injecting,
    /// All probes collected; CSV flush pending.
    Finalize,
}

pub struct LargePasteState {
    pub phase: PastePhase,
    pub settle_frames: u32,
    /// Revision assigned to the paste. Filled in after injection.
    pub paste_revision: Option<u64>,
    pub t_paste: Option<Instant>,
    pub t_applied: Option<Instant>,
    /// (frame_id, frame_us) for 120 frames after paste.
    pub frame_times: Vec<(u32, u64)>,
    pub target_frames: u32,
    /// Has the final CSV been written already? See `EditLatencyState`.
    pub written: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PastePhase {
    LoadFixtures,
    WaitSettle,
    RequestFocus,
    Injecting,
    Recording,
    Finalize,
}

/// The controller drives the bench state machine. The `App` embeds an
/// `Option<BenchController>` and forwards `update()` entry/exit to it.
pub struct BenchController {
    pub config: BenchConfig,
    pub state: BenchState,
    /// Left-pane fixture content, pre-loaded during controller
    /// construction so it is available inside the state machine
    /// without re-reading files each frame.
    pub left_fixture: String,
    pub right_fixture: String,
    /// Paste payload for large_paste (lifted from right_fixture for
    /// direct injection via `Event::Paste`).
    pub paste_payload: String,
}

impl BenchController {
    pub fn new(config: BenchConfig) -> io::Result<Self> {
        std::fs::create_dir_all(&config.out_dir)?;
        let (left_fixture, right_fixture, paste_payload) = match config.scenario {
            Scenario::ColdStart => (String::new(), String::new(), String::new()),
            Scenario::EditLatency => {
                let a = std::fs::read_to_string("bench/2k_lines_a.txt")?;
                let b = std::fs::read_to_string("bench/2k_lines_b.txt")?;
                (a, b, String::new())
            }
            Scenario::LargePaste => {
                let a = std::fs::read_to_string("bench/10k_lines_a.txt")?;
                let b = std::fs::read_to_string("bench/10k_lines_b.txt")?;
                // Right pane starts empty; the paste payload is the
                // contents of the `b` fixture.
                (a, String::new(), b)
            }
        };
        let state = match config.scenario {
            Scenario::ColdStart => BenchState::ColdStart {
                frames_painted: 0,
                t1: None,
                finished: false,
            },
            Scenario::EditLatency => BenchState::EditLatency(EditLatencyState {
                phase: EditPhase::LoadFixtures,
                settle_frames: 0,
                probes: Vec::with_capacity(64),
                next_probe_id: 0,
                inject_this_frame: true,
                target_probe_count: 60,
                written: false,
            }),
            Scenario::LargePaste => BenchState::LargePaste(LargePasteState {
                phase: PastePhase::LoadFixtures,
                settle_frames: 0,
                paste_revision: None,
                t_paste: None,
                t_applied: None,
                frame_times: Vec::with_capacity(120),
                target_frames: 120,
                written: false,
            }),
        };
        Ok(Self {
            config,
            state,
            left_fixture,
            right_fixture,
            paste_payload,
        })
    }
}

/// Deterministic LCG-based line generator used by unit tests to
/// confirm fixture-generation invariants. `scripts/gen_fixtures.rs`
/// carries its own copy so that regeneration does not require the
/// bench feature to be enabled.
#[allow(dead_code)]
pub fn gen_lines(line_count: usize, seed: u64, divergence_every: usize) -> String {
    let mut rng_state = seed;
    let next = |s: &mut u64| -> u64 {
        // SplitMix64 — small, self-contained, deterministic.
        *s = s.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    };
    let mut out = String::with_capacity(line_count * 48);
    for idx in 0..line_count {
        let tok_a = next(&mut rng_state) & 0xFFFF;
        let tok_b = next(&mut rng_state) & 0xFFFF;
        let tok_c = next(&mut rng_state) & 0xFFFF;
        let diverge = divergence_every > 0 && (idx % divergence_every == 0);
        if diverge {
            out.push_str(&format!(
                "line {idx:05}: MUT {tok_a:04x} {tok_b:04x} {tok_c:04x}\n"
            ));
        } else {
            out.push_str(&format!(
                "line {idx:05}: NOM {tok_a:04x} {tok_b:04x} {tok_c:04x}\n"
            ));
        }
    }
    out
}

/// Write `cold_start.csv` with one row for this trial.
///
/// Schema: `trial_id,t0_ns,t1_ns,ms`
pub fn write_cold_start_csv(
    out_dir: &Path,
    trial: u32,
    t0_ns: u128,
    t1_ns: u128,
) -> io::Result<()> {
    let path = out_dir.join("cold_start.csv");
    let new_file = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    if new_file {
        writeln!(f, "trial_id,t0_ns,t1_ns,ms")?;
    }
    let ms = (t1_ns.saturating_sub(t0_ns)) as f64 / 1_000_000.0;
    writeln!(f, "{trial},{t0_ns},{t1_ns},{ms:.3}")?;
    Ok(())
}

/// Write `edit_latency.csv` with one row per resolved probe.
///
/// Schema: `probe_id,revision,t0_ns,t1_ns,latency_us`
pub fn write_edit_latency_csv(out_dir: &Path, probes: &[Probe]) -> io::Result<()> {
    let path = out_dir.join("edit_latency.csv");
    let new_file = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    if new_file {
        writeln!(f, "probe_id,revision,t0_ns,t1_ns,latency_us")?;
    }
    let t_ref = probes.first().map(|p| p.t0).unwrap_or_else(Instant::now);
    for p in probes {
        let t1 = match p.t1 {
            Some(t) => t,
            None => continue,
        };
        let t0_ns = p.t0.saturating_duration_since(t_ref).as_nanos();
        let t1_ns = t1.saturating_duration_since(t_ref).as_nanos();
        let latency_us = t1.saturating_duration_since(p.t0).as_micros();
        writeln!(
            f,
            "{},{},{},{},{}",
            p.id, p.revision, t0_ns, t1_ns, latency_us
        )?;
    }
    Ok(())
}

/// Write `large_paste.csv` (per-frame) and `large_paste_settle.csv`
/// (single row).
///
/// Frame-times schema: `frame_id,frame_us`
/// Settle schema: `t_paste_ns,t_applied_ns,settle_ms`
pub fn write_large_paste_csvs(
    out_dir: &Path,
    frame_times: &[(u32, u64)],
    t_paste: Instant,
    t_applied: Instant,
) -> io::Result<()> {
    let frame_path = out_dir.join("large_paste.csv");
    let settle_path = out_dir.join("large_paste_settle.csv");
    {
        let new_file = !frame_path.exists();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&frame_path)?;
        if new_file {
            writeln!(f, "frame_id,frame_us")?;
        }
        for (id, us) in frame_times {
            writeln!(f, "{id},{us}")?;
        }
    }
    {
        let new_file = !settle_path.exists();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&settle_path)?;
        if new_file {
            writeln!(f, "t_paste_ns,t_applied_ns,settle_ms")?;
        }
        let t_ref = t_paste;
        let t_paste_ns = 0u128;
        let t_applied_ns = t_applied.saturating_duration_since(t_ref).as_nanos();
        let settle_ms = t_applied.saturating_duration_since(t_paste).as_millis();
        writeln!(f, "{t_paste_ns},{t_applied_ns},{settle_ms}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::segment;

    #[test]
    fn parse_args_handles_all_three_scenarios() {
        let c = parse_args(["fast-diff-viewer", "--bench", "cold_start"].iter()).unwrap();
        assert_eq!(c.scenario, Scenario::ColdStart);

        let c = parse_args(["fast-diff-viewer", "--bench", "edit_latency"].iter()).unwrap();
        assert_eq!(c.scenario, Scenario::EditLatency);

        let c = parse_args(["fast-diff-viewer", "--bench", "large_paste"].iter()).unwrap();
        assert_eq!(c.scenario, Scenario::LargePaste);
    }

    #[test]
    fn parse_args_returns_none_when_bench_absent() {
        assert!(parse_args(["fast-diff-viewer"].iter()).is_none());
        assert!(parse_args(["fast-diff-viewer", "--other", "value"].iter()).is_none());
    }

    #[test]
    fn parse_args_captures_trial_and_out() {
        let c = parse_args(
            [
                "fast-diff-viewer",
                "--bench",
                "cold_start",
                "--trial",
                "7",
                "--out",
                "bench/results",
            ]
            .iter(),
        )
        .unwrap();
        assert_eq!(c.trial, 7);
        assert_eq!(c.out_dir, PathBuf::from("bench/results"));
    }

    #[test]
    fn gen_lines_is_deterministic() {
        let a1 = gen_lines(100, 0xDEAD_BEEF, 20);
        let a2 = gen_lines(100, 0xDEAD_BEEF, 20);
        assert_eq!(a1, a2, "same seed must produce byte-identical output");
        let b = gen_lines(100, 0xDEAD_BEEF, 0);
        // a has divergence-tagged lines (5% mutated); b has none.
        assert!(a1.contains("MUT "));
        assert!(!b.contains("MUT "));
    }

    #[test]
    fn gen_lines_divergence_rate_matches_parameter() {
        let text = gen_lines(200, 42, 20);
        let mutated = segment(&text)
            .iter()
            .filter(|seg| text[seg.range.clone()].contains("MUT "))
            .count();
        assert_eq!(
            mutated, 10,
            "200 lines with divergence_every=20 → 10 mutated"
        );
    }

    #[test]
    fn stable_text_edit_ids_are_distinct() {
        assert_ne!(left_edit_id(), right_edit_id());
    }
}
