//! Classify visual-contract screenshot rows using Graphite hue heuristics.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example visual_contract_classify -- \
//!   --case A1 --in-dir docs/visual-contract
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type ClassifyResult<T> = Result<T, ClassifyError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum PixelClass {
    Bg,
    LeftLine,
    LeftInline,
    RightLine,
    RightInline,
}

#[derive(Debug)]
struct ClassifyError {
    message: String,
}

impl ClassifyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ClassifyError {}

impl From<std::io::Error> for ClassifyError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<png::DecodingError> for ClassifyError {
    fn from(value: png::DecodingError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug)]
struct Config {
    case_id: String,
    in_dir: PathBuf,
}

impl Config {
    fn from_args<I>(args: I) -> ClassifyResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut case_id = None;
        let mut in_dir = PathBuf::from("docs").join("visual-contract");
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(ClassifyError::new("help requested"));
                }
                "--case" => case_id = Some(parser.next_string("--case")?),
                "--in-dir" => in_dir = parser.next_path("--in-dir")?,
                _ if arg.starts_with("--case=") => {
                    case_id = Some(split_inline_value(&arg, "--case=")?.to_owned());
                }
                _ if arg.starts_with("--in-dir=") => {
                    in_dir = PathBuf::from(split_inline_value(&arg, "--in-dir=")?);
                }
                _ => {
                    return Err(ClassifyError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        let case_id =
            case_id.ok_or_else(|| ClassifyError::new("missing required --case <case-id>"))?;
        Ok(Self { case_id, in_dir })
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

    fn next_string(&mut self, name: &str) -> ClassifyResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| ClassifyError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| ClassifyError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_path(&mut self, name: &str) -> ClassifyResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| ClassifyError::new(format!("missing value for {name}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> ClassifyResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| ClassifyError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(ClassifyError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example visual_contract_classify -- \\
  --case <case-id> [--in-dir docs/visual-contract]"
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

fn run() -> ClassifyResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let path = config.in_dir.join(format!("{}-before.png", config.case_id));
    if !path.exists() {
        return Err(ClassifyError::new(format!("Missing {}", path.display())));
    }

    let image = Image::load_png(&path)?;
    print_classification(&config.case_id, &image);
    Ok(())
}

#[derive(Clone, Debug)]
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn load_png(path: &Path) -> ClassifyResult<Self> {
        let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        let mut reader = decoder.read_info()?;
        let output_size = reader
            .output_buffer_size()
            .ok_or_else(|| ClassifyError::new("PNG output buffer size is unknown"))?;
        let mut buffer = vec![0; output_size];
        let info = reader.next_frame(&mut buffer)?;
        let bytes = &buffer[..info.buffer_size()];
        let rgba = decode_to_rgba(bytes, info.color_type)?;
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    #[cfg(test)]
    fn new_for_tests(width: u32, height: u32, fill: Rgb) -> Self {
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[0] = fill.r;
            pixel[1] = fill.g;
            pixel[2] = fill.b;
            pixel[3] = 255;
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    #[cfg(test)]
    fn set_pixel(&mut self, x: u32, y: u32, rgb: Rgb) {
        let index = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[index] = rgb.r;
        self.rgba[index + 1] = rgb.g;
        self.rgba[index + 2] = rgb.b;
        self.rgba[index + 3] = 255;
    }

    fn pixel(&self, x: u32, y: u32) -> Rgb {
        let index = (y as usize * self.width as usize + x as usize) * 4;
        Rgb::new(self.rgba[index], self.rgba[index + 1], self.rgba[index + 2])
    }
}

fn decode_to_rgba(bytes: &[u8], color_type: png::ColorType) -> ClassifyResult<Vec<u8>> {
    match color_type {
        png::ColorType::Rgba => Ok(bytes.to_vec()),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(bytes.len() / 3 * 4);
            for pixel in bytes.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            Ok(rgba)
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity(bytes.len() * 4);
            for &value in bytes {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
            Ok(rgba)
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity(bytes.len() / 2 * 4);
            for pixel in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            Ok(rgba)
        }
        png::ColorType::Indexed => Err(ClassifyError::new(
            "indexed PNG color output is not supported by this classifier",
        )),
    }
}

fn print_classification(case_id: &str, image: &Image) {
    println!("=== {case_id} ===");
    for row in detect_rows(image) {
        let y = row.center;
        let left = row_scan(image, y, left_x0(), left_x1(image.width));
        let right = row_scan(image, y, right_x0(image.width), right_x1(image.width));
        let left_mark = gutter_has_marker(image, y, Pane::Left);
        let right_mark = gutter_has_marker(image, y, Pane::Right);
        let left_role = if left.counts.left_line > 0 || left.counts.left_inline > 0 {
            "OLD"
        } else if left_mark {
            "OLDg"
        } else {
            "-"
        };
        let right_role = if right.counts.right_line > 0 || right.counts.right_inline > 0 {
            "NEW"
        } else if right_mark {
            "NEWg"
        } else {
            "-"
        };

        println!(
            "y={y:>3}  L[{left_role:<4}] tint=[{}..{}] inline=[{}..{}] gut={}  |  R[{right_role:<4}] tint=[{}..{}] inline=[{}..{}] gut={}",
            left.ranges.left_line.first,
            left.ranges.left_line.last,
            left.ranges.left_inline.first,
            left.ranges.left_inline.last,
            format_bool(left_mark),
            right.ranges.right_line.first,
            right.ranges.right_line.last,
            right.ranges.right_inline.first,
            right.ranges.right_inline.last,
            format_bool(right_mark)
        );
    }
}

fn format_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn classify_pixel(rgb: Rgb) -> PixelClass {
    let r = i32::from(rgb.r);
    let g = i32::from(rgb.g);
    let b = i32::from(rgb.b);

    if (48..=78).contains(&r) && (r - g) >= 20 && (r - b) >= 15 {
        PixelClass::LeftLine
    } else if r >= 130 && (r - g) >= 80 && (r - b) >= 55 {
        PixelClass::LeftInline
    } else if (42..=62).contains(&g) && (g - r) >= 15 && (g - b) >= 8 {
        PixelClass::RightLine
    } else if g >= 90 && (g - r) >= 50 && (g - b) >= 25 {
        PixelClass::RightInline
    } else {
        PixelClass::Bg
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Range {
    first: i32,
    last: i32,
}

impl Range {
    fn update(&mut self, x: i32) {
        if self.first < 0 {
            self.first = x;
        }
        self.last = x;
    }
}

impl Default for Range {
    fn default() -> Self {
        Self {
            first: -1,
            last: -1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Counts {
    left_line: u32,
    left_inline: u32,
    right_line: u32,
    right_inline: u32,
    bg: u32,
}

impl Counts {
    fn non_bg(self) -> u32 {
        self.left_line + self.left_inline + self.right_line + self.right_inline
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Ranges {
    left_line: Range,
    left_inline: Range,
    right_line: Range,
    right_inline: Range,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RowScan {
    counts: Counts,
    ranges: Ranges,
}

fn row_scan(image: &Image, y: u32, x0: i32, x1: i32) -> RowScan {
    let mut scan = RowScan::default();
    for x in x0..x1 {
        if x < 0 || x >= image.width as i32 || y >= image.height {
            continue;
        }
        match classify_pixel(image.pixel(x as u32, y)) {
            PixelClass::Bg => scan.counts.bg += 1,
            PixelClass::LeftLine => {
                scan.counts.left_line += 1;
                scan.ranges.left_line.update(x);
            }
            PixelClass::LeftInline => {
                scan.counts.left_inline += 1;
                scan.ranges.left_inline.update(x);
            }
            PixelClass::RightLine => {
                scan.counts.right_line += 1;
                scan.ranges.right_line.update(x);
            }
            PixelClass::RightInline => {
                scan.counts.right_inline += 1;
                scan.ranges.right_inline.update(x);
            }
        }
    }
    scan
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pane {
    Left,
    Right,
}

fn gutter_row_has_marker(image: &Image, y: u32, pane: Pane) -> bool {
    let pane_half = pane_half(image.width);
    let (x0, x1) = match pane {
        Pane::Left => (4, 17),
        Pane::Right => (pane_half + 4, pane_half + 17),
    };
    for x in x0..x1 {
        if x < 0 || x >= image.width as i32 || y >= image.height {
            continue;
        }
        let pixel = image.pixel(x as u32, y);
        let r = i32::from(pixel.r);
        let g = i32::from(pixel.g);
        let b = i32::from(pixel.b);
        let has_marker = match pane {
            Pane::Left => r >= 50 && (r - g) >= 10 && (r - b) >= 5,
            Pane::Right => g >= 55 && (g - r) >= 10 && (g - b) >= 5,
        };
        if has_marker {
            return true;
        }
    }
    false
}

fn gutter_has_marker(image: &Image, y: u32, pane: Pane) -> bool {
    for dy in -6..=6 {
        let yy = y as i32 + dy;
        if yy < 0 || yy >= image.height as i32 {
            continue;
        }
        if gutter_row_has_marker(image, yy as u32, pane) {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DetectedRow {
    min: u32,
    max: u32,
    center: u32,
}

fn detect_rows(image: &Image) -> Vec<DetectedRow> {
    let mut hit_y = Vec::new();
    for y in 40..image.height.saturating_sub(20) {
        let left_mark = gutter_row_has_marker(image, y, Pane::Left);
        let right_mark = gutter_row_has_marker(image, y, Pane::Right);
        let mut has_text = false;
        if !(left_mark || right_mark) {
            let left = row_scan(image, y, left_x0(), left_x1(image.width));
            let right = row_scan(image, y, right_x0(image.width), right_x1(image.width));
            has_text = left.counts.non_bg() > 0 || right.counts.non_bg() > 0;
        }
        if left_mark || right_mark || has_text {
            hit_y.push(y);
        }
    }

    collapse_hit_rows(&hit_y)
}

fn collapse_hit_rows(hit_y: &[u32]) -> Vec<DetectedRow> {
    let mut rows = Vec::new();
    let mut current_min = None;
    let mut current_max = 0;

    for &y in hit_y {
        match current_min {
            None => {
                current_min = Some(y);
                current_max = y;
            }
            Some(min) if y - current_max <= 4 => {
                current_max = y;
                current_min = Some(min);
            }
            Some(min) => {
                rows.push(DetectedRow {
                    min,
                    max: current_max,
                    center: round_half_to_even_sum(min, current_max),
                });
                current_min = Some(y);
                current_max = y;
            }
        }
    }

    if let Some(min) = current_min {
        rows.push(DetectedRow {
            min,
            max: current_max,
            center: round_half_to_even_sum(min, current_max),
        });
    }
    rows
}

fn round_half_to_even_sum(a: u32, b: u32) -> u32 {
    let sum = a + b;
    let half = sum / 2;
    if sum.is_multiple_of(2) || half.is_multiple_of(2) {
        half
    } else {
        half + 1
    }
}

fn pane_half(width: u32) -> i32 {
    let half = width / 2;
    if width.is_multiple_of(2) || half.is_multiple_of(2) {
        half as i32
    } else {
        (half + 1) as i32
    }
}

fn left_x0() -> i32 {
    42
}

fn left_x1(width: u32) -> i32 {
    pane_half(width) - 28
}

fn right_x0(width: u32) -> i32 {
    pane_half(width) + 42
}

fn right_x1(width: u32) -> i32 {
    width as i32 - 28
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_pixel_preserves_role_hue_categories() {
        assert_eq!(
            classify_pixel(Rgb::new(0x3A, 0x1F, 0x25)),
            PixelClass::LeftLine
        );
        assert_eq!(
            classify_pixel(Rgb::new(0xA0, 0x38, 0x4A)),
            PixelClass::LeftInline
        );
        assert_eq!(
            classify_pixel(Rgb::new(0x1F, 0x33, 0x27)),
            PixelClass::RightLine
        );
        assert_eq!(
            classify_pixel(Rgb::new(0x31, 0x6D, 0x4C)),
            PixelClass::RightInline
        );
        assert_eq!(classify_pixel(Rgb::new(0x1C, 0x1D, 0x21)), PixelClass::Bg);
    }

    #[test]
    fn row_scan_counts_and_ranges_non_background_pixels() {
        let mut image = Image::new_for_tests(120, 80, Rgb::new(0x1C, 0x1D, 0x21));
        image.set_pixel(42, 50, Rgb::new(0x3A, 0x1F, 0x25));
        image.set_pixel(44, 50, Rgb::new(0xA0, 0x38, 0x4A));
        image.set_pixel(45, 50, Rgb::new(0xA0, 0x38, 0x4A));

        let scan = row_scan(&image, 50, 42, 50);

        assert_eq!(scan.counts.left_line, 1);
        assert_eq!(scan.counts.left_inline, 2);
        assert_eq!(
            scan.ranges.left_line,
            Range {
                first: 42,
                last: 42
            }
        );
        assert_eq!(
            scan.ranges.left_inline,
            Range {
                first: 44,
                last: 45
            }
        );
    }

    #[test]
    fn row_detection_collapses_nearby_hit_y_values() {
        assert_eq!(
            collapse_hit_rows(&[50, 51, 52, 70]),
            vec![
                DetectedRow {
                    min: 50,
                    max: 52,
                    center: 51,
                },
                DetectedRow {
                    min: 70,
                    max: 70,
                    center: 70,
                },
            ]
        );
    }

    #[test]
    fn odd_width_pane_half_matches_powershell_int_rounding() {
        assert_eq!(pane_half(801), 400);
        assert_eq!(pane_half(803), 402);
        assert_eq!(right_x0(803), 444);
    }
}
