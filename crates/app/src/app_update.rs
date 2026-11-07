//! Updates for this app itself, from its GitHub releases.
//!
//! The latest release is checked on startup (stable builds only, cached for
//! six hours) and on demand from Configuration → App. Nothing is installed
//! until the user clicks: the platform archive is downloaded, verified against
//! the release's `SHA256SUMS`, extracted and swapped in place of the running
//! app, and "Restart now" relaunches it once this process has quit.
//!
//! The cache and the "Skip this version" choice live in
//! `<data dir>/gentle-shell-gpui/app-update.json`.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{App, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

use crate::updates::{is_newer, stable_parts};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const RELEASES_URL: &str = "https://github.com/ismaelosuna7824/gentle-shell-gpui/releases";
const LATEST_API: &str =
    "https://api.github.com/repos/ismaelosuna7824/gentle-shell-gpui/releases/latest";
const USER_AGENT: &str = concat!("gentle-shell-gpui/", env!("CARGO_PKG_VERSION"));
const SUMS: &str = "SHA256SUMS";
/// The executable inside every archive (`.exe` on Windows).
const BINARY: &str = "gentle-shell-gpui";
/// A fresh answer is trusted for six hours.
const TTL_MS: u64 = 6 * 60 * 60 * 1000;
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Releases and versions

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// `X.Y.Z`, without the tag's `v`.
    pub version: String,
    pub notes_url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// Builds without a stable `X.Y.Z` (or `0.0.0`) are development builds: no
/// automatic checks, and any published release counts as newer.
pub fn is_dev(version: &str) -> bool {
    version == "0.0.0" || stable_parts(version).is_none()
}

/// Whether `latest` should replace `current`.
pub fn is_update(current: &str, latest: &str) -> bool {
    let Some(latest) = stable_parts(latest) else {
        return false;
    };
    match stable_parts(current) {
        Some(current_parts) if !is_dev(current) => is_newer(latest, current_parts),
        _ => true,
    }
}

/// The release archive for `(os, arch)` as `std::env::consts` names them.
pub fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let suffix = match (os, arch) {
        ("macos", "aarch64" | "x86_64") => format!("macos-{arch}.zip"),
        ("windows", "x86_64") => "windows-x86_64.zip".to_string(),
        ("linux", "x86_64") => "linux-x86_64.tar.gz".to_string(),
        _ => return None,
    };
    Some(format!("{BINARY}-{version}-{suffix}"))
}

fn platform_asset(release: &Release) -> Option<&Asset> {
    let name = asset_name(
        &release.version,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    release.asset(&name)
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// A GitHub `releases/latest` answer; only published stable `vX.Y.Z` tags count.
pub fn parse_release(json: &str) -> Option<Release> {
    let api: ApiRelease = serde_json::from_str(json).ok()?;
    let version = api.tag_name.strip_prefix('v').unwrap_or(&api.tag_name);
    if api.draft || api.prerelease || stable_parts(version).is_none() {
        return None;
    }
    Some(Release {
        version: version.to_string(),
        notes_url: api.html_url,
        assets: api
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    })
}

/// `SHA256SUMS` lines: `<64 hex>  <name>` (a `*` binary marker is accepted).
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name).trim_end();
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (hash.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

pub fn expected_digest(sums: &str, name: &str) -> Option<String> {
    parse_sums(sums)
        .into_iter()
        .find_map(|(hash, file)| (file == name).then_some(hash))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Refuse anything but an exact match with the published digest.
pub fn verify_digest(name: &str, actual: &str, sums: &str) -> Result<(), Problem> {
    let expected = expected_digest(sums, name).ok_or_else(|| {
        Problem::new(format!(
            "SHA256SUMS has no entry for {name}; refusing to install an unverified download."
        ))
    })?;
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(Problem::new(format!(
            "{name} failed verification (sha256 {actual}, expected {expected}); nothing was installed."
        )))
    }
}

// ---------------------------------------------------------------------------
// Cache

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Store {
    #[serde(default)]
    checked_at: u64,
    #[serde(default)]
    latest: Option<Release>,
    #[serde(default)]
    skipped: Option<String>,
}

fn store_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("gentle-shell-gpui")
            .join("app-update.json"),
    )
}

