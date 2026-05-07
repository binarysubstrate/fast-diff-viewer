//! Regenerate the shell / window icon assets from code.
//!
//! This example rasterizes the Fast Diff Viewer brand mark
//! (diagonal-split rounded square, see `assets/icon/fast-diff-viewer.svg` and
//! `draw_brand_mark` in `src/main.rs`) at every shell-icon size
//! Windows and macOS may ask for, then emits icon artifacts under
//! `assets/icon/`:
//!
//! * `fast-diff-viewer.ico` - multi-resolution ICO consumed by the Windows
//!   resource compiler via `build.rs` to embed the file / Explorer
//!   icon into `fast-diff-viewer.exe`.
//! * `window_icon.rgba.bin` - flat 256x256 RGBA byte buffer that
//!   `src/main.rs` hands to `ViewportBuilder::with_icon` so the live
//!   taskbar / Alt-Tab icon matches the PE resource at runtime.
//! * `fast-diff-viewer.png` - 1024x1024 PNG used by the macOS release
//!   workflow to generate the `.app` bundle's `.icns` icon.
//!
//! Run with:
//!
//! ```text
//! cargo run --example gen_icon --release
//! ```
//!
//! Only rerun when the brand mark geometry or the shipping
//! (Graphite) inline palette changes. The generated artifacts are
//! committed to the repo so normal `cargo build --release` never
//! needs to execute this example.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

// Shell-icon colorway is locked to the Graphite (shipping default)
// theme's inline-emphasis pair. Keep these in sync with
// `AppTheme::graphite()` in `src/theme.rs`.
const REMOVED_INLINE: [u8; 3] = [0xA0, 0x38, 0x4A];
const ADDED_INLINE: [u8; 3] = [0x31, 0x6D, 0x4C];
// Graphite `ink` at ~15% opacity, matching the in-app ring in
// `draw_brand_mark`.
const RING_INK: [u8; 3] = [0xE6, 0xE1, 0xD6];
const RING_ALPHA: f32 = 0.15;

// Proportions baked into the 256x256 vector master. `CORNER_RATIO`
// matches `rect.width() * (48.0 / 256.0)` from `draw_brand_mark`.
const CORNER_RATIO: f32 = 48.0 / 256.0;

// Windows shell needs 16-256 px; these cover small list views
// (16, 20, 24), Explorer medium icons (32, 40, 48), Alt-Tab / jump
// list (64, 128), and high-DPI shell surfaces (256). Sizes below
// `RING_MIN_SIZE` drop the hairline ring because it collapses into a
// muddy pixel on very small renderings.
const ICO_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 128, 256];
const RING_MIN_SIZE: u32 = 24;
const WINDOW_ICON_SIZE: u32 = 256;
const APP_ICON_PNG_SIZE: u32 = 1024;

// Supersample factor for anti-aliasing. A 4x oversample + box filter
// is enough to keep the diagonal and the rounded corners clean at
// every output size without pulling in a full vector renderer.
const SUPERSAMPLE: u32 = 4;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon");
    fs::create_dir_all(&out_dir)?;

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICO_SIZES {
        let rgba = render_mark(size);
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    let ico_path = out_dir.join("fast-diff-viewer.ico");
    icon_dir.write(BufWriter::new(File::create(&ico_path)?))?;
    println!("wrote {} ({} sizes)", ico_path.display(), ICO_SIZES.len());

    let window_rgba = render_mark(WINDOW_ICON_SIZE);
    let rgba_path = out_dir.join("window_icon.rgba.bin");
    fs::write(&rgba_path, &window_rgba)?;
    println!(
        "wrote {} ({}x{} RGBA, {} bytes)",
        rgba_path.display(),
        WINDOW_ICON_SIZE,
        WINDOW_ICON_SIZE,
        window_rgba.len()
    );

    let app_icon_rgba = render_mark(APP_ICON_PNG_SIZE);
    let png_path = out_dir.join("fast-diff-viewer.png");
    write_png(&png_path, APP_ICON_PNG_SIZE, &app_icon_rgba)?;
    println!(
        "wrote {} ({}x{} PNG source)",
        png_path.display(),
        APP_ICON_PNG_SIZE,
        APP_ICON_PNG_SIZE
    );

    Ok(())
}

