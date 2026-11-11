//! Downloads the engine installs into the app's own toolchain folder: Node.js
//! from nodejs.org, the Engram and Maestro releases from GitHub and a Java
//! runtime from Adoptium, each verified against the checksums its release
//! publishes.

use std::fs;
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{Log, MIN_NODE, capture, parse_version, prefix_bin, toolchain_dir};
use crate::features::app_update::{expected_digest, extract_tar_gz, extract_zip};

const USER_AGENT: &str = concat!("gentle-shell-gpui/", env!("CARGO_PKG_VERSION"));
const NODE_DIST: &str = "https://nodejs.org/dist";
const ENGRAM_LATEST: &str =
    "https://api.github.com/repos/Gentleman-Programming/engram/releases/latest";
const MAESTRO_LATEST: &str = "https://api.github.com/repos/mobile-dev-inc/maestro/releases/latest";
const ADOPTIUM_LATEST: &str = "https://api.adoptium.net/v3/assets/latest";
/// The Java Maestro runs on: Temurin's LTS JRE (Maestro needs 17 or newer).
const JAVA_FEATURE: u32 = 21;

// ── Pure helpers ───────────────────────────────────────────────────────────

/// The newest Node 22 LTS in nodejs.org's `index.json` that meets the minimum.
pub fn pick_node22(index: &Value) -> Option<String> {
    index
        .as_array()?
        .iter()
        .filter(|e| e["lts"].is_string() || e["lts"] == true)
        .filter_map(|e| e["version"].as_str())
        .filter(|v| v.starts_with("v22."))
        .filter_map(|v| Some((parse_version(v)?, v)))
        .filter(|(parsed, _)| *parsed >= MIN_NODE)
        .max_by_key(|(parsed, _)| *parsed)
        .map(|(_, v)| v.to_string())
}

/// nodejs.org's archive for `(os, arch)` as `std::env::consts` names them.
pub fn node_asset(version: &str, os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return None,
    };
    let ext = if os == "win" { "zip" } else { "tar.gz" };
    Some(format!("node-{version}-{os}-{arch}.{ext}"))
}

/// Engram's release archive (GoReleaser names) for `(os, arch)`.
pub fn engram_asset(version: &str, os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => return None,
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Some(format!("engram_{version}_{os}_{arch}.{ext}"))
}

/// A GitHub release: its version (tag without `v`) and `(name, url)` assets.
pub struct Release {
    pub version: String,
    pub assets: Vec<(String, String)>,
}

pub fn parse_release(json: &str) -> Option<Release> {
    let value: Value = serde_json::from_str(json).ok()?;
    let tag = value["tag_name"].as_str()?;
    let assets = value["assets"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some((
                a["name"].as_str()?.to_string(),
                a["browser_download_url"].as_str()?.to_string(),
            ))
        })
        .collect();
    Some(Release {
        version: tag.strip_prefix('v').unwrap_or(tag).to_string(),
        assets,
    })
}

/// Adoptium's query for the latest Temurin JRE on `(os, arch)` as
/// `std::env::consts` names them.
pub fn java_runtime_query(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "mac",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "aarch64",
        "x86_64" => "x64",
        _ => return None,
    };
    Some(format!(
        "{ADOPTIUM_LATEST}/{JAVA_FEATURE}/hotspot?os={os}&architecture={arch}&image_type=jre&vendor=eclipse"
    ))
}

/// One download with its published sha256.
#[derive(Debug, PartialEq)]
pub struct Package {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

/// The package of Adoptium's first answer.
pub fn parse_java_runtime(json: &str) -> Option<Package> {
    let value: Value = serde_json::from_str(json).ok()?;
    let package = &value.as_array()?.first()?["binary"]["package"];
    Some(Package {
        name: package["name"].as_str()?.to_string(),
        url: package["link"].as_str()?.to_string(),
        sha256: package["checksum"].as_str()?.to_lowercase(),
    })
}

// ── HTTP ───────────────────────────────────────────────────────────────────

fn agent(download: bool) -> ureq::Agent {
    let config = ureq::Agent::config_builder().http_status_as_error(false);
    let config = if download {
        config
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(Duration::from_secs(60)))
    } else {
        config.timeout_global(Some(Duration::from_secs(30)))
    };
    config.build().into()
}