fn read_store(path: &Path) -> Store {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_store(path: &Path, store: &Store) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(store)? + "\n")?;
    fs::rename(&tmp, path)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn cached(store: &Store, now: u64) -> Option<&Release> {
    store
        .latest
        .as_ref()
        .filter(|_| now.saturating_sub(store.checked_at) < TTL_MS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Available(Release),
    /// Newer, but the user chose to skip it (automatic checks only).
    Skipped(String),
}

/// A manual check shows a skipped version again; the startup check does not.
pub fn decide(current: &str, release: Release, skipped: Option<&str>, manual: bool) -> Outcome {
    if !is_update(current, &release.version) {
        Outcome::UpToDate
    } else if !manual && skipped == Some(release.version.as_str()) {
        Outcome::Skipped(release.version)
    } else {
        Outcome::Available(release)
    }
}

// ---------------------------------------------------------------------------
// Network

fn agent(download: bool) -> ureq::Agent {
    let config = ureq::Agent::config_builder().http_status_as_error(false);
    let config = if download {
        config
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(Duration::from_secs(60)))
    } else {
        config.timeout_global(Some(CHECK_TIMEOUT))
    };
    config.build().into()
}

fn get(agent: &ureq::Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>, Problem> {
    let response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| Problem::new(format!("Couldn't reach GitHub: {e}")))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(response)
    } else {
        Err(Problem::new(format!(
            "GitHub answered HTTP {status} for {url}"
        )))
    }
}

fn fetch_release() -> Result<Release, Problem> {
    let mut response = agent(false)
        .get(LATEST_API)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| Problem::new(format!("Couldn't reach GitHub: {e}")))?;
    match response.status().as_u16() {
        200..=299 => {}
        404 => return Err(Problem::new("No release has been published yet.")),
        403 | 429 => {
            return Err(Problem::new(
                "GitHub's rate limit was hit; try again in a while.",
            ));
        }
        status => return Err(Problem::new(format!("GitHub answered HTTP {status}."))),
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| Problem::new(format!("Couldn't read GitHub's answer: {e}")))?;
    parse_release(&body)
        .ok_or_else(|| Problem::new("GitHub's latest release is not a stable vX.Y.Z release."))
}

/// Blocking: the startup (cached) or manual (fresh) answer.
fn check_blocking(manual: bool) -> Result<Outcome, Problem> {
    let path = store_path();
    let mut store = path.as_deref().map(read_store).unwrap_or_default();
    let now = now_ms();
    let release = match cached(&store, now).filter(|_| !manual) {
        Some(release) => release.clone(),
        None => {
            let release = fetch_release()?;
            store.latest = Some(release.clone());
            store.checked_at = now;
            if let Some(path) = &path {
                let _ = write_store(path, &store);
            }
            release
        }
    };
    Ok(decide(CURRENT, release, store.skipped.as_deref(), manual))
}

fn skip_blocking(version: &str) -> io::Result<()> {
    let path = store_path().ok_or_else(|| io::Error::other("no data directory"))?;
    let mut store = read_store(&path);
    store.skipped = Some(version.to_string());
    write_store(&path, &store)
}

/// Stream `url` into `dest`, hashing on the way; returns the sha256 hex.
fn download(
    url: &str,
    dest: &Path,
    size: u64,
    progress: &mut dyn FnMut(Phase),
) -> Result<String, Problem> {
    use sha2::{Digest as _, Sha256};
    let mut response = get(&agent(true), url)?;
    let total = response
        .body()
        .content_length()
        .or((size > 0).then_some(size));
    let mut reader = response.body_mut().as_reader();
    let mut file = fs::File::create(dest).map_err(fail("Couldn't create the download file"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut reported) = (0u64, 0u64);
    progress(Phase::Downloading(0, total));
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| Problem::new(format!("The download was interrupted: {e}")))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if total.is_some_and(|t| done > t) {
            return Err(Problem::new(
                "The download is larger than announced; stopped.",
            ));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(fail("Couldn't write the download"))?;
        if done - reported >= 256 * 1024 {
            reported = done;
            progress(Phase::Downloading(done, total));
        }
    }
    file.sync_all()
        .map_err(fail("Couldn't write the download"))?;
    progress(Phase::Downloading(done, total));
    Ok(hex(&hasher.finalize()))
}

fn fetch_text(url: &str) -> Result<String, Problem> {
    get(&agent(false), url)?
        .body_mut()
        .read_to_string()
        .map_err(|e| Problem::new(format!("Couldn't read {SUMS}: {e}")))
}

// ---------------------------------------------------------------------------
// Archives

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

fn extract(archive: &Path, dest: &Path) -> io::Result<()> {
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with(".tar.gz") {
        extract_tar_gz(archive, dest)
    } else {
        extract_zip(archive, dest)
    }
}

/// What to install from an extracted archive, per the release layout.
pub fn find_payload(os: &str, root: &Path, version: &str) -> io::Result<PathBuf> {
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

// ---------------------------------------------------------------------------
// Install targets and replacement

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    /// Somewhere to go instead (the releases page).
    pub link: Option<String>,
}

