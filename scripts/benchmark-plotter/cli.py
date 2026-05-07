"""Plot benchmark history and direct benchmark-run comparisons."""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from statistics import median
from typing import Any, Callable, Literal, cast

import matplotlib

_matplotlib = cast(Any, matplotlib)
_matplotlib.use("Agg")

import matplotlib.pyplot as pyplot  # noqa: E402

plt = cast(Any, pyplot)

MODULE_DIR = Path(__file__).resolve().parent
REPO_ROOT = MODULE_DIR.parent.parent
DEFAULT_BEFORE_DIR = REPO_ROOT / "bench" / "history" / (
    "2026-04-20_fe62e84_pre-app-shell-extraction"
)
DEFAULT_AFTER_DIR = REPO_ROOT / "bench" / "history" / (
    "2026-04-20_fe62e84_post-app-shell-extraction"
)
DEFAULT_HISTORY_ROOT = REPO_ROOT / "bench" / "history"
DEFAULT_DIRECT_OUT = DEFAULT_HISTORY_ROOT / "first_to_last_comparison.png"

MetricKey = Literal[
    "cold_start_median",
    "edit_latency_median",
    "large_paste_worst_frame",
    "large_paste_settle",
]
MetricChoice = Literal[
    "cold_start_median",
    "edit_latency_median",
    "large_paste_worst_frame",
    "large_paste_settle",
    "all",
]


@dataclass(frozen=True)
class MetricSpec:
    """Describe a benchmark metric to aggregate and plot."""

    key: MetricKey
    title: str
    subtitle: str
    unit: str
    csv_name: str
    extractor: Callable[[Path], float]


@dataclass(frozen=True)
class RunPoint:
    """Store one metric value for a numbered benchmark run."""

    run: int
    value: float


@dataclass(frozen=True)
class HistoryEntry:
    """Benchmark-history metadata used for labels and ordering."""

    name: str
    path: Path
    label: str
    date: str
    commit: str
    short_commit: str


@dataclass(frozen=True)
class TimelinePoint:
    """One aggregated metric value for a history entry."""

    index: int
    entry: HistoryEntry
    value: float


def parse_args() -> argparse.Namespace:
    """Parse command-line arguments for the plot generator."""

    parser = argparse.ArgumentParser(
        description="Render Fast Diff Viewer benchmark charts."
    )
    parser.add_argument(
        "--before-dir",
        type=Path,
        default=DEFAULT_BEFORE_DIR,
        help="Directory containing before/run-* benchmark folders.",
    )
    parser.add_argument(
        "--after-dir",
        type=Path,
        default=DEFAULT_AFTER_DIR,
        help="Directory containing after/run-* benchmark folders.",
    )
    parser.add_argument(
        "--history-root",
        type=Path,
        default=DEFAULT_HISTORY_ROOT,
        help="Benchmark history root containing named entries.",
    )
    parser.add_argument(
        "--compare",
        nargs=2,
        metavar=("ENTRY_A", "ENTRY_B"),
        help="Compare two entries under --history-root.",
    )
    parser.add_argument(
        "--timeline",
        action="store_true",
        help="Plot all history entries in chronological order.",
    )
    parser.add_argument(
        "--metric",
        choices=[
            "cold_start_median",
            "edit_latency_median",
            "large_paste_worst_frame",
            "large_paste_settle",
            "all",
        ],
        default="all",
        help="Metric to plot. Use all for a four-panel chart.",
    )
    parser.add_argument(
        "--out-file",
        type=Path,
        default=None,
        help="Output image path. PNG is recommended.",
    )
    args = parser.parse_args()
    if args.timeline and args.compare is not None:
        parser.error("--timeline and --compare are mutually exclusive")
    return args


