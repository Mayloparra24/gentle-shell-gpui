//! Installing a release: what the running app is (macOS bundle or Windows /
//! Linux executable), the per-OS in-place replacement, and the blocking
//! download → verify → extract → swap pipeline.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::archive::{extract, find_payload};
use super::network::{download, fetch_text};
use super::release::{Release, platform_asset, verify_digest};
use super::{BINARY, Phase, Problem, SUMS, fail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// macOS: the running `X.app`.
    Bundle(PathBuf),
    /// Windows and Linux: the running executable.
    Exe(PathBuf),
}

impl Target {
    /// What a relaunch opens.
    pub fn path(&self) -> &Path {
        match self {
            Target::Bundle(path) | Target::Exe(path) => path,
        }
    }
}

/// `…/X.app/Contents/MacOS/<exe>` → `…/X.app`.
pub(super) fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// `…/target/[<triple>/]{debug,release,dist}/<exe>`: a `cargo` build, not a
/// release (`<triple>` for cross builds such as the packaging scripts make).
fn in_cargo_target(exe: &Path) -> bool {
    let profile = exe.parent();
    let named =
        |p: Option<&Path>, name: &str| p.and_then(Path::file_name).is_some_and(|n| n == name);
    let is_profile = profile
        .and_then(Path::file_name)
        .is_some_and(|n| n == "debug" || n == "release" || n == "dist");
    let parent = profile.and_then(Path::parent);
    is_profile && (named(parent, "target") || named(parent.and_then(Path::parent), "target"))
}

/// What an update would replace for the app running from `exe`.
pub(super) fn install_target(exe: &Path, os: &str) -> Result<Target, Problem> {
    let build_dir = || {
        Problem::with_releases("Running from a build directory; download the release from GitHub.")
    };
    match os {
        "macos" => {
            let bundle = bundle_of(exe).ok_or_else(build_dir)?;
            if bundle
                .components()
                .any(|c| c.as_os_str() == "AppTranslocation")
            {
                return Err(Problem::new(
                    "macOS is running Gentle Shell from a temporary read-only copy. Move Gentle Shell.app to Applications, open it from there, then update.",
                ));
            }
            Ok(Target::Bundle(bundle))
        }
        "windows" | "linux" if in_cargo_target(exe) => Err(build_dir()),
        "windows" | "linux" => Ok(Target::Exe(exe.to_path_buf())),
        other => Err(Problem::with_releases(format!(
            "Self-update isn't available on {other}; download the release from GitHub."
        ))),
    }
}

fn not_writable(dir: &Path) -> Problem {
    Problem::with_releases(format!(
        "Gentle Shell can't write to {}. Move the app to a folder you own (e.g. ~/Applications) or install the update from GitHub yourself.",
        dir.display()
    ))
}

fn ensure_writable(dir: &Path) -> Result<(), Problem> {
    let probe = dir.join(format!(".{BINARY}-write-test-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(not_writable(dir)),
    }
}

/// `dir/name` → `dir/.name.<tag>-<pid>`: hidden, same directory (same volume).
fn sibling(path: &Path, tag: &str) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{tag}-{}", std::process::id()))
}

/// Move `new` next to `target` so the swap is a same-volume rename.
fn stage_beside(new: &Path, target: &Path) -> io::Result<PathBuf> {
    let staged = sibling(target, "new");
    let _ = fs::remove_dir_all(&staged);
    if fs::rename(new, &staged).is_err() {
        // Another volume: copy, keeping symlinks, modes and signatures.
        let status = pi_rpc::command("ditto").arg(new).arg(&staged).status()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&staged);
            return Err(io::Error::other(format!("ditto failed ({status})")));
        }
    }
    Ok(staged)
}

