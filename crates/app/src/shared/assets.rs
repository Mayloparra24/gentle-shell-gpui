//! Icons beyond GPUI Kit's default set, embedded on top of its `Assets`.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Activity,
        CodeXml,
        FilePen,
        Save,
        Wrench,
        Shield,
        Sparkles,
        Flame,
        MessageSquare,
        Braces,
        FileCode,
        FileDiff,
        Files,
        Image,
        Lock,
        Package,
        Funnel,
        OctagonX,
        Columns2,
        Rows2,
        GripHorizontal,
        Pencil,
        Bot,
        Brain,
        Command,
        Gauge,
        GitBranch,
        KeyRound,
        Layers,
        ListTodo,
        LogIn,
        LogOut,
        RefreshCw,
        RotateCcwClock,
        Settings,
        ShieldCheck,
        SquareTerminal,
        Trash,
        Unlink,
        Upload,
        Users,
        X,
        Zap,
        Paperclip,
        // Configuration → Water reminder.
        Droplets,
        // File tree header and context menu.
        FilePlus,
        FolderPlus,
        FolderInput,
        ArrowUpRight,
        Terminal,
        CopyPlus,
        Scissors,
        ClipboardPaste,
        ChevronsDownUp,
        Diff,
        // Changes and Branches tabs.
        List,
        ListTree,
        Archive,
        Tag,
        Cloud,
        FolderGit,
        GitCommitHorizontal,
        // Effort levels (off, minimal, low, high; Brain, Zap and Flame above).
        Moon,
        Feather,
        Leaf,
        BrainCircuit,
        // In-app browser panel.
        Maximize2,
        Minimize2,
        // Device cards and the instance tray.
        Smartphone,
        MonitorSmartphone,
        Power,
        // The music player.
        Music,
        SkipForward,
        Volume2,
        Square,
        // The device panel.
        Apple,
        PanelRight,
        PictureInPicture2
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