def load_csv_rows(path: Path) -> list[dict[str, str]]:
    """Load a CSV file into row dictionaries with clear missing-file errors."""

    if not path.exists():
        raise FileNotFoundError(f"Required CSV not found: {path}")
    with path.open("r", newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        rows: list[dict[str, str]] = []
        for row in reader:
            normalized = {
                str(key): value
                for key, value in row.items()
                if key is not None and value is not None
            }
            rows.append(normalized)
    if not rows:
        raise ValueError(f"CSV has no data rows: {path}")
    return rows


def extract_cold_start_median_ms(run_path: Path) -> float:
    """Return the median cold-start latency in milliseconds for one run."""

    rows = load_csv_rows(run_path / "cold_start.csv")
    return float(median(float(row["ms"]) for row in rows))


def extract_edit_latency_median_ms(run_path: Path) -> float:
    """Return the median edit latency in milliseconds for one run."""

    rows = load_csv_rows(run_path / "edit_latency.csv")
    return float(median(float(row["latency_us"]) / 1000.0 for row in rows))


def extract_large_paste_worst_ms(run_path: Path) -> float:
    """Return the worst large-paste frame time in milliseconds."""

    rows = load_csv_rows(run_path / "large_paste.csv")
    return max(float(row["frame_us"]) / 1000.0 for row in rows)


def extract_large_paste_settle_ms(run_path: Path) -> float:
    """Return the large-paste settle time in milliseconds."""

    rows = load_csv_rows(run_path / "large_paste_settle.csv")
    return float(rows[0]["settle_ms"])


def build_metric_specs() -> dict[MetricKey, MetricSpec]:
    """Return every benchmark metric the charting CLI knows about."""

    return {
        "cold_start_median": MetricSpec(
            key="cold_start_median",
            title="Cold Start Median",
            subtitle="Median of cold_start.csv ms values",
            unit="ms",
            csv_name="cold_start.csv",
            extractor=extract_cold_start_median_ms,
        ),
        "edit_latency_median": MetricSpec(
            key="edit_latency_median",
            title="Edit Latency Median",
            subtitle="Median of edit_latency.csv latency_us values",
            unit="ms",
            csv_name="edit_latency.csv",
            extractor=extract_edit_latency_median_ms,
        ),
        "large_paste_worst_frame": MetricSpec(
            key="large_paste_worst_frame",
            title="Large Paste Worst Frame",
            subtitle="Max of large_paste.csv frame_us values",
            unit="ms",
            csv_name="large_paste.csv",
            extractor=extract_large_paste_worst_ms,
        ),
        "large_paste_settle": MetricSpec(
            key="large_paste_settle",
            title="Large Paste Settle",
            subtitle="large_paste_settle.csv settle_ms value",
            unit="ms",
            csv_name="large_paste_settle.csv",
            extractor=extract_large_paste_settle_ms,
        ),
    }


def select_metrics(choice: MetricChoice) -> list[MetricSpec]:
    """Return the selected metric list in stable display order."""

    specs = build_metric_specs()
    order: list[MetricKey] = [
        "cold_start_median",
        "edit_latency_median",
        "large_paste_worst_frame",
        "large_paste_settle",
    ]
    if choice == "all":
        return [specs[key] for key in order]
    return [specs[choice]]


def get_run_directories(root: Path) -> list[Path]:
    """Return run directories sorted by their numeric suffix."""

    if not root.exists():
        raise FileNotFoundError(f"Run directory not found: {root}")
    run_dirs = [
        path
        for path in root.iterdir()
        if path.is_dir() and path.name.startswith("run-")
    ]
    if not run_dirs:
        raise FileNotFoundError(f"No run directories found under {root}")
    return sorted(run_dirs, key=lambda path: int(path.name.removeprefix("run-")))


def build_series(
    root: Path,
    metric: MetricSpec,
    *,
    allow_sparse: bool,
) -> list[RunPoint]:
    """Aggregate one metric across sorted run directories."""

    series: list[RunPoint] = []
    missing: list[Path] = []
    for run_dir in get_run_directories(root):
        csv_path = run_dir / metric.csv_name
        if not csv_path.exists() and allow_sparse:
            continue
        if not csv_path.exists():
            missing.append(csv_path)
            continue
        run_number = int(run_dir.name.removeprefix("run-"))
        series.append(RunPoint(run=run_number, value=metric.extractor(run_dir)))
    if missing:
        joined = "\n  ".join(str(path) for path in missing)
        raise FileNotFoundError(f"Missing required CSVs:\n  {joined}")
    if not series:
        raise FileNotFoundError(
            f"No {metric.csv_name} files found under run-* folders in {root}"
        )
    return series


def aggregate_entry_metric(entry_path: Path, metric: MetricSpec) -> float:
    """Aggregate a history entry from its run-* folders for timeline plots."""

    series = build_series(entry_path, metric, allow_sparse=True)
    values = [point.value for point in series]
    if metric.key == "large_paste_worst_frame":
        return max(values)
    return float(median(values))


def load_history_entry(root: Path, name: str) -> HistoryEntry:
    """Load one benchmark-history entry by directory name."""

    path = root / name
    metadata_path = path / "metadata.json"
    if not metadata_path.exists():
        raise FileNotFoundError(f"History metadata not found: {metadata_path}")
    raw = json.loads(metadata_path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise ValueError(f"History metadata must be an object: {metadata_path}")
    metadata = cast(dict[str, object], raw)
    return HistoryEntry(
        name=name,
        path=path,
        label=str(metadata.get("label", name)),
        date=str(metadata.get("date", "unknown-date")),
        commit=str(metadata.get("commit", "")),
        short_commit=str(metadata.get("short_commit", "")),
    )


def clean_commit(commit: str) -> str:
    """Return the Git object id portion of a benchmark metadata commit."""

    return commit.removesuffix("-dirty")


def load_git_commit_order(repo_root: Path) -> dict[str, int]:
    """Return reachable commits ordered from oldest to newest."""

    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), "log", "--reverse", "--format=%H"],
            check=True,
            capture_output=True,
            encoding="utf-8",
        )
    except (OSError, subprocess.CalledProcessError):
        return {}
    commits = [line.strip() for line in completed.stdout.splitlines()]
    return {commit: index for index, commit in enumerate(commits) if commit}


