//! Where the screen reads and writes: Gentle Shell's homes, the installed
//! `gentle-pi` package and the agents discoverable from them.

use std::fs;
use std::path::{Path, PathBuf};

use gentle_core::profiles::{AgentEntry, AgentSource, parse_agent_name};

/// `CORE_MODEL_AGENT_NAMES`: listed first, the rest alphabetically.
const CORE_AGENTS: [&str; 3] = ["jd-judge-a", "jd-judge-b", "jd-fix-agent"];

/// Every location the screen reads or writes, resolved once off the UI thread.
#[derive(Clone, Debug)]
pub struct Env {
    pub cwd: PathBuf,
    pub agent_home: PathBuf,
    pub config_home: PathBuf,
    pub home: PathBuf,
    /// The installed `gentle-pi` package (for built-in agent directories).
    pub package_root: Option<PathBuf>,
}

impl Env {
    pub(super) fn resolve(cwd: PathBuf) -> Self {
        let homes = pi_rpc::resolve_homes();
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let package_root = pi_rpc::resolve_backend(&cwd)
            .ok()
            .and_then(|spec| gentle_pi_root(&spec.program));
        Self {
            cwd,
            agent_home: homes.agent,
            config_home: homes.config,
            home,
            package_root,
        }
    }

    pub(super) fn profiles_path(&self) -> PathBuf {
        self.config_home.join("profiles.json")
    }

    pub(super) fn models_path(&self) -> PathBuf {
        self.config_home.join("models.json")
    }

    pub(super) fn settings_path(&self) -> PathBuf {
        self.agent_home.join("settings.json")
    }

    pub(super) fn subagents_path(&self, source: AgentSource) -> PathBuf {
        match source {
            AgentSource::Project => self.cwd.join(".pi").join("subagents.json"),
            _ => self.agent_home.join("subagents.json"),
        }
    }
}

/// `<gentle-pi>` from the launcher: `<gentle-pi>/bin/gentle-shell.mjs`, or the
/// Windows shim beside `node_modules/gentle-pi`.
pub(crate) fn gentle_pi_root(program: &Path) -> Option<PathBuf> {
    let launcher = dunce::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    let mut roots = Vec::new();
    if let Some(root) = launcher.parent().and_then(Path::parent) {
        roots.push(root.to_path_buf());
    }
    if let Some(dir) = program.parent() {
        roots.push(dir.join("node_modules").join("gentle-pi"));
        roots.push(
            dir.join("..")
                .join("lib")
                .join("node_modules")
                .join("gentle-pi"),
        );
    }
    roots
        .into_iter()
        .find(|root| root.join("extensions").join("gentle-ai.ts").is_file())
}

/// `listAgentFilesRecursive`: `.md` but not `.chain.md`, skipping `skills/`.
fn list_agent_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if name != "skills" {
                list_agent_files(&entry.path(), out);
            }
        } else if kind.is_file() && name.ends_with(".md") && !name.ends_with(".chain.md") {
            out.push(entry.path());
        }
    }
}

fn agents_in(dir: &Path, source: AgentSource) -> Vec<AgentEntry> {
    let mut files = Vec::new();
    list_agent_files(dir, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let name = parse_agent_name(&fs::read_to_string(&path).ok()?)?;
            Some(AgentEntry { name, source, path })
        })
        .collect()
}

/// `discoverableNonBuiltinAgentRoots`, deduplicated by canonical path with
/// later-root precedence (and the package-managed root's lexical path kept).
fn non_builtin_roots(env: &Env) -> Vec<(PathBuf, AgentSource)> {
    let roots = [
        (env.agent_home.join("agents"), AgentSource::User, true),
        (env.agent_home.join("subagents"), AgentSource::User, false),
        (env.home.join(".agents"), AgentSource::User, false),
        (env.cwd.join(".agents"), AgentSource::Project, false),
        (
            env.cwd.join(".pi").join("agents"),
            AgentSource::Project,
            false,
        ),
        (
            env.cwd.join(".pi").join("subagents"),
            AgentSource::Project,
            false,
        ),
    ];
    let mut unique: Vec<(PathBuf, (PathBuf, AgentSource, bool))> = Vec::new();
    for (dir, source, managed) in roots {
        let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if let Some(index) = unique.iter().position(|(c, _)| *c == canonical) {
            let (_, existing) = unique.remove(index);
            let managed_dir = if existing.2 {
                Some(existing.0)
            } else if managed {
                Some(dir.clone())
            } else {
                None
            };
            let is_managed = managed_dir.is_some();
            unique.push((canonical, (managed_dir.unwrap_or(dir), source, is_managed)));
        } else {
            unique.push((canonical, (dir, source, managed)));
        }
    }
    unique
        .into_iter()
        .map(|(_, (dir, source, _))| (dir, source))
        .collect()
}

fn builtin_dirs(env: &Env) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for package in ["pi-subagents-j0k3r", "pi-subagents"] {
        if let Some(root) = &env.package_root {
            dirs.push(root.join("..").join(package).join("agents"));
        }
        dirs.push(
            env.cwd
                .join(".pi")
                .join("npm")
                .join("node_modules")
                .join(package)
                .join("agents"),
        );
        dirs.push(
            env.home
                .join(".local")
                .join("lib")
                .join("node_modules")
                .join(package)
                .join("agents"),
        );
    }
    dirs
}

/// `listDiscoverableAgents`: built-ins, then user and project roots; a later
/// definition of the same name wins. Core agents first, the rest sorted.
pub fn discover_agents(env: &Env) -> Vec<AgentEntry> {
    let mut all = Vec::new();
    for dir in builtin_dirs(env) {
        all.extend(agents_in(&dir, AgentSource::Builtin));
    }
    for (dir, source) in non_builtin_roots(env) {
        all.extend(agents_in(&dir, source));
    }
    let mut by_name: Vec<AgentEntry> = Vec::new();
    for agent in all {
        match by_name.iter_mut().find(|a| a.name == agent.name) {
            Some(slot) => *slot = agent,
            None => by_name.push(agent),
        }
    }
    let mut ordered: Vec<AgentEntry> = CORE_AGENTS
        .iter()
        .filter_map(|core| by_name.iter().find(|a| a.name == *core).cloned())
        .collect();
    let mut rest: Vec<AgentEntry> = by_name
        .into_iter()
        .filter(|a| !CORE_AGENTS.contains(&a.name.as_str()))
        .collect();
    rest.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    ordered.extend(rest);
    ordered
}
