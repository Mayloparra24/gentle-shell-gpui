//! Profiles: the native `/gentle:profiles` models screen. Profiles are named
//! routing snapshots in `<config home>/profiles.json`; the table edits one
//! profile's orchestrator and agent routes, and the footer decides where it
//! applies. Semantics follow gentle-pi `lib/agent-profiles.ts`,
//! `lib/model-routing-authority.ts`, `lib/agent-profile-pin.ts`,
//! `lib/profiles-orchestrator.ts` and the `RoutingTablePanel` handler in
//! `extensions/gentle-ai.ts`.
//!
//! The pure model (routing entries, the store format, pins, agent
//! frontmatter) is `gentle_core::profiles`. Here, `env` resolves the homes
//! and discovers agents, `files` reads and writes the store, routing and Pi
//! settings, `pins` the repository pin files, and `apply` materializes routes
//! and runs Make default. `ProfilesView` keeps its state and queries here; its
//! draft edits (`draft`), store actions (`actions`) and render (`view`) live
//! in child modules.

use std::path::PathBuf;
use std::rc::Rc;

use gentle_core::profiles::{
    ORCHESTRATOR, PinStatus, ProfilesFile, RouteEntry, Routing, evaluate_pin,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

mod actions;
mod apply;
mod draft;
mod env;
mod files;
mod pins;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use env::gentle_pi_root;
pub use gentle_core::profiles::ModelChoice;

use apply::DefaultPlan;
use env::Env;
use files::load_screen;

pub enum ProfilesEvent {
    Close,
    /// Apply the profile to the running session: send RPC `set_model` with
    /// `model` (provider, id) and then `set_thinking_level` with `thinking`.
    /// `None` means the profile leaves the orchestrator untouched. The draft
    /// was saved to `profiles.json` first when it had edits.
    UseInSession {
        profile: String,
        model: Option<(String, String)>,
        thinking: Option<String>,
    },
    /// Make default finished outside a pinned repository: the TUI also moves
    /// the running session to the new default orchestrator. Send RPC
    /// `set_model` (provider, id) and, when `thinking` is set,
    /// `set_thinking_level`. Files were already written (`Changed` follows).
    SwitchOrchestrator {
        model: (String, String),
        thinking: Option<String>,
    },
    /// Send this slash command as a prompt (`/gentle:profiles clear`,
    /// `/gentle:profiles publish <name>`).
    RunCommand(String),
    /// `profiles.json`, routing, pins or Pi settings were written.
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Good,
    Bad,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameMode {
    New,
    Duplicate,
    Rename,
}

#[derive(Clone, PartialEq, Eq)]
enum Leave {
    Switch(String),
    New,
    Duplicate,
    Close,
}

enum Confirm {
    Unsaved(Leave),
    Delete(String),
    MakeDefault(DefaultPlan),
}

/// Rows: the orchestrator, "every agent", then each assignment name.
const ALL_ROW: &str = "__all";

pub struct ProfilesView {
    env: Option<Env>,
    cwd: PathBuf,
    models: Rc<Vec<ModelChoice>>,
    loading: bool,
    error: Option<String>,
    file: ProfilesFile,
    names: Vec<String>,
    pin: Option<PinStatus>,
    notices: Vec<String>,
    /// Pi's current `settings.json` default orchestrator.
    pi_default: Option<RouteEntry>,
    selected: Option<String>,
    stored: Routing,
    draft: Routing,
    status: Option<(String, Tone)>,
    busy: bool,
    name_mode: Option<NameMode>,
    name_input: Entity<InputState>,
    custom_target: Option<String>,
    custom_input: Entity<InputState>,
    confirm: Option<Confirm>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProfilesEvent> for ProfilesView {}

impl ProfilesView {
    pub fn new(
        cwd: PathBuf,
        models: Vec<ModelChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("profile-name"));
        let custom_input = cx.new(|cx| InputState::new(window, cx).placeholder("provider/model"));
        let subscriptions = vec![
            cx.subscribe_in(
                &name_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_name(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &custom_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_custom(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            env: None,
            cwd,
            models: Rc::new(models),
            loading: true,
            error: None,
            file: ProfilesFile::default(),
            names: Vec::new(),
            pin: None,
            notices: Vec::new(),
            pi_default: None,
            selected: None,
            stored: Routing::default(),
            draft: Routing::default(),
            status: None,
            busy: false,
            name_mode: None,
            name_input,
            custom_target: None,
            custom_input,
            confirm: None,
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// Run blocking file work off the UI thread, then apply its result. The
    /// task is detached so a write already under way finishes even if the
    /// screen closes (the result is then dropped).
    fn run_io<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        self.busy = true;
        let job = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Re-read the store, agents and pins; unsaved edits to the shown profile
    /// survive when that profile still exists.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let env = self.env.clone();
        let cwd = self.cwd.clone();
        self.run_io(
            cx,
            move || {
                let env = env.unwrap_or_else(|| Env::resolve(cwd));
                let loaded = load_screen(&env);
                (env, loaded)
            },
            |this, (env, loaded), cx| {
                this.loading = false;
                this.env = Some(env);
                match loaded {
                    Err(error) => this.error = Some(error),
                    Ok(loaded) => {
                        this.error = None;
                        let dirty = this.change_count() > 0;
                        let pin_winner = evaluate_pin(loaded.pin.as_ref(), &loaded.file)
                            .winner
                            .map(|w| w.1);
                        this.file = loaded.file;
                        this.names = loaded.names;
                        this.pin = loaded.pin;
                        this.notices = loaded.notices;
                        this.pi_default = loaded.pi_default;
                        if let Some(status) = loaded.status {
                            this.say(status, Tone::Info);
                            cx.emit(ProfilesEvent::Changed);
                        }
                        let keep = this.selected.clone().filter(|s| this.file.has(s));
                        let target = keep
                            .clone()
                            .or(pin_winner)
                            .or_else(|| this.file.active.clone().filter(|a| this.file.has(a)))
                            .or_else(|| this.file.names().next().map(str::to_string));
                        let keep_draft = dirty && keep.is_some() && keep == target;
                        this.select(target, keep_draft);
                    }
                }
            },
        );
    }

    fn select(&mut self, name: Option<String>, keep_draft: bool) {
        self.stored = name
            .as_deref()
            .and_then(|n| self.file.get(n))
            .cloned()
            .unwrap_or_default();
        if !keep_draft {
            self.draft = self.stored.clone();
        }
        self.selected = name;
        self.custom_target = None;
    }

    fn say(&mut self, message: impl Into<String>, tone: Tone) {
        self.status = Some((message.into(), tone));
    }

    fn pin_winner(&self) -> Option<String> {
        evaluate_pin(self.pin.as_ref(), &self.file)
            .winner
            .map(|w| w.1)
    }

    fn catalog_model(&self, model: &str) -> Option<&ModelChoice> {
        self.models.iter().find(|m| m.route_id() == model)
    }

    fn fast_capable(&self, model: Option<&str>) -> bool {
        model
            .and_then(|m| self.catalog_model(m))
            .is_some_and(ModelChoice::supports_fast)
    }

    /// The levels a model supports: a catalog model without reasoning offers
    /// only `off`; reasoning or unknown models offer every level.
    fn supported_levels(&self, model: Option<&str>) -> Option<Vec<&'static str>> {
        let choice = self.catalog_model(model?)?;
        (!choice.reasoning).then(|| vec!["off"])
    }

    fn row_keys(&self) -> Vec<String> {
        let mut keys = vec![ORCHESTRATOR.to_string()];
        keys.extend(self.names.iter().cloned());
        keys
    }

    fn change_count(&self) -> usize {
        self.row_keys()
            .iter()
            .map(|key| {
                let (d, s) = (self.draft.get(key), self.stored.get(key));
                let field = |f: fn(&RouteEntry) -> &Option<String>| {
                    d.and_then(|e| f(e).as_ref()) != s.and_then(|e| f(e).as_ref())
                };
                usize::from(field(|e| &e.model))
                    + usize::from(field(|e| &e.thinking))
                    + usize::from(field(|e| &e.tier))
            })
            .sum()
    }

    fn target_keys(&self, key: &str) -> Vec<String> {
        if key == ALL_ROW {
            self.names.clone()
        } else {
            vec![key.to_string()]
        }
    }

    fn row_label(key: &str) -> &str {
        match key {
            ORCHESTRATOR => "the orchestrator",
            ALL_ROW => "every agent",
            other => other,
        }
    }
}
