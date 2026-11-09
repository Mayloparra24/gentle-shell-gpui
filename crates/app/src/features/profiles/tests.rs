//! File effects end to end, in a scratch directory: the store, seeding,
//! settings, Make default, managed agent files and pins.

use std::fs;
use std::path::PathBuf;

use gentle_core::profiles::{
    AgentSource, RouteEntry, Routing, assignment_names, bootstrap_profiles, create_profile,
    evaluate_pin, serialize_profiles, update_frontmatter_routing,
};
use serde_json::{Value, json};

use super::apply::{
    SettingsWrite, apply_orchestrator_settings, make_default, sha256_hex, update_managed_ownership,
};
use super::env::{Env, discover_agents};
use super::files::{StoreRead, read_store, seed_profiles, unique_token, write_atomic};
use super::pins::{pin_status_at, write_pin};

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gsg-profiles-{tag}-{}", unique_token()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn env(&self) -> Env {
        Env {
            cwd: self.0.join("project"),
            agent_home: self.0.join("agent"),
            config_home: self.0.join("config"),
            home: self.0.join("home"),
            package_root: None,
        }
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry(model: Option<&str>, thinking: Option<&str>, tier: Option<&str>) -> RouteEntry {
    RouteEntry {
        model: model.map(str::to_string),
        thinking: thinking.map(str::to_string),
        tier: tier.map(str::to_string),
    }
}

#[test]
fn save_is_atomic_and_keeps_unknown_top_level_keys() {
    let scratch = Scratch::new("save");
    let path = scratch.write(
        "config/profiles.json",
        r#"{"kind":"gentle-pi.agent_model_profiles","version":1,"futureKey":{"a":1},"profiles":{"p":{"w":"a/b"}}}"#,
    );
    let StoreRead::Valid(file, _) = read_store(&path) else {
        panic!("store should parse")
    };
    let next = create_profile(
        &file,
        "q",
        Routing(vec![(
            "orchestrator".into(),
            entry(Some("x/y"), Some("max"), None),
        )]),
    )
    .unwrap();
    write_atomic(&path, &serialize_profiles(&next)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.ends_with("}\n"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["futureKey"], json!({"a": 1}));
    assert_eq!(value["profiles"]["p"]["w"], json!({"model": "a/b"}));
    assert_eq!(
        value["profiles"]["q"]["orchestrator"],
        json!({"model": "x/y", "thinking": "max"})
    );
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["kind", "version", "profiles", "futureKey"]);
    let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
    // Identical bytes are not rewritten.
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    write_atomic(&path, &text).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn seeds_current_from_saved_and_materialized_routing() {
    let scratch = Scratch::new("seed");
    let env = scratch.env();
    scratch.write(
        "config/models.json",
        r#"{"gentle-ai-worker":{"model":"openai/gpt-6-sol","thinking":"high"}}"#,
    );
    scratch.write(
        "agent/agents/gentle-ai-explore.md",
        "---\nname: gentle-ai-explore\ndescription: d\nmodel: anthropic/haiku\nthinking: low\n---\nbody\n",
    );
    scratch.write(
        "agent/agents/gentle-ai-worker.md",
        "---\nname: gentle-ai-worker\nmodel: ignored/model\n---\n",
    );
    scratch.write("agent/agents/plain.md", "---\nname: plain\n---\n");
    scratch.write(
        "agent/subagents.json",
        r#"{"model_profiles":{"gentle-ai-explore":{"effort":"medium"}}}"#,
    );
    scratch.write(
        "project/.pi/agents/proj.md",
        "---\nname: \"proj\"\npackage: team\n---\n",
    );
    let agents = discover_agents(&env);
    let names = assignment_names(&agents);
    assert_eq!(&names[..2], ["review-refuter", "review-validator"]);
    assert!(names.contains(&"team.proj".to_string()));
    let file = seed_profiles(&env, &agents);
    assert_eq!(file.active.as_deref(), Some("current"));
    let current = file.get("current").unwrap();
    assert_eq!(
        current.get("gentle-ai-worker"),
        Some(&entry(Some("openai/gpt-6-sol"), Some("high"), None))
    );
    // subagents.json effort wins over frontmatter; model falls back to it.
    assert_eq!(
        current.get("gentle-ai-explore"),
        Some(&entry(Some("anthropic/haiku"), Some("medium"), None))
    );
    assert!(!current.contains("plain"));
    // Nothing routed: seeded but not active.
    let empty = Scratch::new("seed-empty");
    let env = empty.env();
    let file = seed_profiles(&env, &discover_agents(&env));
    assert_eq!(file.active, None);
    assert_eq!(file.get("current"), Some(&Routing::default()));
}

#[test]
fn make_default_settings_merge_keeps_other_keys() {
    let scratch = Scratch::new("settings");
    let path = scratch.write(
        "agent/settings.json",
        "{\n  \"theme\": \"dark\",\n  \"defaultModel\": \"old\",\n  \"defaultThinkingLevel\": \"low\",\n  \"packages\": [\"npm:x\"]\n}\n",
    );
    let written = apply_orchestrator_settings(
        &path,
        &entry(Some("openai/gpt-6-sol"), Some("high"), Some("fast")),
    )
    .unwrap();
    assert_eq!(written, SettingsWrite::Written);
    let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "theme",
            "defaultModel",
            "defaultThinkingLevel",
            "packages",
            "defaultProvider"
        ]
    );
    assert_eq!(value["defaultProvider"], "openai");
    assert_eq!(value["defaultModel"], "gpt-6-sol");
    assert_eq!(value["defaultThinkingLevel"], "high");
    assert_eq!(value["packages"], json!(["npm:x"]));
    assert_eq!(
        apply_orchestrator_settings(&path, &entry(Some("openai/gpt-6-sol"), Some("high"), None))
            .unwrap(),
        SettingsWrite::Unchanged
    );
    apply_orchestrator_settings(&path, &entry(Some("openai/gpt-6-sol"), None, None)).unwrap();
    let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(value.get("defaultThinkingLevel").is_none());
    assert_eq!(
        apply_orchestrator_settings(&path, &entry(None, Some("high"), None)).unwrap(),
        SettingsWrite::Unchanged
    );
    assert!(apply_orchestrator_settings(&path, &entry(Some("no-provider"), None, None)).is_err());
    let broken = scratch.write("agent/broken.json", "{nope");
    assert!(apply_orchestrator_settings(&broken, &entry(Some("a/b"), None, None)).is_err());
    assert_eq!(fs::read_to_string(&broken).unwrap(), "{nope");
}

