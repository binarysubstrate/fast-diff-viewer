#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(feature = "bench")]
mod bench;
mod buffer_stamp;
mod diff;
mod pipeline;
mod render;
mod segment;
mod theme;

use eframe::egui;

use crate::app::App;

#[cfg(feature = "bench")]
fn main() -> eframe::Result<()> {
    // Capture T0 on the very first line of bench-mode entry so cold
    // start measurements include process init cost.
    let _ = bench::BENCH_T0.set(std::time::Instant::now());

    let bench_config = bench::parse_args(std::env::args());
    // In bench mode, disable vsync. The documented benchmark boundary is
    // app-controlled (end of `App::ui()`), not GPU present; leaving
    // vsync on caps the inter-frame interval at the display refresh period
    // (~16.7 ms at 60 Hz), which edit_latency counts as wall clock between
    // t0 (in frame N) and t1 (in frame N+1). Turning it off lets frames
    // pace on CPU work so the measurement reflects what the gates claim to
    // enforce. A release/production build still defaults to vsync on.
    let bench_active = bench_config.is_some();
    let result = eframe::run_native(
        "Fast Diff Viewer",
        native_options(!bench_active),
        Box::new(move |cc| Ok(Box::new(App::new(cc, bench_config)))),
    );
    report_startup_error(&result);
    result
}

#[cfg(not(feature = "bench"))]
fn main() -> eframe::Result<()> {
    let result = eframe::run_native(
        "Fast Diff Viewer",
        native_options(true),
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    );
    report_startup_error(&result);
    result
}

fn native_options(vsync: bool) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 720.0])
            .with_title("Fast Diff Viewer")
            .with_decorations(false)
            .with_icon(window_icon()),
        persist_window: false,
        vsync,
        ..Default::default()
    };
    configure_linux_graphics(&mut options);
    options
}

#[cfg(target_os = "linux")]
fn configure_linux_graphics(_: &mut eframe::NativeOptions) {
    if running_under_wsl() {
        install_linux_graphics_panic_hook();
        set_env_if_unset("LIBGL_ALWAYS_SOFTWARE", "1");
        set_env_if_unset("GALLIUM_DRIVER", "llvmpipe");
        eprintln!(
            "Fast Diff Viewer: WSL detected; starting eframe Glow/OpenGL with Mesa \
             software rendering fallback to avoid EGL/Zink device discovery failures."
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_graphics(_: &mut eframe::NativeOptions) {}

#[cfg(target_os = "linux")]
fn running_under_wsl() -> bool {
    ["/proc/sys/kernel/osrelease", "/proc/version"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .any(|release| kernel_release_indicates_wsl(&release))
}

#[cfg(target_os = "linux")]
fn set_env_if_unset(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        // SAFETY: this runs during single-threaded process startup, before
        // eframe creates the native event loop or the app starts worker
        // threads. The variables are process-wide OpenGL/Mesa startup knobs.
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn kernel_release_indicates_wsl(release: &str) -> bool {
    release
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| {
            part.eq_ignore_ascii_case("microsoft")
                || part.eq_ignore_ascii_case("wsl")
                || part.eq_ignore_ascii_case("wsl2")
        })
}

#[cfg(target_os = "linux")]
fn report_startup_error(result: &eframe::Result<()>) {
    if let Err(err) = result {
        eprintln!("Fast Diff Viewer failed to start using eframe Glow/OpenGL: {err}");
        eprintln!(
            "Next action: if terminal output mentions Mesa, EGL, or Zink, run \
             with `LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
             fast-diff-viewer` or use an Ubuntu desktop session with working \
             OpenGL drivers."
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn report_startup_error(_: &eframe::Result<()>) {}

#[cfg(target_os = "linux")]
fn install_linux_graphics_panic_hook() {
    static GRAPHICS_PANIC_HOOK: std::sync::Once = std::sync::Once::new();

    GRAPHICS_PANIC_HOOK.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            previous_hook(panic_info);
            eprintln!(
                "Fast Diff Viewer startup diagnostic: eframe Glow/OpenGL \
                 initialization panicked before the app window was ready."
            );
            eprintln!(
                "Next action: if terminal output mentions Mesa, EGL, or Zink, \
                 run with `LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
                 fast-diff-viewer` or use an Ubuntu desktop session with \
                 working OpenGL drivers."
            );
        }));
    });
}

/// Window icon attached to the eframe viewport so task switcher surfaces use
/// the same mark as the platform package icon. Windows also embeds the PE
/// resource through `build.rs`; macOS and Ubuntu release archives carry
/// platform integration files alongside the executable.
///
/// The 256x256 RGBA buffer is emitted by the Cargo example at
/// `scripts/gen_icon.rs` via
/// `cargo run --example gen_icon --release` alongside the `.ico` and PNG
/// source, so the platform artifacts trace back to the same rasterization of
/// `assets/icon/fast-diff-viewer.svg`.
fn window_icon() -> egui::IconData {
    const WINDOW_ICON_RGBA: &[u8] = include_bytes!("../assets/icon/window_icon.rgba.bin");
    const WINDOW_ICON_SIDE: u32 = 256;
    debug_assert_eq!(
        WINDOW_ICON_RGBA.len(),
        (WINDOW_ICON_SIDE * WINDOW_ICON_SIDE * 4) as usize,
        "window_icon.rgba.bin size must match {WINDOW_ICON_SIDE}x{WINDOW_ICON_SIDE} RGBA"
    );
    egui::IconData {
        rgba: WINDOW_ICON_RGBA.to_vec(),
        width: WINDOW_ICON_SIDE,
        height: WINDOW_ICON_SIDE,
    }
}

#[cfg(test)]
mod tests {
    use super::kernel_release_indicates_wsl;

    #[test]
    fn detects_microsoft_wsl_kernel_release() {
        assert!(kernel_release_indicates_wsl(
            "6.6.87.2-microsoft-standard-WSL2"
        ));
        assert!(kernel_release_indicates_wsl(
            "5.15.153.1-Microsoft-standard-WSL2"
        ));
    }

    #[test]
    fn ignores_regular_linux_kernel_release() {
        assert!(!kernel_release_indicates_wsl("6.8.0-31-generic"));
        assert!(!kernel_release_indicates_wsl(
            "6.5.0-1018-azure #18~22.04.1-Ubuntu"
        ));
    }
}
