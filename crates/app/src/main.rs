#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod alerts;
mod app;
mod app_update;
mod assets;
mod banner;
mod browser;
mod cli;
mod files;
mod gentle_state;
mod git;
mod instance;
mod integration;
mod memory;
mod notices;
mod onboarding;
mod palette;
mod pane;
mod platform;
mod profiles;
mod providers;
mod session;
mod sessions;
mod settings;
mod setup;
mod surge;
mod terminal;
mod themes;
mod ui;
mod updates;
mod usage;
mod workspace;
mod workspaces;

fn main() {
    // An explicit folder selects (or creates) the workspace rooted there;
    // without one the last workspace is restored.
    let cli = cli::parse_cli();
    let folder = cli
        .folder
        .filter(|p| p.is_dir())
        .and_then(|p| dunce::canonicalize(p).ok());
    let prompt = cli.prompt;
    // Only one app per user: a second launch hands its request over and exits.
    let request = instance::OpenRequest {
        folder: folder.clone(),
        prompt: prompt.clone(),
    };
    let requests = match instance::claim(&request) {
        instance::Role::Forwarded => return,
        instance::Role::Primary(requests) => requests,
    };
    // Toolchains onboarding installed into the app's own folders.
    setup::apply_toolchain_path();
    // Gentle Shell 4.7's own install moved its config folder without copying
    // the old one: bring profiles, routing and preferences along once.
    match pi_rpc::adopt_standalone_config() {
        Ok(copied) if !copied.is_empty() => {
            eprintln!(
                "Copied {} Gentle Shell settings to its new config folder",
                copied.len()
            )
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("Could not copy Gentle Shell's settings to its new folder: {error}")
        }
    }
    app::run(folder, prompt, requests);
}
