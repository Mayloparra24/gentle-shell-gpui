//! The project file browser: [`FileTreeView`] (sidebar with the lazily
//! loaded tree, the working-tree changes and the branches) and [`FileView`]
//! (editable, syntax-highlighted file — ⌘S saves — or read-only diff pane).

mod changes;
mod listing;
mod ops;
mod refs;
mod tree;
mod viewer;

pub use tree::{FileTreeEvent, FileTreeView, init};
pub use viewer::FileView;

use std::path::Path;

use gpui_kit::Hsla;
use gpui_kit::assets::IconName;

use crate::shared::ui;

/// The icon and tint for a file name in the tree.
fn file_icon(name: &str) -> (IconName, Hsla) {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match (lower.as_str(), ext) {
        ("cargo.toml" | "package.json" | "pyproject.toml" | "go.mod", _) => {
            (IconName::Package, ui::champagne())
        }
        (n, _) if n.starts_with("readme") => (IconName::Info, ui::info()),
        (n, _) if n.starts_with(".git") => (IconName::GitBranch, ui::muted()),
        (n, _) if n.ends_with(".lock") || n.ends_with("-lock.json") || n == "pnpm-lock.yaml" => {
            (IconName::Lock, ui::faint())
        }
        (_, "rs") => (IconName::FileCode, ui::warning()),
        (
            _,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "c" | "h" | "cc" | "cpp"
            | "hpp" | "java" | "kt" | "swift" | "rb" | "lua" | "zig" | "cs" | "php" | "scala"
            | "ex" | "exs" | "sh" | "bash" | "zsh" | "fish" | "sql" | "html" | "css" | "scss"
            | "vue" | "svelte" | "astro",
        ) => (IconName::FileCode, ui::info()),
        (_, "json" | "jsonc" | "json5") => (IconName::Braces, ui::champagne()),
        (_, "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env") => {
            (IconName::Settings2, ui::muted())
        }
        (_, "md" | "mdx" | "txt" | "rst") => (IconName::FileText, ui::muted()),
        (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp") => {
            (IconName::Image, ui::petal())
        }
        _ => (IconName::File, ui::muted()),
    }
}

/// `(highlighter token, label)` for a path. The token goes to the editor's
/// syntect backend, which resolves names and extensions; `None` is plain text.
fn language(path: &Path) -> (Option<&'static str>, &'static str) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let pick = match (name.as_str(), ext) {
        ("makefile" | "gnumakefile", _) => ("make", "Makefile"),
        ("dockerfile", _) => ("dockerfile", "Dockerfile"),
        ("cargo.lock", _) => ("toml", "TOML"),
        (_, "rs") => ("rust", "Rust"),
        (_, "toml") => ("toml", "TOML"),
        (_, "md" | "mdx") => ("markdown", "Markdown"),
        (_, "json" | "jsonc") => ("json", "JSON"),
        (_, "ts" | "mts" | "cts") => ("typescript", "TypeScript"),
        (_, "tsx") => ("tsx", "TSX"),
        (_, "js" | "mjs" | "cjs" | "jsx") => ("javascript", "JavaScript"),
        (_, "py" | "pyi") => ("python", "Python"),
        (_, "go") => ("go", "Go"),
        (_, "sh" | "bash" | "zsh") => ("bash", "Shell"),
        (_, "yaml" | "yml") => ("yaml", "YAML"),
        (_, "html" | "htm") => ("html", "HTML"),
        (_, "css" | "scss") => ("css", "CSS"),
        (_, "c" | "h") => ("c", "C"),
        (_, "cc" | "cpp" | "cxx" | "hpp" | "hxx") => ("cpp", "C++"),
        (_, "java") => ("java", "Java"),
        (_, "kt" | "kts") => ("kotlin", "Kotlin"),
        (_, "swift") => ("swift", "Swift"),
        (_, "rb") => ("ruby", "Ruby"),
        (_, "lua") => ("lua", "Lua"),
        (_, "zig") => ("zig", "Zig"),
        (_, "sql") => ("sql", "SQL"),
        (_, "php") => ("php", "PHP"),
        (_, "cs") => ("csharp", "C#"),
        (_, "diff" | "patch") => ("diff", "Diff"),
        (_, "xml" | "svg") => ("xml", "XML"),
        _ => return (None, "Plain text"),
    };
    (Some(pick.0), pick.1)
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

/// Color for a collapsed porcelain status letter (see [`gentle_core::git::status::FileChange`]).
fn status_color(status: char) -> Hsla {
    match status {
        'A' | '?' => ui::success(),
        'D' => ui::danger(),
        'R' => ui::info(),
        _ => ui::warning(),
    }
}

#[cfg(test)]
mod tests {
    use super::{human_size, language};
    use std::path::Path;

    #[test]
    fn languages_and_sizes() {
        assert_eq!(language(Path::new("src/main.rs")), (Some("rust"), "Rust"));
        assert_eq!(language(Path::new("Cargo.lock")), (Some("toml"), "TOML"));
        assert_eq!(language(Path::new("LICENSE")), (None, "Plain text"));
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