/// `from` > 0 asks for the rest of the file from that byte (HTTP Range).
fn get(
    agent: &ureq::Agent,
    url: &str,
    github_api: bool,
    from: u64,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut request = agent.get(url).header("User-Agent", USER_AGENT);
    if from > 0 {
        request = request.header("Range", format!("bytes={from}-"));
    }
    if github_api {
        request = request
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
    }
    let response = request
        .call()
        .map_err(|e| format!("Couldn't reach {}: {e}", host(url)))?;
    match response.status().as_u16() {
        200..=299 => Ok(response),
        403 | 429 if github_api => {
            Err("GitHub's rate limit was hit; try again in a while.".to_string())
        }
        status => Err(format!("{} answered HTTP {status} for {url}", host(url))),
    }
}

fn host(url: &str) -> &str {
    url.split("//")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(url)
}

fn get_text(url: &str, github_api: bool) -> Result<String, String> {
    get(&agent(false), url, github_api, 0)?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Couldn't read {url}: {e}"))
}

/// Stalls in a row (no new bytes between them) before a download gives up.
const DOWNLOAD_ATTEMPTS: u32 = 5;

/// Stream `url` into `dest`, hashing on the way; returns the sha256 hex.
/// A stalled or dropped connection resumes from the last byte received
/// (HTTP Range), so a slow link doesn't lose a big download to one stall.
fn download(url: &str, dest: &Path, log: &Log) -> Result<String, String> {
    use sha2::{Digest as _, Sha256};
    let agent = agent(true);
    let mut file =
        fs::File::create(dest).map_err(|e| format!("Couldn't create the download file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut step, mut total) = (0u64, 0u64, None::<u64>);
    let mut stalls = 0;
    loop {
        let before = done;
        let attempt = (|| -> Result<(), String> {
            let mut response = get(&agent, url, false, done)?;
            if done > 0 && response.status().as_u16() != 206 {
                // The server ignored the range and sent the whole file again.
                file.set_len(0)
                    .and_then(|()| file.rewind())
                    .map_err(|e| format!("Couldn't restart the download: {e}"))?;
                (hasher, done, step) = (Sha256::new(), 0, 0);
            }
            if done == 0 {
                total = response.body().content_length().filter(|t| *t > 0);
            }
            let mut reader = response.body_mut().as_reader();
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    return Ok(());
                }
                done += n as u64;
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n])
                    .map_err(|e| format!("Couldn't write the download: {e}"))?;
                if let Some(total) = total {
                    let now = done * 20 / total;
                    if now > step {
                        step = now;
                        log(format!(
                            "  {}% of {:.1} MB",
                            now * 5,
                            total as f64 / 1_048_576.
                        ));
                    }
                }
            }
        })();
        let error = match attempt {
            Ok(()) if total.is_none_or(|t| done >= t) => break,
            Ok(()) => "the server closed the connection early".to_string(),
            Err(e) if e.starts_with("Couldn't write") || e.starts_with("Couldn't restart") => {
                return Err(e);
            }
            Err(e) => e,
        };
        stalls = if done > before { 1 } else { stalls + 1 };
        if stalls >= DOWNLOAD_ATTEMPTS {
            return Err(format!("The download was interrupted: {error}"));
        }
        log(format!(
            "  The connection dropped at {:.1} MB; resuming…",
            done as f64 / 1_048_576.
        ));
    }
    file.sync_all()
        .map_err(|e| format!("Couldn't write the download: {e}"))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Refuse anything but an exact match with the published digest.
fn verify(name: &str, actual: &str, sums: &str) -> Result<(), String> {
    match expected_digest(sums, name) {
        Some(expected) if expected == actual => Ok(()),
        Some(_) => Err(format!(
            "{name} doesn't match its published checksum; refusing to install it."
        )),
        None => Err(format!(
            "The checksums list has no entry for {name}; refusing to install an unverified download."
        )),
    }
}

fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    let result = if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else {
        extract_tar_gz(archive, dest)
    };
    result.map_err(|e| format!("Couldn't unpack {name}: {e}"))
}

/// A scratch folder inside the toolchain dir (same volume as the target).
struct Staging(PathBuf);

