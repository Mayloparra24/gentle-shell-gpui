//! Command line: `gentle-shell-gpui [FOLDER] [-p|--prompt TEXT]`.

use std::path::PathBuf;

/// `gentle-shell-gpui [FOLDER] [-p|--prompt TEXT]`
pub struct Cli {
    pub folder: Option<PathBuf>,
    pub prompt: Option<String>,
}

pub fn parse_cli() -> Cli {
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
        .and_then(|p| dunce::canonicalize(p).ok())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|p| p.parent().is_some())
        })
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}