impl Problem {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            link: None,
        }
    }

    fn with_releases(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            link: Some(RELEASES_URL.to_string()),
        }
    }
}

fn fail(what: &'static str) -> impl FnOnce(io::Error) -> Problem {
    move |e| Problem::new(format!("{what}: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
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
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
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
pub fn install_target(exe: &Path, os: &str) -> Result<Target, Problem> {
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
        let status = Command::new("ditto").arg(new).arg(&staged).status()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&staged);
            return Err(io::Error::other(format!("ditto failed ({status})")));
        }
    }
    Ok(staged)
}

/// `old` → backup, `new` → `old`, backup removed. If the second rename fails
/// the original is put back, so a failed swap leaves the app as it was.
pub fn swap_dir(old: &Path, new: &Path) -> io::Result<()> {
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
pub fn replace_file(target: &Path, new: &Path) -> io::Result<()> {
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
fn old_exe(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Windows: a running exe can be renamed but not overwritten, so it moves to
/// `*.old` (deleted on the next start) and the new one takes its path.
pub fn replace_running_exe(target: &Path, new: &Path) -> io::Result<()> {
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
            let _ = Command::new("xattr")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Downloading(u64, Option<u64>),
    Installing,
}

/// Blocking: download, verify, extract and swap in `release`; returns what a
/// relaunch should open.
fn install_blocking(
    release: &Release,
    progress: &mut dyn FnMut(Phase),
) -> Result<PathBuf, Problem> {
    let exe = std::env::current_exe().map_err(fail("Couldn't locate the running app"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
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

/// A detached helper that waits for `pid` to exit, then opens `target`. It
/// starts after this app released the single-instance guard, so the new app
/// becomes the primary instead of forwarding to the dying one.
fn relaunch_command(target: &Path, pid: u32) -> Command {
    if cfg!(windows) {
        let quoted = target.display().to_string().replace('\'', "''");
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!(
                "Wait-Process -Id {pid} -Timeout 60 -ErrorAction SilentlyContinue; Start-Process -FilePath '{quoted}'"
            ),
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        command
    } else {
        let launch = if cfg!(target_os = "macos") {
            "exec open -n \"$1\""
        } else {
            "exec \"$1\""
        };
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "i=0; while kill -0 {pid} 2>/dev/null && [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; {launch}"
            ))
            .arg("sh")
            .arg(target);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        command
    }
}

fn spawn_relaunch(target: &Path) -> io::Result<()> {
    relaunch_command(target, std::process::id())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// The previous exe a Windows update left behind.
fn remove_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(old_exe(&exe));
    }
}

// ---------------------------------------------------------------------------
// App state

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No answer yet (development builds don't check on startup).
    Idle,
    Checking,
    UpToDate,
    Skipped(String),
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
        total: Option<u64>,
    },
    Installing(Release),
    /// Installed; "Restart now" opens `relaunch`.
    Ready {
        release: Release,
        relaunch: PathBuf,
    },
    /// The last check or install failed; `release` can be retried.
    Failed {
        problem: Problem,
        release: Option<Release>,
    },
}

impl Status {
    /// Checking, installing, or waiting for the restart: no new check.
    pub fn busy(&self) -> bool {
        matches!(
            self,
            Status::Checking
                | Status::Downloading { .. }
                | Status::Installing(_)
                | Status::Ready { .. }
        )
    }

    /// A release the user can install right now.
    pub fn installable(&self) -> Option<&Release> {
        match self {
            Status::Available(release)
            | Status::Failed {
                release: Some(release),
                ..
            } => Some(release),
            _ => None,
        }
    }
}

pub struct AppUpdate {
    pub status: Status,
    _job: Task<()>,
}

impl Global for AppUpdate {}

fn set_status(status: Status, cx: &mut App) {
    cx.update_global::<AppUpdate, _>(|update, _| update.status = status);
}

pub fn status(cx: &App) -> &Status {
    &cx.global::<AppUpdate>().status
}

/// The title-bar pill: `(label, tooltip)` while an update waits for the user.
pub fn badge(cx: &App) -> Option<(&'static str, String)> {
    match &cx.try_global::<AppUpdate>()?.status {
        Status::Available(r) => {
            Some(("Update", format!("Gentle Shell {} is available", r.version)))
        }
        Status::Downloading { release: r, .. } | Status::Installing(r) => Some((
            "Updating…",
            format!("Installing Gentle Shell {}", r.version),
        )),
        Status::Ready { release: r, .. } => Some((
            "Restart to update",
            format!("Gentle Shell {} is installed", r.version),
        )),
        _ => None,
    }
}

/// Register the state and, for release builds, check in the background.
pub fn init(cx: &mut App) {
    cx.set_global(AppUpdate {
        status: Status::Idle,
        _job: Task::ready(()),
    });
    cx.background_executor()
        .spawn(async { remove_leftovers() })
        .detach();
    if !is_dev(CURRENT) {
        check(false, cx);
    }
}

/// Ask GitHub (or, on startup, the six-hour cache). Startup failures stay
/// quiet; a manual check reports them.
pub fn check(manual: bool, cx: &mut App) {
    if status(cx).busy() {
        return;
    }
    set_status(Status::Checking, cx);
    let job = cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { check_blocking(manual) })
            .await;
        let status = match result {
            Ok(Outcome::UpToDate) => Status::UpToDate,
            Ok(Outcome::Skipped(version)) => Status::Skipped(version),
            Ok(Outcome::Available(release)) => Status::Available(release),
            Err(problem) if manual => Status::Failed {
                problem,
                release: None,
            },
            Err(_) => Status::Idle,
        };
        cx.update(|cx| set_status(status, cx));
    });
    cx.global_mut::<AppUpdate>()._job = job;
}