/// `old` → backup, `new` → `old`, backup removed. If the second rename fails
/// the original is put back, so a failed swap leaves the app as it was.
pub(super) fn swap_dir(old: &Path, new: &Path) -> io::Result<()> {
    let backup = sibling(old, "old");
    let _ = fs::remove_dir_all(&backup);
    fs::rename(old, &backup)?;
    if let Err(error) = fs::rename(new, old) {
        fs::rename(&backup, old)?;
        return Err(error);
    }
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

/// Linux: a same-directory copy of `new`, given `target`'s mode, renamed
/// over `target` (the running binary keeps its open inode).
pub(super) fn replace_file(target: &Path, new: &Path) -> io::Result<()> {
    let staged = sibling(target, "new");
    let result = fs::copy(new, &staged)
        .and_then(|_| fs::set_permissions(&staged, fs::metadata(target)?.permissions()))
        .and_then(|()| fs::rename(&staged, target));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// `gentle-shell-gpui.exe` → `gentle-shell-gpui.exe.old`.
pub(super) fn old_exe(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Windows: a running exe can be renamed but not overwritten, so it moves to
/// `*.old` (deleted on the next start) and the new one takes its path.
pub(super) fn replace_running_exe(target: &Path, new: &Path) -> io::Result<()> {
    let old = old_exe(target);
    let _ = fs::remove_file(&old);
    fs::rename(target, &old)?;
    let moved = fs::rename(new, target).or_else(|_| fs::copy(new, target).map(drop));
    if let Err(error) = moved {
        let _ = fs::remove_file(target);
        fs::rename(&old, target)?;
        return Err(error);
    }
    Ok(())
}

fn replace(target: &Target, payload: &Path) -> Result<(), Problem> {
    let dir = target
        .path()
        .parent()
        .unwrap_or(target.path())
        .to_path_buf();
    let denied = |e: io::Error, what: &str| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            not_writable(&dir)
        } else {
            Problem::new(format!("{what}: {e}"))
        }
    };
    match target {
        Target::Bundle(bundle) => {
            let staged = stage_beside(payload, bundle)
                .map_err(|e| denied(e, "Couldn't stage the update"))?;
            if let Err(e) = swap_dir(bundle, &staged) {
                let _ = fs::remove_dir_all(&staged);
                return Err(denied(e, "Couldn't replace the app"));
            }
            // Downloads made by this app carry no quarantine, but a copied
            // bundle might; failing to strip it is harmless.
            let _ = pi_rpc::command("xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(bundle)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            Ok(())
        }
        Target::Exe(exe) if cfg!(windows) => {
            replace_running_exe(exe, payload).map_err(|e| denied(e, "Couldn't replace the app"))
        }
        Target::Exe(exe) => {
            replace_file(exe, payload).map_err(|e| denied(e, "Couldn't replace the app"))
        }
    }
}

/// Blocking: download, verify, extract and swap in `release`; returns what a
/// relaunch should open.
pub(super) fn install_blocking(
    release: &Release,
    progress: &mut dyn FnMut(Phase),
) -> Result<PathBuf, Problem> {
    let exe = std::env::current_exe().map_err(fail("Couldn't locate the running app"))?;
    let exe = dunce::canonicalize(&exe).unwrap_or(exe);
    let target = install_target(&exe, std::env::consts::OS)?;
    ensure_writable(target.path().parent().unwrap_or(target.path()))?;
    let asset = platform_asset(release).ok_or_else(|| {
        Problem::with_releases(format!(
            "Release {} has no download for {}-{}.",
            release.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let sums = release.asset(SUMS).ok_or_else(|| {
        Problem::new(format!(
            "Release {} has no {SUMS}; refusing to install an unverified download.",
            release.version
        ))
    })?;
    let work = std::env::temp_dir().join(format!("{BINARY}-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(fail("Couldn't create a download folder"))?;
    let result = (|| {
        let sums = fetch_text(&sums.url)?;
        let archive = work.join(&asset.name);
        let actual = download(&asset.url, &archive, asset.size, progress)?;
        verify_digest(&asset.name, &actual, &sums)?;
        progress(Phase::Installing);
        let unpacked = work.join("unpacked");
        extract(&archive, &unpacked).map_err(fail("Couldn't extract the update"))?;
        let payload = find_payload(std::env::consts::OS, &unpacked, &release.version)
            .map_err(fail("The update archive is not what was expected"))?;
        replace(&target, &payload)?;
        Ok(target.path().to_path_buf())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::super::RELEASES_URL;
    use super::super::test_support::{fake_bundle, temp_dir};
    use super::*;

    #[test]
    fn finds_bundles_and_targets() {
        let exe = Path::new("/Applications/Gentle Shell.app/Contents/MacOS/gentle-shell-gpui");
        assert_eq!(
            bundle_of(exe).as_deref(),
            Some(Path::new("/Applications/Gentle Shell.app"))
        );
        assert_eq!(
            install_target(exe, "macos"),
            Ok(Target::Bundle("/Applications/Gentle Shell.app".into()))
        );
        let dev = Path::new("/src/gentle-shell-gpui/target/debug/gentle-shell-gpui");
        assert_eq!(bundle_of(dev), None);
        let problem = install_target(dev, "macos").unwrap_err();
        assert!(problem.message.contains("build directory"));
        assert_eq!(problem.link.as_deref(), Some(RELEASES_URL));
        assert!(install_target(dev, "linux").is_err());
        // The packaging profile, plain and cross-compiled, is a build dir too.
        for build in [
            "/src/g/target/dist/gentle-shell-gpui",
            "/src/g/target/x86_64-unknown-linux-gnu/dist/gentle-shell-gpui",
            "/src/g/target/x86_64-pc-windows-msvc/release/gentle-shell-gpui.exe",
        ] {
            assert!(
                install_target(Path::new(build), "linux").is_err(),
                "{build}"
            );
        }
        assert!(bundle_of(Path::new("/x/App/Contents/MacOS/bin")).is_none());
        let translocated = Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/Gentle Shell.app/Contents/MacOS/gentle-shell-gpui",
        );
        assert!(install_target(translocated, "macos").is_err());
        let installed = Path::new("/opt/gentle/gentle-shell-gpui");
        assert_eq!(
            install_target(installed, "linux"),
            Ok(Target::Exe(installed.into()))
        );
    }

    #[test]
    fn swaps_bundles() {
        let dir = temp_dir("swap");
        let old = fake_bundle(&dir, "Gentle Shell.app", "old");
        let new = fake_bundle(&dir.join("download"), "Gentle Shell.app", "new");
        let staged = stage_beside(&new, &old).unwrap();
        assert_eq!(staged.parent(), old.parent());
        swap_dir(&old, &staged).unwrap();
        assert_eq!(
            fs::read_to_string(old.join("Contents/MacOS").join(BINARY)).unwrap(),
            "new"
        );
        assert!(!staged.exists());
        // Only the bundle and the emptied download folder remain: no backup.
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["Gentle Shell.app", "download"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_swap_keeps_the_original() {
        let dir = temp_dir("swap-fail");
        let old = fake_bundle(&dir, "Gentle Shell.app", "old");
        assert!(swap_dir(&old, &dir.join("missing.app")).is_err());
        assert_eq!(
            fs::read_to_string(old.join("Contents/MacOS").join(BINARY)).unwrap(),
            "old"
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaces_files_keeping_mode() {
        let dir = temp_dir("replace");
        let target = dir.join(BINARY);
        fs::write(&target, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let new = dir.join("new-binary");
        fs::write(&new, "new").unwrap();
        replace_file(&target, &new).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaces_running_exe_via_old() {
        let dir = temp_dir("exe");
        let target = dir.join("gentle-shell-gpui.exe");
        fs::write(&target, "old").unwrap();
        let new = dir.join("download.exe");
        fs::write(&new, "new").unwrap();
        replace_running_exe(&target, &new).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(fs::read_to_string(old_exe(&target)).unwrap(), "old");
        assert_eq!(old_exe(&target), dir.join("gentle-shell-gpui.exe.old"));
        // A missing download leaves the original in place.
        fs::remove_file(old_exe(&target)).unwrap();
        assert!(replace_running_exe(&target, &dir.join("missing.exe")).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        fs::remove_dir_all(&dir).unwrap();
    }
}