#[test]
fn make_default_writes_store_routing_agents_and_settings() {
    let scratch = Scratch::new("default");
    let env = scratch.env();
    fs::create_dir_all(&env.cwd).unwrap();
    let worker = scratch.write(
        "agent/agents/worker.md",
        "---\nname: worker\ndescription: w\nmodel: old/model\n---\nbody\n",
    );
    scratch.write(
        "agent/agents/other.md",
        "---\nname: other\nthinking: low\n---\n",
    );
    scratch.write(
        "agent/subagents.json",
        r#"{"keep":true,"model_profiles":{"other":{"effort":"low"}}}"#,
    );
    scratch.write("agent/settings.json", r#"{"theme":"x"}"#);
    let config = Routing(vec![
        (
            "orchestrator".into(),
            entry(Some("openai/gpt-6-sol"), Some("high"), None),
        ),
        (
            "worker".into(),
            entry(Some("anthropic/opus"), Some("max"), None),
        ),
    ]);
    let file = create_profile(&bootstrap_profiles(&Routing::default()), "deep", config).unwrap();
    let outcome = make_default(&env, &file, "deep").unwrap();
    assert_eq!(outcome.file.active.as_deref(), Some("deep"));
    assert_eq!(
        outcome.orchestrator,
        Some(entry(Some("openai/gpt-6-sol"), Some("high"), None))
    );
    let StoreRead::Valid(stored, _) = read_store(&env.profiles_path()) else {
        panic!()
    };
    assert_eq!(stored.active.as_deref(), Some("deep"));
    let models: Value =
        serde_json::from_str(&fs::read_to_string(env.models_path()).unwrap()).unwrap();
    assert_eq!(
        models["worker"],
        json!({"model": "anthropic/opus", "thinking": "max"})
    );
    assert_eq!(
        fs::read_to_string(&worker).unwrap(),
        "---\nname: worker\ndescription: w\nmodel: anthropic/opus\nthinking: max\n---\nbody\n"
    );
    assert_eq!(
        fs::read_to_string(scratch.0.join("agent/agents/other.md")).unwrap(),
        "---\nname: other\n---\n"
    );
    let subagents: Value =
        serde_json::from_str(&fs::read_to_string(env.subagents_path(AgentSource::User)).unwrap())
            .unwrap();
    assert_eq!(
        subagents,
        json!({"keep": true, "model_profiles": {"worker": {"model": "anthropic/opus", "effort": "max"}}})
    );
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(env.settings_path()).unwrap()).unwrap();
    assert_eq!(
        settings,
        json!({"theme": "x", "defaultProvider": "openai", "defaultModel": "gpt-6-sol", "defaultThinkingLevel": "high"})
    );
}

#[test]
fn managed_agent_files_keep_their_manifest_hash() {
    let scratch = Scratch::new("managed");
    let env = scratch.env();
    let original = "---\nname: worker\n---\n";
    let path = scratch.write("agent/agents/worker.md", original);
    scratch.write(
        "agent/gentle-ai/managed-assets.json",
        &json!({"schemaVersion": 1, "assets": {"agents/worker.md": sha256_hex(original.as_bytes())}}).to_string(),
    );
    let next = update_frontmatter_routing(original, &entry(Some("a/b"), None, None));
    assert!(update_managed_ownership(&env.agent_home, &path, original, &next).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), next);
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(scratch.0.join("agent/gentle-ai/managed-assets.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["assets"]["agents/worker.md"],
        json!(sha256_hex(next.as_bytes()))
    );
    assert!(
        !scratch
            .0
            .join("agent/gentle-ai/managed-assets.lock")
            .exists()
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn pins_resolve_local_then_repo() {
    let scratch = Scratch::new("pin");
    let file = create_profile(
        &bootstrap_profiles(&Routing::default()),
        "team",
        Routing::default(),
    )
    .unwrap();
    let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
    write_pin(&status.repo_path, "team").unwrap();
    scratch.write("git/gentle-ai/profile-pin.json", "{}");
    let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
    let eval = evaluate_pin(Some(&status), &file);
    assert_eq!(
        eval.winner.as_ref().map(|w| (w.0, w.1.as_str())),
        Some(("repo", "team"))
    );
    assert_eq!(eval.invalid.len(), 1);
    write_pin(&status.local_path, "gone").unwrap();
    let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
    let eval = evaluate_pin(Some(&status), &file);
    assert_eq!(eval.stale.len(), 1);
    assert_eq!(eval.winner.map(|w| w.0), Some("repo"));
    assert_eq!(
        fs::read_to_string(&status.repo_path).unwrap(),
        "{\n  \"kind\": \"gentle-pi.agent_model_profile_pin\",\n  \"version\": 1,\n  \"profile\": \"team\"\n}\n"
    );
}
