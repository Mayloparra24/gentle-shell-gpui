//! Repository pins (gentle-pi `lib/agent-profile-pin.ts`): the pin file
//! format, which layer wins and what the screen must say about the rest.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::pretty;
use super::store::{ProfilesFile, is_valid_profile_name};

const PIN_KIND: &str = "gentle-pi.agent_model_profile_pin";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinRead {
    Missing,
    Invalid,
    Valid(String),
}

#[derive(Clone, Debug)]
pub struct PinStatus {
    pub local_path: PathBuf,
    pub repo_path: PathBuf,
    pub local: PinRead,
    pub repo: PinRead,
}

#[derive(Clone, Debug, Default)]
pub struct PinEval {
    /// (layer, profile, path)
    pub winner: Option<(&'static str, String, PathBuf)>,
    pub invalid: Vec<(&'static str, PathBuf)>,
    pub stale: Vec<(&'static str, String, PathBuf)>,
}

pub fn parse_pin(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("kind").and_then(Value::as_str) != Some(PIN_KIND)
        || value.get("version").and_then(Value::as_f64) != Some(1.0)
    {
        return None;
    }
    let profile = value.get("profile")?.as_str()?;
    is_valid_profile_name(profile).then(|| profile.to_string())
}

pub fn serialize_pin(profile: &str) -> String {
    pretty(&json!({ "kind": PIN_KIND, "version": 1, "profile": profile }))
}

/// `evaluateProfilePin`: local, then repo; the first legal name the store
/// defines wins; everything else is reported.
pub fn evaluate_pin(status: Option<&PinStatus>, file: &ProfilesFile) -> PinEval {
    let mut eval = PinEval::default();
    let Some(status) = status else { return eval };
    for (layer, path, read) in [
        ("local", &status.local_path, &status.local),
        ("repo", &status.repo_path, &status.repo),
    ] {
        match read {
            PinRead::Missing => {}
            PinRead::Invalid => eval.invalid.push((layer, path.clone())),
            PinRead::Valid(profile) => {
                if !file.has(profile) {
                    eval.stale.push((layer, profile.clone(), path.clone()));
                } else if eval.winner.is_none() {
                    eval.winner = Some((layer, profile.clone(), path.clone()));
                }
            }
        }
    }
    eval
}

/// What the screen must not hide about the repository pins
/// (`routingScreenNotices`, `profilePinScopeNote`).
pub fn pin_notes(pin: Option<&PinStatus>, file: &ProfilesFile) -> Vec<String> {
    let eval = evaluate_pin(pin, file);
    let mut notes = Vec::new();
    for (layer, path) in &eval.invalid {
        notes.push(format!(
            "The {layer} pin file {} is not a valid pin and is ignored. Fix or remove it.",
            path.display()
        ));
    }
    for (layer, profile, path) in &eval.stale {
        notes.push(format!(
            "The {layer} pin names \"{profile}\", which no profile defines, so it is ignored ({}).",
            path.display()
        ));
    }
    if let Some((layer, profile, path)) = &eval.winner {
        notes.push(format!(
            "This repository pins \"{profile}\" ({layer} pin at {}), so its subagent launches resolve that profile instead of the global routing.",
            path.display()
        ));
    }
    notes
}