def label_phase_rank(label: str) -> int:
    """Keep paired migrated pre/post benchmark labels in natural order."""

    if label.startswith("pre-"):
        return 0
    if label.startswith("post-"):
        return 2
    return 1


def history_entry_sort_key(
    entry: HistoryEntry,
    commit_order: dict[str, int],
) -> tuple[int, str, int, str]:
    """Return a stable timeline key for benchmark-history entries."""

    commit = clean_commit(entry.commit)
    fallback_rank = len(commit_order)
    return (
        commit_order.get(commit, fallback_rank),
        entry.date,
        label_phase_rank(entry.label),
        entry.name,
    )


def load_history_entries(root: Path) -> list[HistoryEntry]:
    """Load every history entry under a root, sorted chronologically."""

    if not root.exists():
        raise FileNotFoundError(f"History root not found: {root}")
    entries = [
        load_history_entry(root, path.name)
        for path in root.iterdir()
        if path.is_dir() and (path / "metadata.json").exists()
    ]
    if not entries:
        raise FileNotFoundError(f"No history entries found under {root}")
    commit_order = load_git_commit_order(REPO_ROOT)
    return sorted(entries, key=lambda item: history_entry_sort_key(item, commit_order))


def format_value(value: float) -> str:
    """Format a numeric metric value for labels."""

    if abs(value) >= 100.0:
        return f"{value:.0f}"
    if abs(value) >= 10.0:
        return f"{value:.1f}"
    return f"{value:.2f}"


def format_delta(value: float) -> str:
    """Format a signed delta for chart annotations."""

    if value > 0:
        return f"+{format_value(value)}"
    if value < 0:
        return f"-{format_value(abs(value))}"
    return "0.00"


def apply_style() -> None:
    """Apply consistent styling to the generated Matplotlib figure."""

    plt.rcParams.update(
        {
            "figure.facecolor": "#F7F3EE",
            "axes.facecolor": "#FFFCF7",
            "axes.edgecolor": "#D6CCBE",
            "axes.labelcolor": "#3A342E",
            "axes.titlecolor": "#3A342E",
            "axes.grid": False,
            "xtick.color": "#5C544B",
            "ytick.color": "#5C544B",
            "font.family": "DejaVu Sans",
            "font.size": 10,
        }
    )


