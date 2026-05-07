//! Drive one visual-contract case and write its screenshot.
//!
//! Invoke:
//!
//! ```text
//! cargo run --release --example visual_contract_drive_case -- \
//!   --case A1 --left "alpha\nbeta" --right "alpha\nbeta" \
//!   --out-dir docs/visual-contract --suffix rust-smoke
//! ```

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) type DriverResult<T> = Result<T, DriverError>;

#[derive(Debug)]
pub(crate) struct DriverError {
    message: String,
}

impl DriverError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DriverError {}

impl From<std::io::Error> for DriverError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<png::EncodingError> for DriverError {
    fn from(value: png::EncodingError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug)]
struct Config {
    case_id: String,
    left: String,
    right: String,
    out_dir: PathBuf,
    exe: PathBuf,
    window_width: i32,
    window_height: i32,
    suffix: String,
}

impl Config {
    fn from_args<I>(args: I) -> DriverResult<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut case_id = None;
        let mut left = None;
        let mut right = None;
        let mut out_dir = None;
        let mut exe = default_exe();
        let mut window_width = 1200;
        let mut window_height = 720;
        let mut suffix = "before".to_owned();
        let mut parser = ArgParser::new(args.into_iter().collect());

        while let Some(arg) = parser.next_arg() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                "--help" | "-h" => {
                    print_usage();
                    return Err(DriverError::new("help requested"));
                }
                "--case" => case_id = Some(parser.next_string("--case")?),
                "--left" => left = Some(parser.next_string("--left")?),
                "--right" => right = Some(parser.next_string("--right")?),
                "--out-dir" => out_dir = Some(parser.next_path("--out-dir")?),
                "--exe" => exe = parser.next_path("--exe")?,
                "--window-width" => window_width = parser.next_i32("--window-width")?,
                "--window-height" => window_height = parser.next_i32("--window-height")?,
                "--suffix" => suffix = parser.next_string("--suffix")?,
                _ if arg.starts_with("--case=") => {
                    case_id = Some(split_inline_value(&arg, "--case=")?.to_owned());
                }
                _ if arg.starts_with("--left=") => {
                    left = Some(split_inline_value(&arg, "--left=")?.to_owned());
                }
                _ if arg.starts_with("--right=") => {
                    right = Some(split_inline_value(&arg, "--right=")?.to_owned());
                }
                _ if arg.starts_with("--out-dir=") => {
                    out_dir = Some(PathBuf::from(split_inline_value(&arg, "--out-dir=")?));
                }
                _ if arg.starts_with("--exe=") => {
                    exe = PathBuf::from(split_inline_value(&arg, "--exe=")?);
                }
                _ if arg.starts_with("--window-width=") => {
                    window_width = parse_i32_option(
                        "--window-width",
                        split_inline_value(&arg, "--window-width=")?,
                    )?;
                }
                _ if arg.starts_with("--window-height=") => {
                    window_height = parse_i32_option(
                        "--window-height",
                        split_inline_value(&arg, "--window-height=")?,
                    )?;
                }
                _ if arg.starts_with("--suffix=") => {
                    suffix = split_inline_value(&arg, "--suffix=")?.to_owned();
                }
                _ => {
                    return Err(DriverError::new(format!(
                        "unknown option '{arg}'; pass --help for usage"
                    )));
                }
            }
        }

        if window_width <= 0 || window_height <= 0 {
            return Err(DriverError::new(
                "--window-width and --window-height must be positive",
            ));
        }

        Ok(Self {
            case_id: case_id
                .ok_or_else(|| DriverError::new("missing required --case <case-id>"))?,
            left: left.ok_or_else(|| DriverError::new("missing required --left <text>"))?,
            right: right.ok_or_else(|| DriverError::new("missing required --right <text>"))?,
            out_dir: out_dir
                .ok_or_else(|| DriverError::new("missing required --out-dir <path>"))?,
            exe,
            window_width,
            window_height,
            suffix,
        })
    }

    #[cfg(test)]
    fn output_path(&self) -> PathBuf {
        self.out_dir
            .join(format!("{}-{}.png", self.case_id, self.suffix))
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

    fn next_path(&mut self, name: &str) -> DriverResult<PathBuf> {
        self.next_arg()
            .map(PathBuf::from)
            .ok_or_else(|| DriverError::new(format!("missing value for {name}")))
    }

    fn next_string(&mut self, name: &str) -> DriverResult<String> {
        let value = self
            .next_arg()
            .ok_or_else(|| DriverError::new(format!("missing value for {name}")))?;
        value
            .into_string()
            .map_err(|_| DriverError::new(format!("{name} requires valid UTF-8")))
    }

    fn next_i32(&mut self, name: &str) -> DriverResult<i32> {
        parse_i32_option(name, &self.next_string(name)?)
    }
}