impl Staging {
    fn new(toolchain: &Path, tag: &str) -> Result<Self, String> {
        let dir = toolchain.join(format!(".{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ── Node.js ────────────────────────────────────────────────────────────────

/// The latest Node 22 LTS into `<toolchain>/node`, verified against the
/// release's `SHASUMS256.txt`. Never touches a system Node.
pub fn install_node(log: &Log) -> Result<(), String> {
    let toolchain = toolchain_dir().ok_or("This system has no app data folder.")?;
    log("Looking up the latest Node.js 22 LTS on nodejs.org…".into());
    let index: Value = serde_json::from_str(&get_text(&format!("{NODE_DIST}/index.json"), false)?)
        .map_err(|e| format!("nodejs.org's release index is unreadable: {e}"))?;
    let version = pick_node22(&index).ok_or("nodejs.org lists no Node.js 22 LTS release.")?;
    let asset = node_asset(&version, std::env::consts::OS, std::env::consts::ARCH)
        .ok_or("nodejs.org has no Node.js build for this system.")?;
    let target = toolchain.join("node");
    let node =
        prefix_bin(&target, cfg!(windows)).join(if cfg!(windows) { "node.exe" } else { "node" });
    if capture(&node, &["--version"]).is_some_and(|(ok, out)| ok && out.trim() == version) {
        log(format!(
            "Node.js {version} is already installed in {}.",
            target.display()
        ));
        return Ok(());
    }
    let sums = get_text(&format!("{NODE_DIST}/{version}/SHASUMS256.txt"), false)?;
    let staging = Staging::new(&toolchain, "node-download")?;
    let archive = staging.0.join(&asset);
    log(format!("Downloading {asset}…"));
    let digest = download(&format!("{NODE_DIST}/{version}/{asset}"), &archive, log)?;
    verify(&asset, &digest, &sums)?;
    log("Checksum verified. Unpacking…".into());
    let unpacked = staging.0.join("unpacked");
    extract(&archive, &unpacked)?;
    let root = unpacked.join(
        asset
            .strip_suffix(".tar.gz")
            .or_else(|| asset.strip_suffix(".zip"))
            .unwrap_or(&asset),
    );
    if !root.is_dir() {
        return Err(format!("{asset} doesn't have the expected layout."));
    }
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|e| {
            format!(
                "Couldn't replace the previous Node.js in {} ({e}). Close anything using it and try again.",
                target.display()
            )
        })?;
    }
    fs::rename(&root, &target)
        .map_err(|e| format!("Couldn't move Node.js into {}: {e}", target.display()))?;
    log(format!(
        "Node.js {version} is installed in {}.",
        target.display()
    ));
    Ok(())
}

// ── Engram ─────────────────────────────────────────────────────────────────

/// The latest Engram release binary into `<toolchain>/bin`, verified when
/// the release publishes checksums.
pub fn install_engram_release(log: &Log) -> Result<(), String> {
    let toolchain = toolchain_dir().ok_or("This system has no app data folder.")?;
    log("Looking up the latest Engram release on GitHub…".into());
    let release = parse_release(&get_text(ENGRAM_LATEST, true)?)
        .ok_or("GitHub's answer about Engram's latest release is unreadable.")?;
    let name = engram_asset(
        &release.version,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .ok_or("Engram publishes no build for this system.")?;
    let url = release
        .assets
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, u)| u.clone())
        .ok_or_else(|| format!("Engram {} has no {name}.", release.version))?;
    let sums_url = release
        .assets
        .iter()
        .find(|(n, _)| n.ends_with("checksums.txt"))
        .map(|(_, u)| u.clone());
    let staging = Staging::new(&toolchain, "engram-download")?;
    let archive = staging.0.join(&name);
    log(format!("Downloading {name}…"));
    let digest = download(&url, &archive, log)?;
    match sums_url {
        Some(sums_url) => {
            verify(&name, &digest, &get_text(&sums_url, false)?)?;
            log("Checksum verified. Unpacking…".into());
        }
        None => log("This release publishes no checksums. Unpacking…".into()),
    }
    let unpacked = staging.0.join("unpacked");
    extract(&archive, &unpacked)?;
    let exe = if cfg!(windows) {
        "engram.exe"
    } else {
        "engram"
    };
    let found = find_file(&unpacked, exe).ok_or_else(|| format!("{name} has no {exe}."))?;
    let bin = toolchain.join("bin");
    fs::create_dir_all(&bin).map_err(|e| format!("Couldn't create {}: {e}", bin.display()))?;
    let target = bin.join(exe);
    let staged = bin.join(format!(".{exe}.new"));
    fs::copy(&found, &staged)
        .and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
            }
            fs::rename(&staged, &target)
        })
        .map_err(|e| format!("Couldn't install {}: {e}", target.display()))?;
    log(format!(
        "Engram {} is installed in {}.",
        release.version,
        bin.display()
    ));
    Ok(())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    None
}