def make_axes(metric_count: int, title: str, subtitle: str) -> tuple[Any, list[Any]]:
    """Create a one-panel or four-panel benchmark figure."""

    apply_style()
    if metric_count == 1:
        figure, axis = plt.subplots(1, 1, figsize=(10, 6.2))
        axes = [axis]
    else:
        figure, axis_grid = plt.subplots(2, 2, figsize=(14, 12.0))
        axes = list(axis_grid.flat)
    figure.subplots_adjust(
        left=0.07,
        right=0.98,
        bottom=0.12 if metric_count == 1 else 0.08,
        top=0.82,
        hspace=0.62,
        wspace=0.14,
    )
    figure.suptitle(
        title,
        fontsize=24,
        fontweight="bold",
        x=0.04,
        y=0.965,
        ha="left",
        color="#2F2A25",
    )
    figure.text(0.04, 0.915, subtitle, fontsize=11, color="#6E665C")
    return figure, axes


def style_metric_axis(ax: Any, metric: MetricSpec) -> None:
    """Apply shared per-axis chart styling."""

    ax.set_title(metric.title, loc="left", fontsize=15, fontweight="bold", pad=18)
    ax.text(
        0.0,
        1.02,
        metric.subtitle,
        transform=ax.transAxes,
        fontsize=9.5,
        color="#71695F",
        va="bottom",
    )
    ax.grid(axis="y", color="#E5DDD2", linewidth=1.0)
    ax.set_ylabel(metric.unit)
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    ax.spines["left"].set_color("#D6CCBE")
    ax.spines["bottom"].set_color("#D6CCBE")


def annotate_endpoints(
    ax: Any,
    series: list[RunPoint],
    color: str,
    vertical_alignment: str,
) -> None:
    """Annotate the first and last values of a run series."""

    offset = -12 if vertical_alignment == "top" else 12
    for point, align in ((series[0], "left"), (series[-1], "right")):
        ax.annotate(
            f"r{point.run} {format_value(point.value)}",
            xy=(point.run, point.value),
            xytext=(0, offset),
            textcoords="offset points",
            ha=align,
            va=vertical_alignment,
            fontsize=8.8,
            color=color,
            weight="semibold",
        )


def set_y_padding(ax: Any, values: list[float]) -> None:
    """Set a readable y-axis range around the plotted values."""

    low = min(values)
    high = max(values)
    padding = max((high - low) * 0.18, 1.0)
    ax.set_ylim(low - padding, high + padding)


def plot_comparison_metric(
    ax: Any,
    metric: MetricSpec,
    before: list[RunPoint],
    after: list[RunPoint],
    before_label: str,
    after_label: str,
) -> None:
    """Render one metric panel comparing two run-series."""

    before_color = "#B35A66"
    after_color = "#2F7D4D"
    style_metric_axis(ax, metric)

    before_runs = [point.run for point in before]
    after_runs = [point.run for point in after]
    before_values = [point.value for point in before]
    after_values = [point.value for point in after]

    ax.plot(
        before_runs,
        before_values,
        color=before_color,
        linewidth=2.4,
        marker="o",
        markersize=5.5,
        label=before_label,
    )
    ax.plot(
        after_runs,
        after_values,
        color=after_color,
        linewidth=2.4,
        marker="o",
        markersize=5.5,
        label=after_label,
    )
    all_runs = sorted(set(before_runs) | set(after_runs))
    ax.set_xticks(all_runs)
    ax.set_xlabel("Run")
    set_y_padding(ax, before_values + after_values)

    before_delta = before_values[-1] - before_values[0]
    after_delta = after_values[-1] - after_values[0]
    ax.text(
        0.0,
        0.95,
        (
            f"{before_label}: {format_value(before_values[0])} -> "
            f"{format_value(before_values[-1])} {metric.unit} "
            f"({format_delta(before_delta)})"
        ),
        transform=ax.transAxes,
        fontsize=9.0,
        color=before_color,
        va="top",
    )
    ax.text(
        0.0,
        0.88,
        (
            f"{after_label}: {format_value(after_values[0])} -> "
            f"{format_value(after_values[-1])} {metric.unit} "
            f"({format_delta(after_delta)})"
        ),
        transform=ax.transAxes,
        fontsize=9.0,
        color=after_color,
        va="top",
    )
    annotate_endpoints(ax, before, before_color, "bottom")
    annotate_endpoints(ax, after, after_color, "top")
    ax.margins(x=0.06)


