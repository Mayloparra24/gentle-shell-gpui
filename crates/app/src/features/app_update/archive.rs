//! Release archives: safe zip and `.tar.gz` extraction (no escaping entries
//! or links, no Finder metadata) and the payload each platform installs.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};

use super::BINARY;

/// `__MACOSX/…` and `._name` AppleDouble entries are Finder metadata, not
/// part of the app; extracting them would break the bundle's seal.
fn is_resource_fork(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name == "__MACOSX" || name.starts_with("._")
        }
        _ => false,
    })
}

/// Whether a symlink at `link` (relative to the extraction root) pointing to
/// `target` resolves inside the root.
fn link_stays_inside(link: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = 0;
    let base = link.parent().into_iter().flat_map(Path::components);
    for component in base.chain(target.components()) {
        match component {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => depth -= 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Extract a zip, keeping Unix modes and symlinks (a `.app` needs both).
pub fn extract_zip(archive: &Path, dest: &Path) -> io::Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(io::Error::other)?;
    fs::create_dir_all(dest)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(io::Error::other)?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| invalid("the archive has an entry outside its folder"))?;
        if is_resource_fork(&rel) {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        if entry.is_symlink() {
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            if !link_stays_inside(&rel, Path::new(&target)) {
                return Err(invalid(format!(
                    "the archive links {} outside its folder",
                    rel.display()
                )));
            }
            symlink(Path::new(&target), &out)?;
            continue;
        }
        io::copy(&mut entry, &mut fs::File::create(&out)?)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> io::Result<()> {
    Err(invalid(format!(
        "{} is a symlink; this platform's archive should not have one",
        link.display()
    )))
}

/// Extract a `.tar.gz` (the tar crate refuses entries escaping `dest`).
pub fn extract_tar_gz(archive: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?)).unpack(dest)
}

pub(super) fn extract(archive: &Path, dest: &Path) -> io::Result<()> {
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with(".tar.gz") {
        extract_tar_gz(archive, dest)
    } else {
        extract_zip(archive, dest)
    }
}

/// What to install from an extracted archive, per the release layout.
pub(super) fn find_payload(os: &str, root: &Path, version: &str) -> io::Result<PathBuf> {
    let payload = match os {
        "macos" => {
            let apps: Vec<PathBuf> = fs::read_dir(root)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "app"))
                .collect();
            let [app] = apps.as_slice() else {
                return Err(invalid("the archive should hold exactly one .app"));
            };
            if !app.join("Contents").join("MacOS").join(BINARY).is_file() {
                return Err(invalid(format!(
                    "{} has no Contents/MacOS/{BINARY}",
                    app.display()
                )));
            }
            app.clone()
        }
        "windows" => root.join(format!("{BINARY}.exe")),
        _ => root.join(format!("{BINARY}-{version}")).join(BINARY),
    };
    if payload.exists() {
        Ok(payload)
    } else {
        Err(invalid(format!(
            "the archive has no {}",
            payload.strip_prefix(root).unwrap_or(&payload).display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    #[cfg(target_os = "macos")]
    use super::super::test_support::fake_bundle;
    use super::super::test_support::temp_dir;
    use super::*;

    #[test]
    fn extracts_zip_with_modes_and_links() {
        use zip::write::SimpleFileOptions;
        let dir = temp_dir("zip");
        let archive = dir.join("a.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options = SimpleFileOptions::default();
        zip.add_directory("Gentle Shell.app/Contents/MacOS/", options)
            .unwrap();
        zip.start_file(
            "Gentle Shell.app/Contents/MacOS/gentle-shell-gpui",
            options.unix_permissions(0o755),
        )
        .unwrap();
        zip.write_all(b"binary").unwrap();
        // A `.app` carries symlinks, but only Unix builds extract them; off
        // Unix a symlink is refused (see `refuses_symlinks_off_unix`).
        #[cfg(unix)]
        zip.add_symlink("Gentle Shell.app/Contents/Current", "MacOS", options)
            .unwrap();
        zip.start_file("__MACOSX/._Gentle Shell.app", options)
            .unwrap();
        zip.write_all(b"fork").unwrap();
        zip.finish().unwrap();

        let out = dir.join("out");
        extract_zip(&archive, &out).unwrap();
        let binary = out.join("Gentle Shell.app/Contents/MacOS/gentle-shell-gpui");
        assert_eq!(fs::read(&binary).unwrap(), b"binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
                0o755
            );
            let link = out.join("Gentle Shell.app/Contents/Current");
            assert_eq!(fs::read_link(&link).unwrap(), Path::new("MacOS"));
        }
        assert!(!out.join("__MACOSX").exists());
        assert_eq!(
            find_payload("macos", &out, "0.2.0").unwrap(),
            out.join("Gentle Shell.app")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Windows release zips hold only the exe, so a symlink entry there is
    /// refused instead of being written as something else.
    #[cfg(not(unix))]
    #[test]
    fn refuses_symlinks_off_unix() {
        use zip::write::SimpleFileOptions;
        let dir = temp_dir("zip-link");
        let archive = dir.join("a.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.add_symlink("link", "target", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        let error = extract_zip(&archive, &dir.join("out")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The release zips are made with `ditto -c -k --keepParent`.
    #[cfg(target_os = "macos")]
    #[test]
    fn extracts_ditto_zip() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = temp_dir("ditto");
        let bundle = fake_bundle(&dir.join("src"), "Gentle Shell.app", "binary");
        let binary = bundle.join("Contents/MacOS").join(BINARY);
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("MacOS", bundle.join("Contents/Current")).unwrap();
        let archive = dir.join("a.zip");
        let status = pi_rpc::command("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&bundle)
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        let out = dir.join("out");
        extract_zip(&archive, &out).unwrap();
        let app = find_payload("macos", &out, "0.2.0").unwrap();
        let extracted = app.join("Contents/MacOS").join(BINARY);
        assert_eq!(fs::read_to_string(&extracted).unwrap(), "binary");
        assert_eq!(
            fs::metadata(&extracted).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_link(app.join("Contents/Current")).unwrap(),
            Path::new("MacOS")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_zip_links_outside() {
        use zip::write::SimpleFileOptions;
        let dir = temp_dir("zip-escape");
        let archive = dir.join("a.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.add_symlink("app/evil", "../../outside", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(extract_zip(&archive, &dir.join("out")).is_err());
        assert!(link_stays_inside(Path::new("a/b/link"), Path::new("../c")));
        assert!(!link_stays_inside(Path::new("a/link"), Path::new("/etc")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extracts_tar_gz() {
        let dir = temp_dir("tar");
        let archive = dir.join("a.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                fs::File::create(&archive).unwrap(),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(
                &mut header,
                "gentle-shell-gpui-0.2.0/gentle-shell-gpui",
                &b"binary"[..],
            )
            .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("out");
        extract(&archive, &out).unwrap();
        let payload = find_payload("linux", &out, "0.2.0").unwrap();
        assert_eq!(fs::read(&payload).unwrap(), b"binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&payload).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        assert!(find_payload("linux", &out, "0.3.0").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
