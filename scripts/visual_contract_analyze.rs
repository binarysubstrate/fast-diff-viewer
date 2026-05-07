//! Analyze visual-contract screenshots using Graphite palette heuristics.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example visual_contract_analyze -- \
//!   --case A1 --in-dir docs/visual-contract
//! ```

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type AnalyzeResult<T> = Result<T, AnalyzeError>;

const TOP_INSET: u32 = 34 + 24;
const ROW_HEIGHT: u32 = 17;
const ROW_SAMPLE_OFFSET: u32 = 9;
const MAX_ROWS: u32 = 10;
const COLOR_TOLERANCE_SQUARED: u32 = 120;

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

#[derive(Clone, Copy, Debug)]
struct RefColor {
    name: &'static str,
    rgb: Rgb,
}

const REF_COLORS: &[RefColor] = &[
    RefColor {
        name: "bg_pane",
        rgb: Rgb::new(0x1C, 0x1D, 0x21),
    },
    RefColor {
        name: "removed_line",
        rgb: Rgb::new(0x3A, 0x1F, 0x25),
    },
    RefColor {
        name: "removed_inline",
        rgb: Rgb::new(0xA0, 0x38, 0x4A),
    },
    RefColor {
        name: "added_line",
        rgb: Rgb::new(0x1F, 0x33, 0x27),
    },
    RefColor {
        name: "added_inline",
        rgb: Rgb::new(0x31, 0x6D, 0x4C),
    },
];

#[derive(Debug)]
struct AnalyzeError {
    message: String,
}

impl AnalyzeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for AnalyzeError {}

