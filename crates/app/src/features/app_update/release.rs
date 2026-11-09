//! Releases and versions: the GitHub `releases/latest` answer, version
//! comparison, the archive name per platform, `SHA256SUMS` parsing and the
//! check's outcome. Pure: no network, files or clock.

use serde::{Deserialize, Serialize};

use super::{BINARY, Problem};
use gentle_core::version::{is_newer, stable_parts};

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
    pub(super) fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// Builds without a stable `X.Y.Z` (or `0.0.0`) are development builds: no
/// automatic checks, and any published release counts as newer.
pub fn is_dev(version: &str) -> bool {
    version == "0.0.0" || stable_parts(version).is_none()
}

/// Whether `latest` should replace `current`.
pub(super) fn is_update(current: &str, latest: &str) -> bool {
    let Some(latest) = stable_parts(latest) else {
        return false;
    };
    match stable_parts(current) {
        Some(current_parts) if !is_dev(current) => is_newer(latest, current_parts),
        _ => true,
    }
}

/// The release archive for `(os, arch)` as `std::env::consts` names them.
pub(super) fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let suffix = match (os, arch) {
        ("macos", "aarch64" | "x86_64") => format!("macos-{arch}.zip"),
        ("windows", "x86_64") => "windows-x86_64.zip".to_string(),
        ("linux", "x86_64") => "linux-x86_64.tar.gz".to_string(),
        _ => return None,
    };
    Some(format!("{BINARY}-{version}-{suffix}"))
}

pub(super) fn platform_asset(release: &Release) -> Option<&Asset> {
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
pub(super) fn parse_release(json: &str) -> Option<Release> {
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
pub(super) fn parse_sums(text: &str) -> Vec<(String, String)> {
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

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Refuse anything but an exact match with the published digest.
pub(super) fn verify_digest(name: &str, actual: &str, sums: &str) -> Result<(), Problem> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    UpToDate,
    Available(Release),
    /// Newer, but the user chose to skip it (automatic checks only).
    Skipped(String),
}

/// A manual check shows a skipped version again; the startup check does not.
pub(super) fn decide(
    current: &str,
    release: Release,
    skipped: Option<&str>,
    manual: bool,
) -> Outcome {
    if !is_update(current, &release.version) {
        Outcome::UpToDate
    } else if !manual && skipped == Some(release.version.as_str()) {
        Outcome::Skipped(release.version)
    } else {
        Outcome::Available(release)
    }
}

#[cfg(test)]
mod tests {
    use super::super::SUMS;
    use super::super::test_support::release;
    use super::*;

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
}
