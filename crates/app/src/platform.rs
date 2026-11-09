//! OS-specific window and Dock integration.

/// The window icon; GPUI only applies it on X11 (Windows reads the exe's
/// embedded icon, macOS the bundle's `AppIcon.icns`).
#[cfg(target_os = "linux")]
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let png = include_bytes!("../../../packaging/icons/icon-512.png");
    let icon = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    // 128 px keeps the `_NET_WM_ICON` property well under X11 request limits.
    let icon = image::imageops::resize(
        &icon.into_rgba8(),
        128,
        128,
        image::imageops::FilterType::Lanczos3,
    );
    Some(std::sync::Arc::new(icon))
}

/// macOS only reads `AppIcon.icns` for an app launched as a `.app` bundle;
/// a bare binary (`cargo run`, `target/release/…`) would show the generic
/// "exec" icon in the Dock, so set the rose at runtime too.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    // Inside the bundle LaunchServices already shows `AppIcon.icns`.
    let bundled = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
    let Some(mtm) = MainThreadMarker::new().filter(|_| !bundled) else {
        return;
    };
    // 512 px is the largest the Dock draws (256 pt at 2x).
    let png = include_bytes!("../../../packaging/icons/icon-512.png");
    let data = NSData::with_bytes(png);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        let app = NSApplication::sharedApplication(mtm);
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}