fn write_png(path: &Path, size: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Rasterize the diagonal-split rounded square at `size` x `size`
/// pixels, returning an RGBA8 buffer in row-major, top-down order.
///
/// The renderer paints into a SUPERSAMPLE x oversampled canvas and
/// box-filters down so the rounded corners and the anti-diagonal
/// stay smooth without needing a full SVG rasterizer.
fn render_mark(size: u32) -> Vec<u8> {
    let hi = size * SUPERSAMPLE;
    let mut hi_buf = vec![0u8; (hi * hi * 4) as usize];

    let corner = size as f32 * CORNER_RATIO * SUPERSAMPLE as f32;
    let w = hi as f32;
    let h = hi as f32;
    let draw_ring = size >= RING_MIN_SIZE;
    // Ring thickness: 1 output pixel -> SUPERSAMPLE source pixels.
    let ring_inner = SUPERSAMPLE as f32;

    for y in 0..hi {
        for x in 0..hi {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dist_outside = rounded_rect_sdf(fx, fy, w, h, corner);
            if dist_outside > 0.0 {
                continue;
            }
            // Anti-diagonal split: the line x + y = w runs top-right
            // to bottom-left. Below/right of it is `added_inline`.
            let rgb = if fx + fy >= w {
                ADDED_INLINE
            } else {
                REMOVED_INLINE
            };
            let idx = ((y * hi + x) * 4) as usize;
            hi_buf[idx] = rgb[0];
            hi_buf[idx + 1] = rgb[1];
            hi_buf[idx + 2] = rgb[2];
            hi_buf[idx + 3] = 0xFF;

            if draw_ring && dist_outside > -ring_inner {
                // Blend the hairline ring over the fill in the
                // oversampled buffer; box-filtering then turns it
                // into a sub-pixel wide ring in the final image.
                blend_over(&mut hi_buf[idx..idx + 4], RING_INK, RING_ALPHA);
            }
        }
    }

    box_downsample(&hi_buf, hi, size)
}

/// Signed distance from `(x, y)` to the outside of a rounded
/// rectangle spanning `(0, 0)-(w, h)` with corner radius `r`.
/// Negative inside, positive outside.
fn rounded_rect_sdf(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let dx = (x - cx).abs() - (cx - r);
    let dy = (y - cy).abs() - (cy - r);
    let outside = (dx.max(0.0)).hypot(dy.max(0.0));
    let inside = dx.max(dy).min(0.0);
    outside + inside - r
}

fn blend_over(dst: &mut [u8], rgb: [u8; 3], alpha: f32) {
    for i in 0..3 {
        let base = dst[i] as f32;
        let top = rgb[i] as f32;
        dst[i] = (base * (1.0 - alpha) + top * alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

/// Downsample `src` (hi x hi RGBA, top-down) to `out x out` using a
/// simple unweighted box filter over SUPERSAMPLE x SUPERSAMPLE cells.
fn box_downsample(src: &[u8], hi: u32, out: u32) -> Vec<u8> {
    let block = hi / out;
    let block_area = block * block;
    let mut dst = vec![0u8; (out * out * 4) as usize];
    for oy in 0..out {
        for ox in 0..out {
            let mut sum = [0u32; 4];
            for dy in 0..block {
                for dx in 0..block {
                    let sx = ox * block + dx;
                    let sy = oy * block + dy;
                    let idx = ((sy * hi + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += src[idx + c] as u32;
                    }
                }
            }
            let didx = ((oy * out + ox) * 4) as usize;
            for c in 0..4 {
                dst[didx + c] = (sum[c] / block_area) as u8;
            }
        }
    }
    dst
}
