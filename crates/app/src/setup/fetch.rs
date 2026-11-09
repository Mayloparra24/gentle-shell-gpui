//! Downloads the engine installs into the app's own toolchain folder: Node.js
//! from nodejs.org and the Engram release from GitHub, each verified against
//! the checksums its release publishes.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{Log, MIN_NODE, capture, parse_version, prefix_bin, toolchain_dir};
use crate::app_update::{expected_digest, extract_tar_gz, extract_zip};

const USER_AGENT: &str = concat!("gentle-shell-gpui/", env!("CARGO_PKG_VERSION"));
const NODE_DIST: &str = "https://nodejs.org/dist";
const ENGRAM_LATEST: &str =
    "https://api.github.com/repos/Gentleman-Programming/engram/releases/latest";

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

fn get(
    agent: &ureq::Agent,
    url: &str,
    github_api: bool,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut request = agent.get(url).header("User-Agent", USER_AGENT);
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
    get(&agent(false), url, github_api)?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Couldn't read {url}: {e}"))
}

/// Stream `url` into `dest`, hashing on the way; returns the sha256 hex.
fn download(url: &str, dest: &Path, log: &Log) -> Result<String, String> {
    use sha2::{Digest as _, Sha256};
    let mut response = get(&agent(true), url, false)?;
    let total = response.body().content_length();
    let mut reader = response.body_mut().as_reader();
    let mut file =
        fs::File::create(dest).map_err(|e| format!("Couldn't create the download file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut step) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("The download was interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("Couldn't write the download: {e}"))?;
        if let Some(total) = total.filter(|t| *t > 0) {
            let now = done * 10 / total;
            if now > step {
                step = now;
                log(format!(
                    "  {}% of {:.1} MB",
                    now * 10,
                    total as f64 / 1_048_576.
                ));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