def plot_timeline_metric(
    ax: Any,
    metric: MetricSpec,
    points: list[TimelinePoint],
) -> None:
    """Render one metric panel across history entries."""

    color = "#3F6F8F"
    style_metric_axis(ax, metric)
    x_values = [point.index for point in points]
    y_values = [point.value for point in points]
    labels = [point.entry.date for point in points]
    ax.plot(
        x_values,
        y_values,
        color=color,
        linewidth=2.4,
        marker="o",
        markersize=5.5,
    )
    ax.set_xticks(x_values)
    ax.set_xticklabels(labels, rotation=0, ha="center", fontsize=8.5)
    ax.tick_params(axis="x", pad=8)
    set_y_padding(ax, y_values)
    for point in points:
        ax.annotate(
            format_value(point.value),
            xy=(point.index, point.value),
            xytext=(0, 9),
            textcoords="offset points",
            ha="center",
            va="bottom",
            fontsize=8.5,
            color=color,
            weight="semibold",
        )


def create_comparison_figure(
    metrics: list[MetricSpec],
    before_dir: Path,
    after_dir: Path,
    before_label: str,
    after_label: str,
) -> Any:
    """Create a comparison figure for direct folders or history entries."""

    figure, axes = make_axes(
        len(metrics),
        "Benchmark Comparison",
        "Run-by-run benchmark summaries from paired history entries.",
    )
    legend_handles: tuple[list[object], list[str]] | None = None
    for ax, metric in zip(axes, metrics, strict=True):
        before_series = build_series(before_dir, metric, allow_sparse=True)
        after_series = build_series(after_dir, metric, allow_sparse=True)
        plot_comparison_metric(
            ax,
            metric,
            before_series,
            after_series,
            before_label,
            after_label,
        )
        if legend_handles is None:
            handles, labels = ax.get_legend_handles_labels()
            legend_handles = (list(handles), list(labels))
    if legend_handles is not None:
        handles, labels = legend_handles
        figure.legend(
            handles,
            labels,
            loc="upper right",
            bbox_to_anchor=(0.975, 0.94),
            ncols=2,
            frameon=False,
            fontsize=10,
        )
    return figure


def create_timeline_figure(
    metrics: list[MetricSpec],
    entries: list[HistoryEntry],
) -> Any:
    """Create a timeline figure over all benchmark-history entries."""

    figure, axes = make_axes(
        len(metrics),
        "Benchmark History Timeline",
        "Each point aggregates one committed benchmark-history entry.",
    )
    for ax, metric in zip(axes, metrics, strict=True):
        points = [
            TimelinePoint(
                index=index + 1,
                entry=entry,
                value=aggregate_entry_metric(entry.path, metric),
            )
            for index, entry in enumerate(entries)
        ]
        plot_timeline_metric(ax, metric, points)
    return figure


def default_output_path(args: argparse.Namespace) -> Path:
    """Choose a default output path for the selected mode."""

    if args.out_file is not None:
        return cast(Path, args.out_file)
    history_root = cast(Path, args.history_root)
    if args.timeline:
        metric = cast(str, args.metric)
        return history_root / f"timeline_{metric}.png"
    if args.compare is not None:
        entry_a, entry_b = cast(list[str], args.compare)
        return history_root / f"comparison_{entry_a}_vs_{entry_b}.png"
    return DEFAULT_DIRECT_OUT


def main() -> int:
    """Build and save the requested benchmark chart."""

    args = parse_args()
    metric_choice = cast(MetricChoice, args.metric)
    metrics = select_metrics(metric_choice)
    out_file = default_output_path(args)

    if args.timeline:
        entries = load_history_entries(cast(Path, args.history_root))
        figure = create_timeline_figure(metrics, entries)
    elif args.compare is not None:
        entry_a, entry_b = cast(list[str], args.compare)
        root = cast(Path, args.history_root)
        before = load_history_entry(root, entry_a)
        after = load_history_entry(root, entry_b)
        figure = create_comparison_figure(
            metrics,
            before.path,
            after.path,
            before.label,
            after.label,
        )
    else:
        figure = create_comparison_figure(
            metrics,
            cast(Path, args.before_dir),
            cast(Path, args.after_dir),
            "Before",
            "After",
        )

    out_file.parent.mkdir(parents=True, exist_ok=True)
    figure.savefig(out_file, dpi=200, bbox_inches="tight")
    plt.close(figure)
    print(f"Wrote graph to {out_file}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
