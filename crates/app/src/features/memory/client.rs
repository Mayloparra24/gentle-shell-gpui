//! Engram's local HTTP API: where it listens, blocking GETs and the two
//! queries the browser needs. Runs on the background executor.

use std::time::Duration;

use gentle_core::engram::{Observation, encode, parse_observations};

/// `ENGRAM_URL`, else `ENGRAM_PORT` on loopback, else Engram's default port.
fn base_url() -> String {
    if let Ok(url) = std::env::var("ENGRAM_URL")
        && !url.trim().is_empty()
    {
        return url.trim_end_matches('/').to_string();
    }
    let port = std::env::var("ENGRAM_PORT")
        .ok()
        .filter(|p| p.parse::<u16>().is_ok())
        .unwrap_or_else(|| "7437".into());
    format!("http://127.0.0.1:{port}")
}

fn get(path: &str) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let mut request = agent.get(format!("{}{path}", base_url()));
    if let Ok(token) = std::env::var("ENGRAM_HTTP_TOKEN")
        && !token.is_empty()
    {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|_| {
        "Engram is not running. Gentle Shell starts it with the first session.".to_string()
    })?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| format!("Engram answered {status} without JSON"))?;
    if !(200..300).contains(&status) {
        return Err(json["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Engram answered {status}")));
    }
    Ok(json)
}

/// The project Engram assigns to `cwd` (`GET /project/current`).
pub(super) fn current_project(cwd: &std::path::Path) -> Option<String> {
    let json = get(&format!(
        "/project/current?cwd={}",
        encode(&cwd.to_string_lossy())
    ))
    .ok()?;
    json["project"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

pub(super) fn load(project: Option<&str>, query: &str) -> Result<Vec<Observation>, String> {
    let scope = match project {
        Some(p) => format!("project={}", encode(p)),
        None => "all_projects=true".into(),
    };
    let path = if query.trim().is_empty() {
        format!("/observations/recent?{scope}&limit=60")
    } else {
        format!("/search?q={}&{scope}&limit=40", encode(query.trim()))
    };
    match get(&path) {
        Ok(value) => Ok(parse_observations(value)),
        // A project Engram has never seen simply has no memories yet.
        Err(error) if project.is_some() && error.contains("not found") => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
