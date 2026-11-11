#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod cli;
mod features;
mod platform;
mod shared;

use features::setup;
use platform::instance;

fn main() {
    // An explicit folder selects (or creates) the workspace rooted there;
    // without one the last workspace is restored.
    let cli = cli::parse_cli();
    let folder = cli
        .folder
        .filter(|p| p.is_dir())
        .and_then(|p| dunce::canonicalize(p).ok());
    let prompt = cli.prompt;
    let url = cli.url;
    if let Some(url) = url.as_deref()
        && (url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")))
    {
        eprintln!("gentle-shell-gpui: --open-url expects an http(s) URL");
        std::process::exit(2);
    }
    // Only one app per user: a second launch hands its request over and exits.
    let request = instance::OpenRequest {
        folder: folder.clone(),
        prompt: prompt.clone(),
        url: url.clone(),
    };
    let requests = match instance::claim(&request) {
        instance::Role::Forwarded => return,
        instance::Role::Primary(requests) => requests,
    };
    // `gentle-open` lives in the toolchain folder the agents get on PATH;
    // create it before that PATH is computed.
    features::browser::install_open_tool();
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
    app::run(folder, prompt, url, requests);
}