fn parse_i32_option(name: &str, value: &str) -> DriverResult<i32> {
    value
        .parse()
        .map_err(|err| DriverError::new(format!("invalid numeric value for {name}: {err}")))
}

fn split_inline_value<'a>(arg: &'a str, prefix: &str) -> DriverResult<&'a str> {
    let value = arg
        .strip_prefix(prefix)
        .ok_or_else(|| DriverError::new(format!("expected {prefix}<value>")))?;
    if value.is_empty() && !matches!(prefix, "--left=" | "--right=") {
        return Err(DriverError::new(format!("missing value for {prefix}")));
    }
    Ok(value)
}

pub(crate) fn default_exe() -> PathBuf {
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
    let default_exe = PathBuf::from("target")
        .join("release")
        .join(exe_name("fast-diff-viewer"));
    println!(
        "Usage: cargo run --release --example visual_contract_drive_case -- \\
  --case <case-id> --left <text> --right <text> --out-dir <path> \\
  [--exe {}] [--window-width 1200] \\
  [--window-height 720] [--suffix before]",
        default_exe.display()
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

fn run() -> DriverResult<()> {
    let config = Config::from_args(env::args_os().skip(1))?;
    let summary = drive_and_write(&DriveCaseRequest {
        case_id: &config.case_id,
        left: &config.left,
        right: &config.right,
        out_dir: &config.out_dir,
        exe: &config.exe,
        window_width: config.window_width,
        window_height: config.window_height,
        suffix: &config.suffix,
    })?;

    println!(
        "WINRECT: left={} top={} w={} h={}",
        summary.window_left, summary.window_top, summary.window_width, summary.window_height
    );
    println!("SCREENSHOT: {}", summary.output_path.display());
    Ok(())
}

#[derive(Debug)]
pub(crate) struct DriveCaseRequest<'a> {
    pub(crate) case_id: &'a str,
    pub(crate) left: &'a str,
    pub(crate) right: &'a str,
    pub(crate) out_dir: &'a Path,
    pub(crate) exe: &'a Path,
    pub(crate) window_width: i32,
    pub(crate) window_height: i32,
    pub(crate) suffix: &'a str,
}

#[derive(Debug)]
pub(crate) struct DriveCaseSummary {
    pub(crate) output_path: PathBuf,
    pub(crate) window_left: i32,
    pub(crate) window_top: i32,
    pub(crate) window_width: i32,
    pub(crate) window_height: i32,
}

pub(crate) fn drive_and_write(request: &DriveCaseRequest<'_>) -> DriverResult<DriveCaseSummary> {
    let exe = resolve_exe(request.exe)?;
    fs::create_dir_all(request.out_dir)?;
    let output_path = request
        .out_dir
        .join(format!("{}-{}.png", request.case_id, request.suffix));

    let result = platform::drive_case(
        &exe,
        request.left,
        request.right,
        request.window_width,
        request.window_height,
    )?;
    write_png(&output_path, &result.image)?;

    Ok(DriveCaseSummary {
        output_path,
        window_left: result.window.left,
        window_top: result.window.top,
        window_width: result.window.width,
        window_height: result.window.height,
    })
}

fn resolve_exe(path: &Path) -> DriverResult<PathBuf> {
    if !path.exists() {
        return Err(DriverError::new(format!(
            "Exe not found: {}",
            path.display()
        )));
    }
    fs::canonicalize(path)
        .map_err(|err| DriverError::new(format!("failed to resolve '{}': {err}", path.display())))
}

#[derive(Debug)]
struct WindowRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[derive(Debug)]
struct CapturedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Debug)]
struct DriveOutput {
    window: WindowRect,
    image: CapturedImage,
}