/// Download & install the offered release; only ever from a user's click.
pub fn install(cx: &mut App) {
    let Some(release) = status(cx).installable().cloned() else {
        return;
    };
    let size = platform_asset(&release).map(|a| a.size).filter(|s| *s > 0);
    set_status(
        Status::Downloading {
            release: release.clone(),
            done: 0,
            total: size,
        },
        cx,
    );
    let (tx, rx) = async_channel::unbounded::<Phase>();
    let job = cx.spawn(async move |cx| {
        let work = cx.background_executor().spawn({
            let release = release.clone();
            async move {
                install_blocking(&release, &mut |phase| {
                    let _ = tx.try_send(phase);
                })
            }
        });
        while let Ok(phase) = rx.recv().await {
            let status = match phase {
                Phase::Downloading(done, total) => Status::Downloading {
                    release: release.clone(),
                    done,
                    total,
                },
                Phase::Installing => Status::Installing(release.clone()),
            };
            cx.update(|cx| set_status(status, cx));
        }
        let status = match work.await {
            Ok(relaunch) => Status::Ready { release, relaunch },
            Err(problem) => Status::Failed {
                problem,
                release: Some(release),
            },
        };
        cx.update(|cx| set_status(status, cx));
    });
    cx.global_mut::<AppUpdate>()._job = job;
}

/// "Skip this version": the startup check stays quiet until a newer one.
pub fn skip(cx: &mut App) {
    let Some(release) = status(cx).installable().cloned() else {
        return;
    };
    let status = match skip_blocking(&release.version) {
        Ok(()) => Status::Skipped(release.version),
        Err(e) => Status::Failed {
            problem: Problem::new(format!("Couldn't remember the skip: {e}")),
            release: Some(release),
        },
    };
    set_status(status, cx);
}