/// Move the single folder an archive unpacked to into `target`, replacing
/// what was there.
fn install_root(unpacked: &Path, target: &Path, what: &str) -> Result<(), String> {
    let root = fs::read_dir(unpacked)
        .ok()
        .and_then(|entries| entries.flatten().map(|e| e.path()).find(|p| p.is_dir()))
        .ok_or_else(|| format!("The {what} download doesn't have the expected layout."))?;
    if target.exists() {
        fs::remove_dir_all(target).map_err(|e| {
            format!(
                "Couldn't replace the previous {what} in {} ({e}). Close anything using it and try again.",
                target.display()
            )
        })?;
    }
    fs::rename(&root, target)
        .map_err(|e| format!("Couldn't move {what} into {}: {e}", target.display()))
}

// ── Java and Maestro ───────────────────────────────────────────────────────

/// The latest Temurin JRE into `<toolchain>/java`, verified against the
/// checksum Adoptium publishes. Never touches a system Java.
pub fn install_java_runtime(log: &Log) -> Result<(), String> {
    let toolchain = toolchain_dir().ok_or("This system has no app data folder.")?;
    let query = java_runtime_query(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or("Adoptium publishes no Java runtime for this system.")?;
    log(format!(
        "Looking up the latest Java {JAVA_FEATURE} runtime on Adoptium…"
    ));
    let package = parse_java_runtime(&get_text(&query, false)?)
        .ok_or("Adoptium's answer about its Java runtimes is unreadable.")?;
    let staging = Staging::new(&toolchain, "java-download")?;
    let archive = staging.0.join(&package.name);
    log(format!("Downloading {}…", package.name));
    let digest = download(&package.url, &archive, log)?;
    if digest != package.sha256 {
        return Err(format!(
            "{} doesn't match its published checksum; refusing to install it.",
            package.name
        ));
    }
    log("Checksum verified. Unpacking…".into());
    let unpacked = staging.0.join("unpacked");
    extract(&archive, &unpacked)?;
    let target = toolchain.join("java");
    install_root(&unpacked, &target, "Java runtime")?;
    log(format!("Java is installed in {}.", target.display()));
    Ok(())
}

/// The latest Maestro CLI release into `<toolchain>/maestro`, verified
/// against the release's `checksums_sha256.txt`.
pub fn install_maestro_release(log: &Log) -> Result<(), String> {
    const NAME: &str = "maestro.zip";
    let toolchain = toolchain_dir().ok_or("This system has no app data folder.")?;
    log("Looking up the latest Maestro release on GitHub…".into());
    let release = parse_release(&get_text(MAESTRO_LATEST, true)?)
        .ok_or("GitHub's answer about Maestro's latest release is unreadable.")?;
    let asset = |wanted: &str| {
        release
            .assets
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, url)| url.clone())
    };
    let url = asset(NAME).ok_or_else(|| format!("Maestro {} has no {NAME}.", release.version))?;
    let sums = asset("checksums_sha256.txt").ok_or_else(|| {
        format!(
            "Maestro {} publishes no checksums; refusing to install an unverified download.",
            release.version
        )
    })?;
    let staging = Staging::new(&toolchain, "maestro-download")?;
    let archive = staging.0.join(NAME);
    log(format!(
        "Downloading Maestro {} (about 350 MB)…",
        release.version
    ));
    let digest = download(&url, &archive, log)?;
    verify(NAME, &digest, &get_text(&sums, false)?)?;
    log("Checksum verified. Unpacking…".into());
    let unpacked = staging.0.join("unpacked");
    extract(&archive, &unpacked)?;
    let target = toolchain.join("maestro");
    install_root(&unpacked, &target, "Maestro")?;
    log(format!(
        "Maestro {} is installed in {}.",
        release.version,
        target.display()
    ));
    Ok(())
}

// ── uv (for gentle-stream) ─────────────────────────────────────────────────

