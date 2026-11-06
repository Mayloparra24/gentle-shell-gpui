//! Windows: embeds the app icon (resource id 1, which GPUI loads for its
//! window class) and version info into the exe. A no-op on other targets.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            windows_resources();
        }
    }
}

#[cfg(windows)]
fn windows_resources() {
    use std::path::PathBuf;

    let env = |key: &str| std::env::var(key).unwrap_or_else(|_| panic!("{key} is not set"));
    let icon = PathBuf::from(env("CARGO_MANIFEST_DIR")).join("../../packaging/icons/icon.ico");
    let icon = icon
        .canonicalize()
        .expect("packaging/icons/icon.ico is missing");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env("CARGO_PKG_VERSION");
    let numeric = format!(
        "{},{},{},0",
        env("CARGO_PKG_VERSION_MAJOR"),
        env("CARGO_PKG_VERSION_MINOR"),
        env("CARGO_PKG_VERSION_PATCH")
    );
    // `canonicalize` yields a `\\?\` verbatim path, which rc.exe rejects.
    let icon = icon.display().to_string();
    let icon = icon
        .strip_prefix(r"\\?\")
        .unwrap_or(&icon)
        .replace('\\', "\\\\");
    // Numeric constants instead of <winver.h> (VOS_NT_WINDOWS32, VFT_APP), so
    // rc.exe needs no SDK include path.
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Gentle Shell"
      VALUE "FileDescription", "Gentle Shell"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "gentle-shell-gpui"
      VALUE "OriginalFilename", "gentle-shell-gpui.exe"
      VALUE "ProductName", "Gentle Shell"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "MIT License"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = PathBuf::from(env("OUT_DIR")).join("gentle-shell-gpui.rc");
    std::fs::write(&path, rc).expect("writing the resource script");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_optional()
        .expect("compiling Windows resources");
}