/// Schedule the relaunch, then quit. Workspaces are saved by the
/// workspace's `on_app_quit` hook. Quit directly: `App::dispatch_action`
/// routes through the active window, and from a click handler that window
/// is already being updated, so the action would be dropped.
pub fn restart(cx: &mut App) {
    let Status::Ready { release, relaunch } = status(cx).clone() else {
        return;
    };
    match spawn_relaunch(&relaunch) {
        Ok(()) => cx.quit(),
        Err(e) => set_status(
            Status::Failed {
                problem: Problem::new(format!(
                    "Gentle Shell {} is installed, but the restart couldn't be scheduled ({e}). Quit and reopen the app.",
                    release.version
                )),
                release: None,
            },
            cx,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gs-app-update-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            notes_url: format!("{RELEASES_URL}/tag/v{version}"),
            assets: Vec::new(),
        }
    }

    #[test]
    fn compares_versions() {
        assert!(is_update("0.1.0", "0.2.0"));
        assert!(is_update("0.9.0", "0.10.0"));
        assert!(is_update("1.2.3", "1.2.10"));
        assert!(!is_update("0.2.0", "0.2.0"));
        assert!(!is_update("0.3.0", "0.2.9"));
        assert!(!is_update("0.1.0", "0.2.0-beta.1"));
        // Development builds take any stable release.
        assert!(is_update("0.0.0", "0.1.0"));
        assert!(is_update("0.2.0-dev", "0.1.0"));
        assert!(is_dev("0.0.0") && is_dev("0.2.0-dev") && !is_dev("0.1.0"));
    }

    #[test]
    fn decides_with_skips() {
        let r = release("0.2.0");
        assert_eq!(decide("0.2.0", r.clone(), None, true), Outcome::UpToDate);
        assert_eq!(
            decide("0.1.0", r.clone(), Some("0.2.0"), false),
            Outcome::Skipped("0.2.0".into())
        );
        // A manual check shows the skipped version again.
        assert_eq!(
            decide("0.1.0", r.clone(), Some("0.2.0"), true),
            Outcome::Available(r.clone())
        );
        assert_eq!(
            decide("0.1.0", r.clone(), Some("0.1.5"), false),
            Outcome::Available(r)
        );
    }

    #[test]
    fn names_assets_per_platform() {
        let name = |os, arch| asset_name("1.2.3", os, arch);
        assert_eq!(
            name("macos", "aarch64").as_deref(),
            Some("gentle-shell-gpui-1.2.3-macos-aarch64.zip")
        );
        assert_eq!(
            name("macos", "x86_64").as_deref(),
            Some("gentle-shell-gpui-1.2.3-macos-x86_64.zip")
        );
        assert_eq!(
            name("windows", "x86_64").as_deref(),
            Some("gentle-shell-gpui-1.2.3-windows-x86_64.zip")
        );
        assert_eq!(
            name("linux", "x86_64").as_deref(),
            Some("gentle-shell-gpui-1.2.3-linux-x86_64.tar.gz")
        );
        assert_eq!(name("linux", "aarch64"), None);
        assert_eq!(name("windows", "aarch64"), None);
        assert_eq!(name("freebsd", "x86_64"), None);
    }

    #[test]
    fn parses_github_release() {
        let json = r#"{
            "tag_name": "v0.2.0", "html_url": "https://github.com/x/y/releases/tag/v0.2.0",
            "draft": false, "prerelease": false,
            "assets": [{"name": "SHA256SUMS", "browser_download_url": "https://d/SHA256SUMS", "size": 300}]
        }"#;
        let parsed = parse_release(json).unwrap();
        assert_eq!(parsed.version, "0.2.0");
        assert_eq!(parsed.asset(SUMS).unwrap().url, "https://d/SHA256SUMS");
        assert!(parse_release(&json.replace("v0.2.0\"", "v0.2.0-rc.1\"")).is_none());
        assert!(
            parse_release(&json.replace("\"prerelease\": false", "\"prerelease\": true")).is_none()
        );
    }

    #[test]
    fn parses_sums() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let text = format!(
            "{a}  gentle-shell-gpui-0.2.0-macos-aarch64.zip\n{b} *gentle-shell-gpui-0.2.0-windows-x86_64.zip\n\nnot a line\nabc  short.zip\n"
        );
        let sums = parse_sums(&text);
        assert_eq!(sums.len(), 2);
        assert_eq!(
            expected_digest(&text, "gentle-shell-gpui-0.2.0-windows-x86_64.zip"),
            Some("b".repeat(64))
        );
        assert_eq!(expected_digest(&text, "short.zip"), None);
        assert_eq!(expected_digest(&text, "missing.zip"), None);
    }

    #[test]
    fn verifies_checksums() {
        use sha2::{Digest as _, Sha256};
        let digest = hex(&Sha256::digest(b"abc"));
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let sums = format!("{}  app.zip\n", digest.to_uppercase());
        assert!(verify_digest("app.zip", &digest, &sums).is_ok());
        let wrong = hex(&Sha256::digest(b"abd"));
        assert!(verify_digest("app.zip", &wrong, &sums).is_err());
        assert!(verify_digest("other.zip", &digest, &sums).is_err());
    }

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
        let status = Command::new("ditto")
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

    fn fake_bundle(root: &Path, name: &str, body: &str) -> PathBuf {
        let bundle = root.join(name);
        let macos = bundle.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join(BINARY), body).unwrap();
        bundle
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

    #[test]
    fn caches_for_six_hours() {
        let store = Store {
            checked_at: 1_000,
            latest: Some(release("0.2.0")),
            skipped: None,
        };
        assert!(cached(&store, 1_000 + TTL_MS - 1).is_some());
        assert!(cached(&store, 1_000 + TTL_MS).is_none());
        let dir = temp_dir("store");
        let path = dir.join("app-update.json");
        write_store(&path, &store).unwrap();
        let read = read_store(&path);
        assert_eq!(read.latest, store.latest);
        assert_eq!(read.checked_at, 1_000);
        fs::remove_dir_all(&dir).unwrap();
    }
}
