//! Probe sampled pixels from a visual-contract screenshot.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example visual_contract_probe -- \
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

type ProbeResult<T> = Result<T, ProbeError>;

const SAMPLE_ROWS: &[u32] = &[50, 59, 67, 74, 83, 91, 100, 108, 117];
const SAMPLE_START_X: u32 = 4;
const SAMPLE_END_X: u32 = 200;
const SAMPLE_STEP_X: u32 = 4;
const NON_GRAY_THRESHOLD: u8 = 30;

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

#[derive(Debug)]
struct ProbeError {
    message: String,
}

impl ProbeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ProbeError {}

impl From<std::io::Error> for ProbeError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<png::DecodingError> for ProbeError {
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
    fn from_args<I>(args: I) -> ProbeResult<Self>
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
                    return Err(ProbeError::new("help requested"));
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
                    return Err(ProbeError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        let case_id =
            case_id.ok_or_else(|| ProbeError::new("missing required --case <case-id>"))?;
        Ok(Self { case_id, in_dir })
    }

    fn input_path(&self) -> PathBuf {
        self.in_dir.join(format!("{}-before.png", self.case_id))
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

    fn next_string(&mut self, name: &str) -> ProbeResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| ProbeError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| ProbeError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_path(&mut self, name: &str) -> ProbeResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| ProbeError::new(format!("missing value for {name}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> ProbeResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| ProbeError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(ProbeError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example visual_contract_probe -- \
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

fn run() -> ProbeResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let path = config.input_path();
    if !path.exists() {
        return Err(ProbeError::new(format!("Missing {}", path.display())));
    }

    let image = Image::load_png(&path)?;
    print_probe(&image);
    Ok(())
}

#[derive(Clone, Debug)]
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn load_png(path: &Path) -> ProbeResult<Self> {
        let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        let mut reader = decoder.read_info()?;
        let output_size = reader
            .output_buffer_size()
            .ok_or_else(|| ProbeError::new("PNG output buffer size is unknown"))?;
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
        for pixel in rgba.as_chunks_mut::<4>().0 {
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

    fn pixel(&self, x: u32, y: u32) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y as usize * self.width as usize + x as usize) * 4;
        Some(Rgb::new(
            self.rgba[index],
            self.rgba[index + 1],
            self.rgba[index + 2],
        ))
    }
}

fn decode_to_rgba(bytes: &[u8], color_type: png::ColorType) -> ProbeResult<Vec<u8>> {
    match color_type {
        png::ColorType::Rgba => Ok(bytes.to_vec()),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(bytes.len() / 3 * 4);
            for pixel in bytes.as_chunks::<3>().0 {
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
            for pixel in bytes.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            Ok(rgba)
        }
        png::ColorType::Indexed => Err(ProbeError::new(
            "indexed PNG color output is not supported by this probe",
        )),
    }
}

fn print_probe(image: &Image) {
    let pane_half = pane_half(image.width);
    println!(
        "Image size: {}x{}  paneHalf={pane_half}",
        image.width, image.height
    );

    for &y in SAMPLE_ROWS {
        println!("== y={y} ==");
        println!("  L: {}", sample_line(image, SAMPLE_START_X, y));
        println!("  R: {}", sample_line(image, pane_half + SAMPLE_START_X, y));
    }
}

fn sample_line(image: &Image, start_x: u32, y: u32) -> String {
    let mut line = String::new();
    for x in (start_x..start_x + SAMPLE_END_X - SAMPLE_START_X).step_by(SAMPLE_STEP_X as usize) {
        let Some(pixel) = image.pixel(x, y) else {
            continue;
        };
        if is_above_threshold(pixel) {
            line.push_str(&format!(
                "[{x:>3}]({r:>3},{g:>3},{b:>3}) ",
                r = pixel.r,
                g = pixel.g,
                b = pixel.b
            ));
        }
    }
    line
}

fn is_above_threshold(pixel: Rgb) -> bool {
    pixel.r > NON_GRAY_THRESHOLD || pixel.g > NON_GRAY_THRESHOLD || pixel.b > NON_GRAY_THRESHOLD
}

fn pane_half(width: u32) -> u32 {
    let half = width / 2;
    if width.is_multiple_of(2) || half.is_multiple_of(2) {
        half
    } else {
        half + 1
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_case_and_default_input_dir() {
        let config = Config::from_args(os_args(&["--case", "A1"])).unwrap();

        assert_eq!(config.case_id, "A1");
        assert_eq!(config.in_dir, PathBuf::from("docs/visual-contract"));
        assert_eq!(
            config.input_path(),
            PathBuf::from("docs/visual-contract/A1-before.png")
        );
    }

    #[test]
    fn parses_inline_options() {
        let config = Config::from_args(os_args(&["--case=B5", "--in-dir=target/probe"])).unwrap();

        assert_eq!(config.case_id, "B5");
        assert_eq!(config.in_dir, PathBuf::from("target/probe"));
    }

    #[test]
    fn missing_case_is_rejected() {
        let err = Config::from_args(os_args(&["--in-dir", "docs/visual-contract"])).unwrap_err();

        assert!(err.to_string().contains("missing required --case"));
    }

    #[test]
    fn pane_half_matches_powershell_int_rounding() {
        assert_eq!(pane_half(1200), 600);
        assert_eq!(pane_half(803), 402);
    }

    #[test]
    fn sample_line_prints_only_pixels_above_threshold() {
        let mut image = Image::new_for_tests(240, 128, Rgb::new(30, 30, 30));
        image.set_pixel(4, 50, Rgb::new(31, 30, 30));
        image.set_pixel(8, 50, Rgb::new(30, 30, 31));
        image.set_pixel(12, 50, Rgb::new(30, 30, 30));

        assert_eq!(
            sample_line(&image, 4, 50),
            "[  4]( 31, 30, 30) [  8]( 30, 30, 31) "
        );
    }
}
