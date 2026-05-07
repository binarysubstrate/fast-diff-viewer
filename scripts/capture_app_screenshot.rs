//! Launch Fast Diff Viewer, inject representative text, capture a PNG, and
//! close the app.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example capture_app_screenshot -- \
//!   --output-path docs/visual-contract/smoke.png
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type CaptureResult<T> = Result<T, CaptureError>;

#[cfg(windows)]
const LEFT_SAMPLE: &str = "\
alpha bravo charlie
delta echo foxtrot
golf hotel india
juliet kilo lima
mike november oscar";

#[cfg(windows)]
const RIGHT_SAMPLE: &str = "\
alpha BRAVO charlie
delta echo foxtrot
golf HOTEL india
juliet kilo LIMA
mike november oscar
papa quebec romeo";

#[derive(Debug)]
struct CaptureError {
    message: String,
}

impl CaptureError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CaptureError {}

impl From<std::io::Error> for CaptureError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<png::EncodingError> for CaptureError {
    fn from(value: png::EncodingError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug)]
struct Config {
    output_path: PathBuf,
    exe: PathBuf,
}

impl Config {
    fn from_args<I>(args: I) -> CaptureResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut output_path = None;
        let mut exe = default_exe();
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(CaptureError::new("help requested"));
                }
                "--output-path" => output_path = Some(parser.next_path("--output-path")?),
                "--exe" => exe = parser.next_path("--exe")?,
                _ if arg.starts_with("--output-path=") => {
                    output_path = Some(PathBuf::from(split_inline_value(&arg, "--output-path=")?));
                }
                _ if arg.starts_with("--exe=") => {
                    exe = PathBuf::from(split_inline_value(&arg, "--exe=")?);
                }
                _ => {
                    return Err(CaptureError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        let output_path = output_path
            .ok_or_else(|| CaptureError::new("missing required --output-path <path-to-png>"))?;

        Ok(Self { output_path, exe })
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

    fn next_path(&mut self, name: &str) -> CaptureResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| CaptureError::new(format!("missing value for {name}")))
    }
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> CaptureResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| CaptureError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() {
        return Err(CaptureError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

fn default_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join(exe_name("fast-diff-viewer"))
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

fn print_usage() {
    println!(
        "Usage: cargo run --release --example capture_app_screenshot -- \\
  --output-path <path-to-png> [--exe <path-to-fast-diff-viewer>]"
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

fn run() -> CaptureResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let exe = resolve_exe(&config.exe)?;
    if let Some(parent) = config
        .output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let image = platform::capture_app(&exe)?;
    write_png(&config.output_path, &image)?;

    println!(
        "Saved {} ({} x {})",
        config.output_path.display(),
        image.width,
        image.height
    );
    Ok(())
}

fn resolve_exe(path: &Path) -> CaptureResult<PathBuf> {
    if !path.exists() {
        return Err(CaptureError::new(format!(
            "Exe not found: {}",
            path.display()
        )));
    }
    fs::canonicalize(path)
        .map_err(|err| CaptureError::new(format!("failed to resolve '{}': {err}", path.display())))
}

#[derive(Debug)]
struct CapturedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn write_png(path: &Path, image: &CapturedImage) -> CaptureResult<()> {
    let file = File::create(path).map_err(|err| {
        CaptureError::new(format!("failed to create '{}': {err}", path.display()))
    })?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.rgba)?;
    Ok(())
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use super::{CaptureError, CaptureResult, CapturedImage};

    pub fn capture_app(_exe: &Path) -> CaptureResult<CapturedImage> {
        Err(CaptureError::new(
            "capture_app_screenshot requires Windows Win32 APIs",
        ))
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::ptr::{self, null_mut};
    use std::slice;
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, HDC, HGDIOBJ, SelectObject,
    };
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, SendInput, VK_RETURN, mouse_event,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
        SW_SHOWNORMAL, SetCursorPos, SetForegroundWindow, ShowWindow, WM_CLOSE,
    };

    use super::{CaptureError, CaptureResult, CapturedImage, LEFT_SAMPLE, RIGHT_SAMPLE};

    const MIN_WINDOW_WIDTH: i32 = 400;
    const MIN_WINDOW_HEIGHT: i32 = 300;
    const PRINT_WINDOW_RENDER_FULL_CONTENT: u32 = 0x0000_0002;

    #[derive(Debug)]
    struct AppProcess {
        child: Child,
        hwnd: HWND,
    }

    impl AppProcess {
        fn launch(exe: &Path) -> CaptureResult<Self> {
            let child = Command::new(exe).spawn().map_err(|err| {
                CaptureError::new(format!("failed to launch '{}': {err}", exe.display()))
            })?;
            Ok(Self {
                child,
                hwnd: null_mut(),
            })
        }

        fn wait_for_window(&mut self) -> CaptureResult<RECT> {
            let pid = self.child.id();
            let deadline = Instant::now() + Duration::from_secs(15);

            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait()? {
                    return Err(CaptureError::new(format!(
                        "app exited before main window reached viewport size: {status}"
                    )));
                }

                if let Some(hwnd) = find_window_for_pid(pid) {
                    let rect = window_rect(hwnd)?;
                    if rect_width(&rect) >= MIN_WINDOW_WIDTH
                        && rect_height(&rect) >= MIN_WINDOW_HEIGHT
                    {
                        self.hwnd = hwnd;
                        return Ok(rect);
                    }
                }
                thread::sleep(Duration::from_millis(150));
            }

            Err(CaptureError::new(
                "main window never reached usable viewport size",
            ))
        }

        fn close(&mut self) {
            if self.child_exited() {
                return;
            }

            if !self.hwnd.is_null() {
                // SAFETY: `self.hwnd` was returned by EnumWindows for the
                // launched process. Posting WM_CLOSE mirrors CloseMainWindow.
                unsafe {
                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                }
                self.wait_for_exit(Duration::from_millis(500));
            }

            if !self.child_exited() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }

        fn child_exited(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(Some(_)))
        }

        fn wait_for_exit(&mut self, timeout: Duration) {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if self.child_exited() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    impl Drop for AppProcess {
        fn drop(&mut self) {
            self.close();
        }
    }

    pub fn capture_app(exe: &Path) -> CaptureResult<CapturedImage> {
        let mut app = AppProcess::launch(exe)?;
        let rect = app.wait_for_window()?;

        focus_window(app.hwnd);
        click_pane(&rect, 0.25)?;
        send_text(LEFT_SAMPLE)?;
        thread::sleep(Duration::from_millis(200));

        let rect = window_rect(app.hwnd)?;
        click_pane(&rect, 0.75)?;
        send_text(RIGHT_SAMPLE)?;
        thread::sleep(Duration::from_millis(1_000));

        let rect = window_rect(app.hwnd)?;
        let image = capture_window(app.hwnd, &rect)?;
        app.close();
        Ok(image)
    }

    fn focus_window(hwnd: HWND) {
        // SAFETY: HWND comes from EnumWindows. The PowerShell script ignores
        // the return value here because the following pane click also focuses
        // the window on normal desktop sessions.
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
        }
        thread::sleep(Duration::from_millis(800));
    }

    fn click_pane(rect: &RECT, x_fraction: f32) -> CaptureResult<()> {
        let x = rect.left + ((rect_width(rect) as f32) * x_fraction) as i32;
        let y = rect.top + rect_height(rect) / 2;

        // SAFETY: Coordinates are inside the window rect returned by Win32;
        // mouse_event sends the same left-click sequence as the script.
        unsafe {
            if SetCursorPos(x, y) == 0 {
                return Err(last_os_error("SetCursorPos failed"));
            }
            thread::sleep(Duration::from_millis(150));
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }

        thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    fn send_text(text: &str) -> CaptureResult<()> {
        for ch in text.chars() {
            match ch {
                '\n' => send_key(VK_RETURN)?,
                '\r' => {}
                _ => {
                    let mut units = [0u16; 2];
                    for unit in ch.encode_utf16(&mut units) {
                        send_unicode_unit(*unit)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn send_key(vk: u16) -> CaptureResult<()> {
        send_inputs(&[
            keyboard_input(vk, 0, 0),
            keyboard_input(vk, 0, KEYEVENTF_KEYUP),
        ])
    }

    fn send_unicode_unit(unit: u16) -> CaptureResult<()> {
        send_inputs(&[
            keyboard_input(0, unit, KEYEVENTF_UNICODE),
            keyboard_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        ])
    }

    fn keyboard_input(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send_inputs(inputs: &[INPUT]) -> CaptureResult<()> {
        // SAFETY: The slice points to initialized INPUT values and the cbsize
        // matches the Win32 INPUT layout from windows-sys.
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            )
        };
        if sent != inputs.len() as u32 {
            return Err(last_os_error("SendInput failed"));
        }
        Ok(())
    }

    fn capture_window(hwnd: HWND, rect: &RECT) -> CaptureResult<CapturedImage> {
        let width = rect_width(rect);
        let height = rect_height(rect);
        if width <= 0 || height <= 0 {
            return Err(CaptureError::new(format!(
                "window has invalid capture size: {width} x {height}"
            )));
        }

        // SAFETY: The GDI objects are created for a memory DC, selected for
        // PrintWindow, read while alive, then restored and deleted.
        unsafe {
            let dc = CreateCompatibleDC(ptr::null_mut::<c_void>() as HDC);
            if dc.is_null() {
                return Err(last_os_error("CreateCompatibleDC failed"));
            }

            let mut bits: *mut c_void = null_mut();
            let mut info = BITMAPINFO::default();
            info.bmiHeader.biSize =
                size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;

            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
            if bitmap.is_null() {
                DeleteDC(dc);
                return Err(last_os_error("CreateDIBSection failed"));
            }

            let old = SelectObject(dc, bitmap as HGDIOBJ);
            if old.is_null() {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(dc);
                return Err(last_os_error("SelectObject failed"));
            }

            let print_ok = PrintWindow(hwnd, dc, PRINT_WINDOW_RENDER_FULL_CONTENT) != 0;
            let rgba_result = if print_ok {
                dib_bgra_to_rgba(bits, width as u32, height as u32)
            } else {
                Err(last_os_error("PrintWindow returned false"))
            };

            SelectObject(dc, old);
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(dc);

            rgba_result.map(|rgba| CapturedImage {
                width: width as u32,
                height: height as u32,
                rgba,
            })
        }
    }

    unsafe fn dib_bgra_to_rgba(
        bits: *mut c_void,
        width: u32,
        height: u32,
    ) -> CaptureResult<Vec<u8>> {
        if bits.is_null() {
            return Err(CaptureError::new("CreateDIBSection returned null bits"));
        }

        let len = width as usize * height as usize * 4;
        let bgra = unsafe { slice::from_raw_parts(bits as *const u8, len) };
        let mut rgba = Vec::with_capacity(len);
        for pixel in bgra.chunks_exact(4) {
            rgba.push(pixel[2]);
            rgba.push(pixel[1]);
            rgba.push(pixel[0]);
            rgba.push(255);
        }
        Ok(rgba)
    }

    fn window_rect(hwnd: HWND) -> CaptureResult<RECT> {
        let mut rect = RECT::default();
        // SAFETY: HWND comes from EnumWindows and rect points to initialized
        // stack storage for Win32 to fill.
        let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
        if ok == 0 {
            return Err(last_os_error("GetWindowRect failed"));
        }
        Ok(rect)
    }

    fn rect_width(rect: &RECT) -> i32 {
        rect.right - rect.left
    }

    fn rect_height(rect: &RECT) -> i32 {
        rect.bottom - rect.top
    }

    #[derive(Debug)]
    struct EnumState {
        pid: u32,
        hwnd: HWND,
    }

    fn find_window_for_pid(pid: u32) -> Option<HWND> {
        let mut state = EnumState {
            pid,
            hwnd: null_mut(),
        };

        // SAFETY: lparam carries a valid mutable pointer for the duration of
        // the synchronous EnumWindows call.
        unsafe {
            EnumWindows(
                Some(enum_windows_proc),
                &mut state as *mut EnumState as LPARAM,
            );
        }

        (!state.hwnd.is_null()).then_some(state.hwnd)
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
        let state = unsafe { &mut *(lparam as *mut EnumState) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid == state.pid {
            let visible = unsafe { IsWindowVisible(hwnd) } != 0;
            if visible {
                state.hwnd = hwnd;
                return 0;
            }
        }
        1
    }

    fn last_os_error(context: &str) -> CaptureError {
        CaptureError::new(format!("{context}: {}", std::io::Error::last_os_error()))
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
    fn output_path_is_required() {
        let error = Config::from_args(os_args(&[])).unwrap_err();

        assert!(error.to_string().contains("missing required --output-path"));
    }

    #[test]
    fn parses_output_path_and_exe() {
        let config = Config::from_args(os_args(&[
            "--output-path",
            "docs/visual-contract/smoke.png",
            "--exe=target/release/custom.exe",
        ]))
        .unwrap();

        assert_eq!(
            config.output_path,
            PathBuf::from("docs/visual-contract/smoke.png")
        );
        assert_eq!(config.exe, PathBuf::from("target/release/custom.exe"));
    }
}