impl From<std::io::Error> for AnalyzeError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<png::DecodingError> for AnalyzeError {
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
    fn from_args<I>(args: I) -> AnalyzeResult<Self>
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
                    return Err(AnalyzeError::new("help requested"));
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
                    return Err(AnalyzeError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        let case_id =
            case_id.ok_or_else(|| AnalyzeError::new("missing required --case <case-id>"))?;
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

    fn next_string(&mut self, name: &str) -> AnalyzeResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| AnalyzeError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| AnalyzeError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_path(&mut self, name: &str) -> AnalyzeResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| AnalyzeError::new(format!("missing value for {name}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> AnalyzeResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| AnalyzeError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(AnalyzeError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example visual_contract_analyze -- \\
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

fn run() -> AnalyzeResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let path = config.in_dir.join(format!("{}-before.png", config.case_id));
    if !path.exists() {
        return Err(AnalyzeError::new(format!("Missing {}", path.display())));
    }

    let image = Image::load_png(&path)?;
    print_analysis(&config.case_id, &image);
    Ok(())
}

#[derive(Clone, Debug)]
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn load_png(path: &Path) -> AnalyzeResult<Self> {
        let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        let mut reader = decoder.read_info()?;
        let output_size = reader
            .output_buffer_size()
            .ok_or_else(|| AnalyzeError::new("PNG output buffer size is unknown"))?;
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

fn decode_to_rgba(bytes: &[u8], color_type: png::ColorType) -> AnalyzeResult<Vec<u8>> {
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
        png::ColorType::Indexed => Err(AnalyzeError::new(
            "indexed PNG color output is not supported by this analyzer",
        )),
    }
}

fn print_analysis(case_id: &str, image: &Image) {
    println!("Case {} ({}x{})", case_id, image.width, image.height);
    println!(
        "row | y   | L_tint         | L_inline(count/first..last) | L_gut | R_tint          | R_inline(count/first..last) | R_gut"
    );

    for row in 0..MAX_ROWS {
        let y = TOP_INSET + ROW_SAMPLE_OFFSET + row * ROW_HEIGHT;
        if y >= image.height.saturating_sub(10) {
            break;
        }

        let left_tint = row_tint(image, &left_sample_xs(image.width), y);
        let right_tint = row_tint(image, &right_sample_xs(image.width), y);
        let left_inline = inline_strip(image, Pane::Left, y);
        let right_inline = inline_strip(image, Pane::Right, y);
        let left_gutter = gutter_sample(image, Pane::Left, y);
        let right_gutter = gutter_sample(image, Pane::Right, y);

        println!(
            "{row:>3} | {y:>3} | {left_tint:<14} | {li_count:>3}/{li_first:>4}..{li_last:>4}               | {left_gutter:>3}   | {right_tint:<14} | {ri_count:>3}/{ri_first:>4}..{ri_last:>4}               | {right_gutter}",
            li_count = left_inline.count,
            li_first = left_inline.first_x,
            li_last = left_inline.last_x,
            ri_count = right_inline.count,
            ri_first = right_inline.first_x,
            ri_last = right_inline.last_x,
        );
    }
}

fn left_sample_xs(width: u32) -> [i32; 4] {
    let pane_half = pane_half(width);
    [
        pane_half - 200,
        pane_half - 140,
        pane_half - 90,
        pane_half - 50,
    ]
}

fn pane_half(width: u32) -> i32 {
    let half = width / 2;
    if width.is_multiple_of(2) || half.is_multiple_of(2) {
        half as i32
    } else {
        (half + 1) as i32
    }
}

fn right_sample_xs(width: u32) -> [i32; 4] {
    let width = width as i32;
    [width - 220, width - 160, width - 110, width - 60]
}

fn closest(rgb: Rgb) -> &'static str {
    let mut best = "?";
    let mut best_distance = u32::MAX;
    for reference in REF_COLORS {
        let distance = distance_squared(rgb, reference.rgb);
        if distance < best_distance {
            best_distance = distance;
            best = reference.name;
        }
    }
    if best_distance > COLOR_TOLERANCE_SQUARED {
        "other"
    } else {
        best
    }
}

fn distance_squared(a: Rgb, b: Rgb) -> u32 {
    let dr = i32::from(a.r) - i32::from(b.r);
    let dg = i32::from(a.g) - i32::from(b.g);
    let db = i32::from(a.b) - i32::from(b.b);
    (dr * dr + dg * dg + db * db) as u32
}

fn row_tint(image: &Image, xs: &[i32], y: u32) -> &'static str {
    let mut hits = BTreeMap::<&'static str, u32>::new();
    for &x in xs {
        if x <= 0 || x >= image.width as i32 || y >= image.height {
            continue;
        }
        *hits.entry(closest(image.pixel(x as u32, y))).or_default() += 1;
    }
    hits.into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name)
        .unwrap_or("other")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pane {
    Left,
    Right,
}

#[derive(Debug, Eq, PartialEq)]
struct InlineScan {
    count: u32,
    total: u32,
    first_x: i32,
    last_x: i32,
}

fn inline_strip(image: &Image, pane: Pane, y: u32) -> InlineScan {
    let pane_half = pane_half(image.width);
    let (x0, x1, want) = match pane {
        Pane::Left => (40, pane_half - 30, "removed_inline"),
        Pane::Right => (pane_half + 40, image.width as i32 - 30, "added_inline"),
    };

    let mut count = 0;
    let mut total = 0;
    let mut first_x = -1;
    let mut last_x = -1;
    let mut x = x0;
    while x < x1 {
        if x >= image.width as i32 {
            break;
        }
        if x >= 0 && y < image.height {
            total += 1;
            if closest(image.pixel(x as u32, y)) == want {
                count += 1;
                if first_x < 0 {
                    first_x = x;
                }
                last_x = x;
            }
        }
        x += 2;
    }

    InlineScan {
        count,
        total,
        first_x,
        last_x,
    }
}

fn gutter_sample(image: &Image, pane: Pane, y: u32) -> u32 {
    let pane_half = pane_half(image.width);
    let (x0, x1) = match pane {
        Pane::Left => (28, 40),
        Pane::Right => (pane_half + 28, pane_half + 40),
    };

    let mut bright = 0;
    for dy in -6..=6 {
        let yy = y as i32 + dy;
        if yy < 0 || yy >= image.height as i32 {
            continue;
        }
        for x in x0..=x1 {
            if x < 0 || x >= image.width as i32 {
                continue;
            }
            let pixel = image.pixel(x as u32, yy as u32);
            let luminance = 0.2126 * f64::from(pixel.r)
                + 0.7152 * f64::from(pixel.g)
                + 0.0722 * f64::from(pixel.b);
            if luminance > 120.0 {
                bright += 1;
            }
        }
    }
    bright
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_classifies_graphite_refs_and_rejects_distant_colors() {
        assert_eq!(closest(Rgb::new(0x1C, 0x1D, 0x21)), "bg_pane");
        assert_eq!(closest(Rgb::new(0xA0, 0x38, 0x4A)), "removed_inline");
        assert_eq!(closest(Rgb::new(0x31, 0x6D, 0x4C)), "added_inline");
        assert_eq!(closest(Rgb::new(255, 255, 255)), "other");
    }

    #[test]
    fn inline_strip_reports_count_and_bounds_for_left_pane() {
        let mut image = Image::new_for_tests(200, 90, Rgb::new(0x1C, 0x1D, 0x21));
        image.set_pixel(40, 20, Rgb::new(0xA0, 0x38, 0x4A));
        image.set_pixel(44, 20, Rgb::new(0xA0, 0x38, 0x4A));

        assert_eq!(
            inline_strip(&image, Pane::Left, 20),
            InlineScan {
                count: 2,
                total: 15,
                first_x: 40,
                last_x: 44,
            }
        );
    }

    #[test]
    fn odd_width_pane_half_matches_powershell_int_rounding() {
        assert_eq!(pane_half(801), 400);
        assert_eq!(pane_half(803), 402);
        assert_eq!(pane_half(805), 402);
        assert_eq!(left_sample_xs(803), [202, 262, 312, 352]);

        let image = Image::new_for_tests(803, 120, Rgb::new(0x1C, 0x1D, 0x21));
        let scan = inline_strip(&image, Pane::Right, 20);

        assert_eq!(scan.total, 166);
    }

    #[test]
    fn gutter_sample_counts_bright_pixels_in_y_window() {
        let mut image = Image::new_for_tests(120, 90, Rgb::new(0x1C, 0x1D, 0x21));
        image.set_pixel(30, 19, Rgb::new(240, 240, 240));
        image.set_pixel(35, 25, Rgb::new(240, 240, 240));
        image.set_pixel(35, 50, Rgb::new(240, 240, 240));

        assert_eq!(gutter_sample(&image, Pane::Left, 20), 2);
    }
}