/// The uv that installs the music player: the version and checksums Gentle
/// Shell's own `/gentle:music` offer pins (`scripts/installer-downloads.mjs`).
const UV_VERSION: &str = "0.13.6";

/// uv's release archive and its sha256 for `(os, arch)` as
/// `std::env::consts` names them; gentle-stream runs on macOS and Linux only.
pub fn uv_asset(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    Some(match (os, arch) {
        ("macos", "aarch64") => (
            "uv-aarch64-apple-darwin.tar.gz",
            "60fbd5c8a1ed681845eea28d9d622fcde03c51e91a43465b64a1e17db1ed3472",
        ),
        ("macos", "x86_64") => (
            "uv-x86_64-apple-darwin.tar.gz",
            "63ba17fdda55ad1a230d4b37fc6017ffa10c23688c83818bd3bfdf914c652a02",
        ),
        ("linux", "x86_64") => (
            "uv-x86_64-unknown-linux-gnu.tar.gz",
            "f29c3e94ffabd7e6f3ba521dba169a9b7ec12100bf3257087f0f24f6123f3645",
        ),
        ("linux", "aarch64") => (
            "uv-aarch64-unknown-linux-gnu.tar.gz",
            "229e194255c7958d86d56e8790d969a6f74736592133f0dfd0076f754c642c1d",
        ),
        _ => return None,
    })
}

