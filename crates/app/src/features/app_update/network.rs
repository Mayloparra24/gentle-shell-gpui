//! GitHub requests: the `releases/latest` lookup, `SHA256SUMS`, and the
//! archive download, hashed while it streams to disk.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::Duration;

use super::release::{Release, hex, parse_release};
use super::{Phase, Problem, SUMS, fail};

const LATEST_API: &str =
    "https://api.github.com/repos/ismaelosuna7824/gentle-shell-gpui/releases/latest";
const USER_AGENT: &str = concat!("gentle-shell-gpui/", env!("CARGO_PKG_VERSION"));
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

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

pub(super) fn fetch_release() -> Result<Release, Problem> {
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

/// Stream `url` into `dest`, hashing on the way; returns the sha256 hex.
pub(super) fn download(
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

pub(super) fn fetch_text(url: &str) -> Result<String, Problem> {
    get(&agent(false), url)?
        .body_mut()
        .read_to_string()
        .map_err(|e| Problem::new(format!("Couldn't read {SUMS}: {e}")))
}
