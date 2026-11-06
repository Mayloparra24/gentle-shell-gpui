mod activity;
mod app_update;
mod assets;
mod banner;
mod files;
mod gentle_state;
mod git;
mod instance;
mod integration;
mod memory;
mod notices;
mod palette;
mod pane;
mod profiles;
mod providers;
mod session;
mod sessions;
mod settings;
mod terminal;
mod themes;
mod tools;
mod transcript;
mod ui;
mod updates;
mod usage;
mod workspace;
mod workspaces;

use std::path::PathBuf;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

use workspace::{
    ClosePane, CloseTab, FocusNextPane, FocusPrevPane, NewTab, NewTerminalTab, NextTab, PrevTab,
    Quit, SplitDown, SplitRight, ToggleRail, ToggleSidebar, Workspace,
};

/// `gentle-shell-gpui [FOLDER] [-p|--prompt TEXT]`
struct Cli {
    folder: Option<PathBuf>,
    prompt: Option<String>,
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        folder: None,
        prompt: None,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-p" || arg == "--prompt" {
            cli.prompt = args.next().map(|p| p.to_string_lossy().into_owned());
        } else if cli.folder.is_none() {
            cli.folder = Some(PathBuf::from(arg));
        }
    }
    cli
}

/// The folder the first tab opens: the CLI argument, else the current
/// directory, else home (GUI launches start in `/`).
pub fn default_cwd() -> PathBuf {
    parse_cli()
        .folder
        .filter(|p| p.is_dir())
        .and_then(|p| p.canonicalize().ok())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|p| p.parent().is_some())
        })
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The window icon; GPUI only applies it on X11 (Windows reads the exe's
/// embedded icon, macOS the bundle's `AppIcon.icns`).
#[cfg(target_os = "linux")]
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let png = include_bytes!("../../../packaging/icons/icon-512.png");
    let icon = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    // 128 px keeps the `_NET_WM_ICON` property well under X11 request limits.
    let icon = image::imageops::resize(
        &icon.into_rgba8(),
        128,
        128,
        image::imageops::FilterType::Lanczos3,
    );
    Some(std::sync::Arc::new(icon))
}

fn main() {
    // An explicit folder selects (or creates) the workspace rooted there;
    // without one the last workspace is restored.
    let cli = parse_cli();
    let folder = cli
        .folder
        .filter(|p| p.is_dir())
        .and_then(|p| p.canonicalize().ok());
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
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            themes::init(cx);
            app_update::init(cx);

            // Cmd on macOS, Ctrl elsewhere.
            macro_rules! key {
                ($k:literal) => {
                    if cfg!(target_os = "macos") {
                        concat!("cmd-", $k)
                    } else {
                        concat!("ctrl-", $k)
                    }
                };
            }
            // Split right is ctrl-shift-d off macOS: ctrl-d belongs to terminals.
            let (split_right, split_down, workspaces) = if cfg!(target_os = "macos") {
                ("cmd-d", "cmd-shift-d", "ctrl-cmd-k")
            } else {
                ("ctrl-shift-d", "ctrl-shift-e", "ctrl-alt-k")
            };
            cx.bind_keys([
                KeyBinding::new(key!("t"), NewTab, None),
                KeyBinding::new(key!("shift-t"), NewTerminalTab, None),
                KeyBinding::new(key!("w"), ClosePane, None),
                KeyBinding::new(key!("shift-w"), CloseTab, None),
                KeyBinding::new("ctrl-tab", NextTab, None),
                KeyBinding::new("ctrl-shift-tab", PrevTab, None),
                KeyBinding::new(split_right, SplitRight, None),
                KeyBinding::new(split_down, SplitDown, None),
                KeyBinding::new(key!("]"), FocusNextPane, None),
                KeyBinding::new(key!("["), FocusPrevPane, None),
                KeyBinding::new(key!("b"), ToggleSidebar, None),
                KeyBinding::new(key!("alt-b"), ToggleRail, None),
                KeyBinding::new(key!("q"), Quit, None),
                KeyBinding::new("escape", workspace::CloseOverlay, Some("GentleWorkspace")),
                KeyBinding::new("escape", session::Abort, Some(session::CONTEXT)),
                KeyBinding::new(
                    key!("shift-a"),
                    session::ToggleAgents,
                    Some(session::CONTEXT),
                ),
                KeyBinding::new(key!("n"), session::NewSession, Some(session::CONTEXT)),
                KeyBinding::new(key!("k"), workspace::OpenPalette, None),
                KeyBinding::new(workspaces, workspace::OpenWorkspaces, None),
                KeyBinding::new(key!("shift-o"), workspace::OpenSessions, None),
            ]);
            files::FileView::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![Menu {
                name: "Gentle Shell".into(),
                items: vec![
                    MenuItem::action("New Tab", NewTab),
                    MenuItem::action("New Terminal Tab", NewTerminalTab),
                    MenuItem::action("Split Right", SplitRight),
                    MenuItem::action("Split Down", SplitDown),
                    MenuItem::action("Close Pane", ClosePane),
                    MenuItem::action("Close Tab", CloseTab),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Toggle Side Panel", ToggleRail),
                    MenuItem::separator(),
                    MenuItem::action("Quit Gentle Shell", Quit),
                ],
                disabled: false,
            }]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
                window_min_size: Some(size(px(720.), px(480.))),
                window_background: WindowBackgroundAppearance::Blurred,
                titlebar: Some(TitlebarOptions {
                    title: Some("Gentle Shell".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(13.))),
                }),
                app_id: Some("dev.gentle-shell.gpui".into()),
                #[cfg(target_os = "linux")]
                icon: window_icon(),
                ..TitleBar::window_options()
            };
            integration::register(cx);
            let (handle, workspace) = gpui_kit::open_window(options, cx, |window, cx| {
                let workspace =
                    cx.new(|cx| Workspace::new(folder.clone(), prompt.clone(), window, cx));
                integration::attach_sidebar(&workspace, window, cx);
                workspace
            })
            .expect("failed to open the Gentle Shell window");
            // Requests from later launches: bring this window forward and act on them.
            cx.spawn(async move |cx| {
                while let Ok(request) = requests.recv().await {
                    let _ = handle.update(cx, |_, window, cx| {
                        window.activate_window();
                        workspace.update(cx, |w, cx| {
                            w.open_request(request.folder, request.prompt, window, cx)
                        });
                    });
                    cx.update(|cx| cx.activate(true));
                }
            })
            .detach();
            cx.activate(true);
        });
}