/// The pinned uv into `target` (its executable only), verified against the
/// pinned checksum; nothing happens when that uv is already there.
pub fn install_uv(target: &Path, log: &Log) -> Result<(), String> {
    if capture(target, &["--version"])
        .is_some_and(|(ok, out)| ok && out.starts_with(&format!("uv {UV_VERSION} ")))
    {
        return Ok(());
    }
    let (name, sha256) = uv_asset(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or("uv has no build for this system.")?;
    let dir = target.parent().ok_or("uv has nowhere to go.")?;
    fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let staging = Staging::new(dir, "uv-download")?;
    let archive = staging.0.join(name);
    log(format!("Downloading uv {UV_VERSION}…"));
    let digest = download(
        &format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{name}"),
        &archive,
        log,
    )?;
    if digest != sha256 {
        return Err(format!(
            "{name} doesn't match its pinned checksum; refusing to install it."
        ));
    }
    let unpacked = staging.0.join("unpacked");
    extract(&archive, &unpacked)?;
    let uv = find_file(&unpacked, "uv").ok_or("The uv download has no uv executable.")?;
    fs::copy(&uv, target).map_err(|e| format!("Couldn't install uv: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(target, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Couldn't make uv executable: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A connection that drops halfway resumes with a Range request, and the
    /// digest covers the whole file exactly once.
    #[test]
    fn a_dropped_download_resumes_where_it_stopped() {
        use sha2::{Digest as _, Sha256};
        use std::io::{BufRead as _, BufReader};
        use std::net::TcpListener;
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();
        let server = std::thread::spawn(move || {
            let mut ranges = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut from = None;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                }
                ranges.push(from);
                let mut stream = stream;
                match from {
                    // First answer: the full length promised, a third sent, then hang up.
                    None => {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                            served.len()
                        );
                        stream.write_all(head.as_bytes()).unwrap();
                        stream.write_all(&served[..100_000]).unwrap();
                    }
                    Some(from) => {
                        let rest = &served[from..];
                        let head = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{}\r\n\r\n",
                            rest.len(),
                            served.len() - 1,
                            served.len()
                        );
                        stream.write_all(head.as_bytes()).unwrap();
                        stream.write_all(rest).unwrap();
                    }
                }
            }
            ranges
        });
        let dir = std::env::temp_dir().join(format!("gs-fetch-resume-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("file.bin");
        let digest =
            download(&format!("http://127.0.0.1:{port}/file.bin"), &dest, &|_| {}).unwrap();
        assert_eq!(server.join().unwrap(), vec![None, Some(100_000)]);
        assert_eq!(fs::read(&dest).unwrap(), body);
        let expected: String = Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(digest, expected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_the_newest_qualifying_node_22_lts() {
        let index = json!([
            {"version": "v24.2.0", "lts": false},
            {"version": "v23.11.0", "lts": false},
            {"version": "v22.21.1", "lts": "Jod"},
            {"version": "v22.19.0", "lts": "Jod"},
            {"version": "v22.18.0", "lts": "Jod"},
            {"version": "v20.19.0", "lts": "Iron"}
        ]);
        assert_eq!(pick_node22(&index).as_deref(), Some("v22.21.1"));
        let too_old =
            json!([{"version": "v22.18.0", "lts": "Jod"}, {"version": "v22.20.0", "lts": false}]);
        assert_eq!(pick_node22(&too_old), None);
    }

    #[test]
    fn node_assets_per_platform() {
        let v = "v22.21.1";
        assert_eq!(
            node_asset(v, "macos", "aarch64").as_deref(),
            Some("node-v22.21.1-darwin-arm64.tar.gz")
        );
        assert_eq!(
            node_asset(v, "macos", "x86_64").as_deref(),
            Some("node-v22.21.1-darwin-x64.tar.gz")
        );
        assert_eq!(
            node_asset(v, "linux", "x86_64").as_deref(),
            Some("node-v22.21.1-linux-x64.tar.gz")
        );
        assert_eq!(
            node_asset(v, "linux", "aarch64").as_deref(),
            Some("node-v22.21.1-linux-arm64.tar.gz")
        );
        assert_eq!(
            node_asset(v, "windows", "x86_64").as_deref(),
            Some("node-v22.21.1-win-x64.zip")
        );
        assert_eq!(
            node_asset(v, "windows", "aarch64").as_deref(),
            Some("node-v22.21.1-win-arm64.zip")
        );
        assert_eq!(node_asset(v, "linux", "arm"), None);
        assert_eq!(node_asset(v, "freebsd", "x86_64"), None);
    }

    #[test]
    fn finds_an_asset_in_shasums256() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!(
            "{a}  node-v22.21.1-darwin-arm64.tar.gz\n{b}  node-v22.21.1-win-x64.zip\n{a}  node-v22.21.1-win-x64.7z\n"
        );
        assert_eq!(
            expected_digest(&sums, "node-v22.21.1-win-x64.zip"),
            Some("b".repeat(64))
        );
        assert_eq!(
            expected_digest(&sums, "node-v22.21.1-linux-x64.tar.gz"),
            None
        );
        assert!(verify("node-v22.21.1-win-x64.zip", &"b".repeat(64), &sums).is_ok());
        assert!(verify("node-v22.21.1-win-x64.zip", &"c".repeat(64), &sums).is_err());
        assert!(verify("missing.zip", &"c".repeat(64), &sums).is_err());
    }

    #[test]
    fn engram_assets_and_release() {
        assert_eq!(
            engram_asset("1.12.0", "macos", "aarch64").as_deref(),
            Some("engram_1.12.0_darwin_arm64.tar.gz")
        );
        assert_eq!(
            engram_asset("1.12.0", "linux", "x86_64").as_deref(),
            Some("engram_1.12.0_linux_amd64.tar.gz")
        );
        assert_eq!(
            engram_asset("1.12.0", "windows", "x86_64").as_deref(),
            Some("engram_1.12.0_windows_amd64.zip")
        );
        let release = parse_release(
            r#"{"tag_name":"v1.12.0","assets":[{"name":"checksums.txt","browser_download_url":"https://x/c"}]}"#,
        )
        .unwrap();
        assert_eq!(release.version, "1.12.0");
        assert_eq!(
            release.assets,
            vec![("checksums.txt".to_string(), "https://x/c".to_string())]
        );
    }

    #[test]
    fn java_runtime_query_and_package() {
        assert_eq!(
            java_runtime_query("macos", "aarch64").as_deref(),
            Some(
                "https://api.adoptium.net/v3/assets/latest/21/hotspot?os=mac&architecture=aarch64&image_type=jre&vendor=eclipse"
            )
        );
        assert!(
            java_runtime_query("windows", "x86_64")
                .unwrap()
                .contains("os=windows&architecture=x64")
        );
        assert_eq!(java_runtime_query("linux", "arm"), None);
        let json = r#"[{"binary":{"package":{"name":"OpenJDK21U-jre.tar.gz","link":"https://x/j.tar.gz","checksum":"ABC123"}}}]"#;
        assert_eq!(
            parse_java_runtime(json),
            Some(Package {
                name: "OpenJDK21U-jre.tar.gz".into(),
                url: "https://x/j.tar.gz".into(),
                sha256: "abc123".into(),
            })
        );
        assert_eq!(parse_java_runtime("[]"), None);
    }
}