fn write_png(path: &Path, image: &CapturedImage) -> DriverResult<()> {
    let file = File::create(path)
        .map_err(|err| DriverError::new(format!("failed to create '{}': {err}", path.display())))?;
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

    use super::{DriveOutput, DriverError, DriverResult};

    pub fn drive_case(
        _exe: &Path,
        _left: &str,
        _right: &str,
        _window_width: i32,
        _window_height: i32,
    ) -> DriverResult<DriveOutput> {
        Err(DriverError::new(
            "visual_contract_drive_case requires Windows Win32 APIs",
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

    use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, HDC, HGDIOBJ, SelectObject,
    };
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, SendInput, VK_A, VK_CONTROL, VK_DELETE, VK_V, mouse_event,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        MoveWindow, PostMessageW, SW_SHOWNORMAL, SetCursorPos, SetForegroundWindow, ShowWindow,
        WM_CLOSE,
    };

    use super::{CapturedImage, DriveOutput, DriverError, DriverResult, WindowRect};

    const WINDOW_LEFT: i32 = 100;
    const WINDOW_TOP: i32 = 100;
    const TITLE_BAR_HEIGHT: i32 = 34;
    const PANE_TOP_PADDING: i32 = 24;
    const PANE_CLICK_Y_OFFSET: i32 = 100;
    const PRINT_WINDOW_RENDER_FULL_CONTENT: u32 = 0x0000_0002;
    const CF_UNICODETEXT: u32 = 13;

    #[derive(Debug)]
    struct AppProcess {
        child: Child,
        hwnd: HWND,
    }

    impl AppProcess {
        fn launch(exe: &Path) -> DriverResult<Self> {
            let child = Command::new(exe).spawn().map_err(|err| {
                DriverError::new(format!("failed to launch '{}': {err}", exe.display()))
            })?;
            Ok(Self {
                child,
                hwnd: null_mut(),
            })
        }

        fn wait_for_window(&mut self) -> DriverResult<HWND> {
            let pid = self.child.id();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait()? {
                    return Err(DriverError::new(format!(
                        "app exited before main window was found: {status}"
                    )));
                }
                if let Some(hwnd) = find_window_for_pid(pid) {
                    self.hwnd = hwnd;
                    return Ok(hwnd);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(DriverError::new("Could not locate fast-diff-viewer window"))
        }

        fn close(&mut self) {
            if self.child_exited() {
                return;
            }
            if !self.hwnd.is_null() {
                // SAFETY: HWND was found by process id for this child.
                unsafe {
                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                }
                self.wait_for_exit(Duration::from_millis(600));
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

    pub fn drive_case(
        exe: &Path,
        left: &str,
        right: &str,
        window_width: i32,
        window_height: i32,
    ) -> DriverResult<DriveOutput> {
        let mut app = AppProcess::launch(exe)?;
        thread::sleep(Duration::from_millis(900));
        let hwnd = app.wait_for_window()?;
        move_window(hwnd, window_width, window_height)?;
        focus_window(hwnd);
        let mut clipboard = ClipboardGuard::capture()?;

        let rect = window_rect(hwnd)?;
        click_pane(&rect, Pane::Left)?;
        replace_text(left, &mut clipboard)?;

        focus_window(hwnd);
        let rect = window_rect(hwnd)?;
        click_pane(&rect, Pane::Right)?;
        replace_text(right, &mut clipboard)?;
        clipboard.restore()?;

        thread::sleep(Duration::from_millis(1_400));
        let rect = window_rect(hwnd)?;
        if rect_width(&rect) <= 0 || rect_height(&rect) <= 0 {
            return Err(DriverError::new(format!(
                "Invalid window rect after drive - w={} h={}",
                rect_width(&rect),
                rect_height(&rect)
            )));
        }
        focus_window(hwnd);
        thread::sleep(Duration::from_millis(300));
        let image = capture_window(hwnd, &rect)?;
        app.close();

        Ok(DriveOutput {
            window: WindowRect {
                left: rect.left,
                top: rect.top,
                width: rect_width(&rect),
                height: rect_height(&rect),
            },
            image,
        })
    }

    fn move_window(hwnd: HWND, width: i32, height: i32) -> DriverResult<()> {
        // SAFETY: HWND comes from EnumWindows for the child process.
        let ok = unsafe { MoveWindow(hwnd, WINDOW_LEFT, WINDOW_TOP, width, height, 1) };
        if ok == 0 {
            return Err(last_os_error("MoveWindow failed"));
        }
        thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    fn focus_window(hwnd: HWND) {
        // SAFETY: HWND comes from EnumWindows. The subsequent click establishes
        // pane focus even if foreground activation is denied by Windows.
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
        }
        thread::sleep(Duration::from_millis(200));
    }

    fn click_pane(rect: &RECT, pane: Pane) -> DriverResult<()> {
        let pane_half = pane_half(rect_width(rect));
        let half_pane_half = round_half_to_even_i32(pane_half);
        let x = match pane {
            Pane::Left => rect.left + half_pane_half,
            Pane::Right => rect.left + pane_half + half_pane_half,
        };
        let y = rect.top + TITLE_BAR_HEIGHT + PANE_TOP_PADDING + PANE_CLICK_Y_OFFSET;

        // SAFETY: Coordinates target the requested app window rect, and this
        // sends the same mouse down/up sequence as the PowerShell driver.
        unsafe {
            if SetCursorPos(x, y) == 0 {
                return Err(last_os_error("SetCursorPos failed"));
            }
            thread::sleep(Duration::from_millis(80));
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
        thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    fn replace_text(text: &str, clipboard: &mut ClipboardGuard) -> DriverResult<()> {
        send_ctrl_key(VK_A)?;
        thread::sleep(Duration::from_millis(60));
        send_key(VK_DELETE)?;
        thread::sleep(Duration::from_millis(60));
        if text.is_empty() {
            return Ok(());
        }
        clipboard.set_text(text)?;
        thread::sleep(Duration::from_millis(100));
        send_ctrl_key(VK_V)?;
        thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    fn send_ctrl_key(vk: u16) -> DriverResult<()> {
        send_inputs(&[
            keyboard_input(VK_CONTROL, 0, 0),
            keyboard_input(vk, 0, 0),
            keyboard_input(vk, 0, KEYEVENTF_KEYUP),
            keyboard_input(VK_CONTROL, 0, KEYEVENTF_KEYUP),
        ])
    }

    fn send_key(vk: u16) -> DriverResult<()> {
        send_inputs(&[
            keyboard_input(vk, 0, 0),
            keyboard_input(vk, 0, KEYEVENTF_KEYUP),
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

    fn send_inputs(inputs: &[INPUT]) -> DriverResult<()> {
        // SAFETY: INPUT slice is initialized and cbsize matches the Win32
        // struct layout.
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

    #[derive(Debug)]
    struct ClipboardGuard {
        original_text: Option<String>,
        restored: bool,
    }

    impl ClipboardGuard {
        fn capture() -> DriverResult<Self> {
            let _clipboard = open_clipboard()?;
            let original_text = read_clipboard_text()?;
            Ok(Self {
                original_text,
                restored: false,
            })
        }

        fn set_text(&mut self, text: &str) -> DriverResult<()> {
            let _clipboard = open_clipboard()?;
            empty_clipboard()?;
            set_clipboard_text_open(text)
        }

        fn restore(&mut self) -> DriverResult<()> {
            if self.restored {
                return Ok(());
            }

            let _clipboard = open_clipboard()?;
            empty_clipboard()?;
            if let Some(text) = self.original_text.as_deref() {
                set_clipboard_text_open(text)?;
            }
            self.restored = true;
            Ok(())
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }

    struct OpenClipboardGuard;

    impl Drop for OpenClipboardGuard {
        fn drop(&mut self) {
            // SAFETY: This guard is only constructed after OpenClipboard
            // succeeds, so CloseClipboard balances that open.
            unsafe {
                CloseClipboard();
            }
        }
    }

    fn open_clipboard() -> DriverResult<OpenClipboardGuard> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // SAFETY: Passing a null owner HWND matches the PowerShell driver,
            // which uses the process clipboard without assigning ownership to
            // the target app window.
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return Ok(OpenClipboardGuard);
            }
            if Instant::now() >= deadline {
                return Err(last_os_error("OpenClipboard failed"));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn empty_clipboard() -> DriverResult<()> {
        // SAFETY: The clipboard is open for the current process while the
        // OpenClipboardGuard is alive.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_os_error("EmptyClipboard failed"));
        }
        Ok(())
    }

    fn read_clipboard_text() -> DriverResult<Option<String>> {
        // SAFETY: The clipboard is open for the current process while the
        // OpenClipboardGuard is alive.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            return Ok(None);
        }

        // SAFETY: GetClipboardData returns a clipboard-owned handle for the
        // requested format while the clipboard remains open.
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Err(last_os_error("GetClipboardData failed"));
        }

        // SAFETY: GlobalSize/GlobalLock operate on the handle returned by
        // GetClipboardData. The data is read-only and unlocked before return.
        let byte_len = unsafe { GlobalSize(handle) };
        if byte_len < size_of::<u16>() {
            return Ok(Some(String::new()));
        }
        let locked = unsafe { GlobalLock(handle) as *const u16 };
        if locked.is_null() {
            return Err(last_os_error("GlobalLock failed"));
        }
        let unit_len = byte_len / size_of::<u16>();
        let text = unsafe {
            let units = slice::from_raw_parts(locked, unit_len);
            let nul = units.iter().position(|unit| *unit == 0).unwrap_or(unit_len);
            String::from_utf16_lossy(&units[..nul])
        };
        // SAFETY: Balances the GlobalLock above. A zero return can simply mean
        // the object is no longer locked, so there is no useful failure to
        // report here.
        unsafe {
            GlobalUnlock(handle);
        }
        Ok(Some(text))
    }

    fn set_clipboard_text_open(text: &str) -> DriverResult<()> {
        let units = utf16_with_nul(text);
        let byte_len = units.len() * size_of::<u16>();

        // SAFETY: Allocates a movable global memory block as required by
        // SetClipboardData for CF_UNICODETEXT.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_len) };
        if handle.is_null() {
            return Err(last_os_error("GlobalAlloc failed"));
        }

        // SAFETY: The handle is owned by this function until SetClipboardData
        // succeeds. The UTF-16 payload includes the required trailing NUL.
        let locked = unsafe { GlobalLock(handle) as *mut u16 };
        if locked.is_null() {
            // SAFETY: The clipboard has not taken ownership of the allocation.
            unsafe {
                GlobalFree(handle);
            }
            return Err(last_os_error("GlobalLock failed"));
        }
        unsafe {
            locked.copy_from_nonoverlapping(units.as_ptr(), units.len());
            GlobalUnlock(handle);
        }

        // SAFETY: The clipboard is open, EmptyClipboard has been called by the
        // caller, and ownership of handle transfers only on success.
        let set_handle = unsafe { SetClipboardData(CF_UNICODETEXT, handle) };
        if set_handle.is_null() {
            // SAFETY: The clipboard did not take ownership on failure.
            unsafe {
                GlobalFree(handle);
            }
            return Err(last_os_error("SetClipboardData failed"));
        }
        Ok(())
    }

    pub(super) fn utf16_with_nul(text: &str) -> Vec<u16> {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        units
    }

    fn capture_window(hwnd: HWND, rect: &RECT) -> DriverResult<CapturedImage> {
        let width = rect_width(rect);
        let height = rect_height(rect);
        if width <= 0 || height <= 0 {
            return Err(DriverError::new(format!(
                "window has invalid capture size: {width} x {height}"
            )));
        }

        // SAFETY: The GDI memory DC and bitmap live through PrintWindow and
        // are restored/deleted before return.
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
    ) -> DriverResult<Vec<u8>> {
        if bits.is_null() {
            return Err(DriverError::new("CreateDIBSection returned null bits"));
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

    fn find_window_for_pid(pid: u32) -> Option<HWND> {
        let mut state = EnumState {
            pid,
            hwnd: null_mut(),
        };
        // SAFETY: lparam is a valid mutable pointer for this synchronous
        // EnumWindows call.
        unsafe {
            EnumWindows(
                Some(enum_windows_proc),
                &mut state as *mut EnumState as LPARAM,
            );
        }
        (!state.hwnd.is_null()).then_some(state.hwnd)
    }

    #[derive(Debug)]
    struct EnumState {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
        let state = unsafe { &mut *(lparam as *mut EnumState) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid == state.pid && unsafe { IsWindowVisible(hwnd) } != 0 && window_title_len(hwnd) > 0 {
            state.hwnd = hwnd;
            return 0;
        }
        1
    }

    fn window_title_len(hwnd: HWND) -> i32 {
        let mut buffer = [0u16; 256];
        // SAFETY: buffer points to valid writable UTF-16 storage.
        unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) }
    }

    fn window_rect(hwnd: HWND) -> DriverResult<RECT> {
        let mut rect = RECT::default();
        // SAFETY: HWND comes from EnumWindows and rect is writable storage.
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
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

    #[derive(Clone, Copy, Debug)]
    enum Pane {
        Left,
        Right,
    }

    pub(super) fn pane_half(width: i32) -> i32 {
        round_half_to_even_i32(width)
    }

    pub(super) fn round_half_to_even_i32(value: i32) -> i32 {
        let half = value / 2;
        if value % 2 == 0 || half % 2 == 0 {
            half
        } else {
            half + value.signum()
        }
    }

    fn last_os_error(context: &str) -> DriverError {
        DriverError::new(format!("{context}: {}", std::io::Error::last_os_error()))
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
    fn parses_required_options_and_defaults() {
        let config = Config::from_args(os_args(&[
            "--case",
            "A1",
            "--left",
            "alpha\nbeta",
            "--right=",
            "--out-dir",
            "docs/visual-contract",
            "--suffix=rust-smoke",
        ]))
        .unwrap();

        assert_eq!(config.case_id, "A1");
        assert_eq!(config.left, "alpha\nbeta");
        assert_eq!(config.right, "");
        assert_eq!(config.out_dir, PathBuf::from("docs/visual-contract"));
        assert_eq!(config.suffix, "rust-smoke");
        assert_eq!(config.window_width, 1200);
        assert_eq!(config.window_height, 720);
    }

    #[test]
    fn output_path_preserves_visual_contract_naming() {
        let config = Config {
            case_id: "B5".to_owned(),
            left: String::new(),
            right: String::new(),
            out_dir: PathBuf::from("out"),
            exe: PathBuf::from("app.exe"),
            window_width: 1200,
            window_height: 720,
            suffix: "before".to_owned(),
        };

        assert_eq!(config.output_path(), PathBuf::from("out/B5-before.png"));
    }

    #[test]
    fn missing_left_or_right_is_rejected_but_empty_is_allowed() {
        let missing =
            Config::from_args(os_args(&["--case", "A1", "--left", "", "--out-dir", "out"]))
                .unwrap_err();
        assert!(missing.to_string().contains("missing required --right"));

        let empty = Config::from_args(os_args(&[
            "--case",
            "A1",
            "--left=",
            "--right=",
            "--out-dir",
            "out",
        ]))
        .unwrap();
        assert_eq!(empty.left, "");
        assert_eq!(empty.right, "");
    }

    #[cfg(windows)]
    #[test]
    fn pane_rounding_matches_powershell_int_rounding() {
        assert_eq!(platform::pane_half(1200), 600);
        assert_eq!(platform::pane_half(803), 402);
        assert_eq!(platform::round_half_to_even_i32(600), 300);
    }

    #[cfg(windows)]
    #[test]
    fn clipboard_payload_preserves_surrogate_pairs() {
        let units = platform::utf16_with_nul("cat 🐱");
        assert_eq!(units.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&units[..units.len() - 1]).unwrap(),
            "cat 🐱"
        );
    }
}
