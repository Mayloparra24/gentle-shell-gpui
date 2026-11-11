//! Turning a device's screen into a GPUI image. Android sends a raw
//! `screencap` (no PNG encoding on the device); iOS writes a PNG through
//! `simctl io`. Both are downscaled to the card's width before they become a
//! texture, so a 1 fps preview costs a few hundred kilobytes, not tens of
//! megabytes per frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::RenderImage;
use image::imageops::FilterType;
use image::{Frame, ImageBuffer, Rgba, RgbaImage};

use super::tools::Tools;
use gentle_core::devices::{Device, Platform, Screencap, parse_screencap};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_SHOT: AtomicU64 = AtomicU64::new(0);

/// Take one frame at `target` pixels wide. Blocking: call it from the
/// background executor. `target` comes from the card's real pixel size, so
/// the texture is as sharp as the screen can show without being oversized.
pub fn capture(tools: &Tools, device: &Device, target: usize) -> Result<Arc<RenderImage>, String> {
    match device.platform {
        Platform::Android => {
            let bytes = tools.adb_bytes(
                &["-s", &device.id, "exec-out", "screencap"],
                CAPTURE_TIMEOUT,
            )?;
            if bytes.starts_with(b"\x89PNG") {
                return from_png(&bytes, target);
            }
            let frame = parse_screencap(&bytes)
                .ok_or("The emulator returned an unexpected screenshot format.")?;
            from_raw(frame, target)
        }
        Platform::Ios => {
            let path = screenshot_path();
            let output = tools.simctl_run(
                &[
                    "io",
                    &device.id,
                    "screenshot",
                    "--type=png",
                    // Multi-display simulators (foldables, iPad with an
                    // external screen) refuse to guess; "primary" is the one
                    // the user is looking at. Single-display ones accept it.
                    "--display=primary",
                    &path.to_string_lossy(),
                ],
                CAPTURE_TIMEOUT,
            );
            let bytes = std::fs::read(&path);
            let _ = std::fs::remove_file(&path);
            output?;
            let bytes =
                bytes.map_err(|error| format!("The screenshot was not written: {error}"))?;
            from_png(&bytes, target)
        }
    }
}

fn screenshot_path() -> PathBuf {
    let shot = NEXT_SHOT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("gentle-device-{}-{shot}.png", std::process::id()))
}

fn from_png(bytes: &[u8], target: usize) -> Result<Arc<RenderImage>, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("The screenshot could not be decoded: {error}"))?;
    Ok(texture(image.to_rgba8(), target))
}

fn from_raw(frame: Screencap<'_>, target: usize) -> Result<Arc<RenderImage>, String> {
    let w = frame.width;
    let h = frame.height;
    let bpp = frame.bytes_per_pixel();
    let pixels = frame.pixels;
    if pixels.len() < w * h * bpp {
        return Err("The emulator returned a truncated screenshot.".into());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for at in (0..pixels.len()).step_by(bpp) {
        match frame.format {
            3 => rgba.extend_from_slice(&[pixels[at], pixels[at + 1], pixels[at + 2], 255]),
            4 => {
                let value = u16::from_le_bytes([pixels[at], pixels[at + 1]]);
                let r = ((value >> 11) & 0x1f) as u8;
                let g = ((value >> 5) & 0x3f) as u8;
                let b = (value & 0x1f) as u8;
                rgba.extend_from_slice(&[r << 3, g << 2, b << 3, 255]);
            }
            // RGBA/RGBX.
            1 | 2 => {
                // Raw screenshots are opaque; a stray zero alpha would make
                // the preview transparent.
                rgba.extend_from_slice(&[pixels[at], pixels[at + 1], pixels[at + 2], 255]);
            }
            // BGRA_8888.
            _ => rgba.extend_from_slice(&[pixels[at + 2], pixels[at + 1], pixels[at], 255]),
        }
    }
    let image = RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or("The emulator returned an unexpected screenshot size.")?;
    Ok(texture(image, target))
}

/// Downscale with a real filter (not nearest sampling), then turn the result
/// into the BGRA texture GPUI uploads. Matching the card's pixel width is what
/// makes text and icons crisp instead of blurry.
fn texture(image: RgbaImage, target: usize) -> Arc<RenderImage> {
    let (w, h) = image.dimensions();
    let (tw, th) = fit(w, h, target);
    let image = if (tw, th) == (w, h) {
        image
    } else {
        image::imageops::resize(&image, tw, th, FilterType::CatmullRom)
    };
    let mut bgra = Vec::with_capacity((tw * th * 4) as usize);
    for pixel in image.pixels() {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    render_image(tw as usize, th as usize, bgra)
}

/// The texture size for `target` pixels wide, preserving the aspect ratio.
/// Screens narrower than the target keep their size (no pointless upscale).
fn fit(w: u32, h: u32, target: usize) -> (u32, u32) {
    if w as usize <= target {
        return (w, h);
    }
    let tw = target.max(1) as u32;
    let th = ((h as f64 * tw as f64 / w as f64).round() as u32).max(1);
    (tw, th)
}

fn render_image(w: usize, h: usize, bgra: Vec<u8>) -> Arc<RenderImage> {
    let buffer = ImageBuffer::<Rgba<u8>, _>::from_raw(w as u32, h as u32, bgra)
        .expect("the downscaled buffer matches its dimensions");
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

#[cfg(test)]
mod tests {
    use super::{fit, from_png, from_raw, texture};
    use gentle_core::devices::parse_screencap;
    use image::{Rgba, RgbaImage};

    #[test]
    fn a_png_screenshot_becomes_a_downscaled_texture() {
        let image = RgbaImage::from_pixel(720, 1600, Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let shot = from_png(&png, 360).unwrap();
        let size = shot.size(0);
        assert_eq!((size.width.0, size.height.0), (360, 800));
    }

    #[test]
    fn the_texture_matches_the_requested_width() {
        // The default card is ~600 device pixels wide on a 2× screen.
        let image = RgbaImage::from_pixel(1206, 2622, Rgba([10, 20, 30, 255]));
        let shot = texture(image, 600);
        let size = shot.size(0);
        assert_eq!(size.width.0, 600);
        assert_eq!(size.height.0, 1304);
    }

    #[test]
    fn narrow_screens_are_not_upscaled() {
        let image = RgbaImage::from_pixel(320, 200, Rgba([1, 2, 3, 255]));
        let shot = texture(image, 600);
        let size = shot.size(0);
        assert_eq!((size.width.0, size.height.0), (320, 200));
    }

    #[test]
    fn fit_preserves_the_aspect_ratio() {
        assert_eq!(fit(1206, 2622, 600), (600, 1304));
        assert_eq!(fit(320, 200, 600), (320, 200));
        assert_eq!(fit(721, 3, 360), (360, 1));
        assert_eq!(fit(1080, 2400, 1440), (1080, 2400));
    }

    #[test]
    fn a_raw_screen_is_turned_into_bgra() {
        let mut bytes = Vec::new();
        for word in [2u32, 1, 1] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]);
        let shot = from_raw(parse_screencap(&bytes).unwrap(), 360).unwrap();
        let size = shot.size(0);
        assert_eq!((size.width.0, size.height.0), (2, 1));
        assert_eq!(shot.as_bytes(0).unwrap(), &[0, 0, 255, 255, 0, 255, 0, 255]);
    }
}
