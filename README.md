# gentle-shell-gpui

A native desktop app for [Gentle Shell](https://github.com/Gentleman-Programming/gentle-shell), built in Rust with [GPUI](https://gpui.rs) and [GPUI Kit](https://github.com/longbridge/gpui-kit). Use Gentle Shell from a window instead of a terminal. The layout follows [Tern](https://stencil.so/tern): translucent glass chrome, tabs in the title bar, a centered composer, and tool calls shown as cards.

The app does not reimplement Gentle Shell. Each tab starts `gentle-shell --mode rpc` and talks to it over Pi's [RPC protocol](https://pi.dev/docs/latest/rpc), so el Gentleman, ODD, Gentle Agents, permissions, Engram memory, review, skills and every extension run unchanged. The app sets `GENTLE_SHELL_INTERACTIVE_HOST=1`, which enables `ask_user_question` dialogs and the live helper activity feed. Gentle Shell's own screens are terminal-UI components that never reach an RPC host, so the app rebuilds them natively from the same files and events.

## Features

| Area | What the app does |
| --- | --- |
| Workspaces and tiling | Named workspaces (each with its own folder and tabs), saved and restored; agents in other workspaces keep running while you switch. Tabs split into panes: agent, terminal or file. Panes have no header: split right (⌘D), split down (⌘⇧D) and close pane (⌘W) sit in the title bar and act on the focused pane. ⌘] / ⌘[ move focus. Drag a divider to resize or a pane's grip to swap it. ⌃⌘K switches workspace. The sidebar remembers whether it was open. |
| Terminal panes | Your login shell in a real PTY (alacritty_terminal) with the Gentleman colors; ⌘⇧T opens a terminal tab. |
| Files sidebar (⌘B) | Files, Changes and Branches tabs. The tree respects `.gitignore` (⋯ → Show Ignored Files lists the rest dimmed), navigates with the arrow keys, and has New File / New Folder buttons with inline naming. Right-click a row for Open, Open to the Side (⌘↵), Open in New Tab (⌥⌘↵), Open with Default App, Insert Path into the agent prompt (⌥↵), Copy Path (⌥⌘C) / Relative Path (⌥⇧⌘C), Reveal in Finder (⌥⌘R), Rename (F2), Duplicate, Cut / Copy / Paste (⌘X / ⌘C / ⌘V) and Move to Trash (⌘⌫). Files open in a syntax-highlighted editor: ⌘S saves atomically, a dot marks unsaved edits, outside writes reload a clean buffer and ask (Reload / Keep mine) when you have unsaved edits, and closing a pane or tab with unsaved edits asks first. Markdown files open rendered, with an Edit · Split · Preview toggle in the file header; the preview follows unsaved edits live (relative images load from the file's folder) and the last choice is remembered for the session. Changes open as a read-only diff against HEAD. **Changes** (after Tern): Unstaged and Staged lists (or by folder) with a checkbox per file, Stage all / Unstage all, stash, discard on right-click, and a commit box (summary, description, Amend, Commit / Commit & Push with ⌘↵); "Generate message" asks your Gentle Shell (`--print --no-session --no-tools`) for a Conventional Commits message from the staged diff. **Branches**: filterable branches, remotes, tags, stashes (apply / pop / drop), worktrees and submodules; click a branch to check it out; the footer shows upstream and ↑ahead ↓behind. |
| Start screen | Gentle Shell's rose and pixel wordmark, written in like the terminal intro. It follows `banner.json` (`showRose`, `showTextLogo`, `color`). |
| Conversation | Streaming Markdown, collapsible thinking, and tool cards (`Edit path +12 −3`, `Bash …`) with live output. Helper answers (`gentle-agents.result`) appear as cards. |
| Composer | Enter sends; while the agent runs, Enter steers it and Esc stops it. `!cmd` / `!!cmd` run shell commands. `/` lists commands. Shows model, thinking level, context %, cost, the ODD phase (`✿ exploring…`), extension statuses, queued messages and running jobs. |
| Attachments | Attach images with 📎, by pasting (screenshots or copied image files) or by dropping them on the composer. Each image becomes an `[Image #N]` token and a thumbnail, and the agent receives the images with the prompt. Models without image input and `!cmd` shell commands refuse attachments with a warning. |
| Notifications (🔔) | Extension notices go to a notification center instead of piling up as toasts; only warnings and errors still toast. The badge counts unread notices, repeats collapse into `×N`, and clicking a notice jumps to the pane that raised it. |
| Sounds | App tabs follow Gentle Shell's `notifications.json` (the file `/gentle:customize` edits in the terminal; Configuration › Notifications). Off by default. When on, a finished turn, a question for you, or context ≥ 90% plays Gentle Shell's tones (macOS `afplay`, Windows PowerShell `SoundPlayer`, Linux `paplay` / `pw-play` / `aplay` when installed, otherwise silent) and bounces the Dock or flashes the taskbar while the app is in the background. |
| Providers (🔑) | Sign in with a subscription (Claude Pro/Max, ChatGPT, Copilot, …) or an API key, or sign out. It runs Pi's own `ModelRuntime.login` through a small Node helper, and Pi writes `auth.json` as `/login` does. |
| Agents (⌘⇧A) | Gentle Agents helpers: status, time, turns, tools, task, last step, and the thread (Activity tab). The app keeps finished helpers after Gentle Shell drops them. "Ask to stop" asks el Gentleman to run `subagent_cancel`, because Gentle Shell has no host-side stop. Configuration → Background subagents (Off / On) runs Gentle Shell's own `/gentle:background-subagents enable|disable` in the active agent tab: with it on, helpers start in the background and their answers arrive when they finish. The row shows which source decides (project file, global file, `GENTLE_PI_BACKGROUND_SUBAGENTS`, default), in the shell's order. |
| Profiles | `/gentle:profiles` rebuilt: profiles, per-agent model, effort and Fast tier. Supports Use in this session, Make default, Publish and Clear binding. |
| Permissions | Normal / Auto / YOLO / Super YOLO picker. Gentle Shell only changes a live session's mode from its terminal UI, so the app saves the default (`permission-mode.json`) and starts a new session. |
| Sessions (⌘⇧O) | Resume previous conversations for this project or any project; sessions from another project open in a new tab. ⌘N starts a new session. |
| Usage & stats | Subscription windows (ChatGPT/Codex, NaN) and local token/cost stats from session files, in place of `/gentle:usage` and `/gentle:stats`. |
| Session chips | As in Tern, what a session tracks sits in chips over the composer instead of a side panel, so every pane keeps its full width: **Todos 3/7** (the task list), background **Jobs**, **Helpers 1/3** (each subagent's status, task and last step, with a button to the full Agents window), extension widgets and the last response's tokens. Click a chip for its card. |
| Command palette (⌘K) | Categories: All, Commands, Workspaces, Files, Branches. Includes every Gentle Shell slash command and skill. ↑↓ move, ↵ runs, Tab or ←→ (while the search is empty) switch category, Esc closes. Gentle Shell commands whose screen only exists in the terminal open the app's own: `/gentle:usage` and `/gentle:stats` open Usage (Subscription / Local stats), `/gentle:agents` and `/gentle:jobs` the Agents window, `/gentle:profiles` (no arguments) Profiles, `/gentle:commands` ⌘K, `/gentle:changes` Files › Changes. `/gentle:customize`, `/gentle:guide`, `/gentle:voice`, `/gentle:music`, `/gentle:panel` and `/history` do nothing over RPC and are hidden. Double-click an agent tab to rename its session. |
| Configuration | Your existing Gentle Shell setup: home, models, profiles, sign-ins, permission mode, persona, MCP, packages (install and remove), Claude bridge plan, and the theme picker: Gentle Shell's themes, your Pi themes (`<agent dir>/themes/`) and popular terminal schemes, applied app-wide and to terminals, remembered across launches. A Transparency slider (default 60%) sets how much of the blurred desktop shows through the window, as in Tern; menus, the palette and screens such as Profiles stay solid at any setting. When a newer Gentle Shell is published, a card offers Update now (runs `gentle-shell upgrade`, then restarts open agent tabs on the new version and resumes their conversations; a tab mid-turn switches when the turn ends), Release notes and Skip this version. The check mirrors the launcher's own startup check: npm `gentle-pi/latest`, stable versions only, cached for a day, with the cache and the skipped version shared with the terminal in `~/.gentle-shell/config.json`. Every new agent tab already runs whichever `gentle-shell` is on your PATH, so updating outside the app works too. |

## Requirements

- Gentle Shell: `npm install -g gentle-pi --allow-scripts=gentle-pi` (npm 11 blocks its postinstall otherwise), then `gentle-shell setup` once. Setup installs Engram through Homebrew; on macOS that may first need `brew trust gentleman-programming/tap`.
- Node.js on your PATH. The app merges in your login shell's PATH, so nvm, fnm and mise work even when you launch the app from Finder.
- Rust 1.88 or newer.
- Linux also needs the GPUI system libraries, for example on Debian or Ubuntu: `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev`.

## Install and run

### From a release

Download the asset for your system from [Releases](https://github.com/ismaelosuna7824/gentle-shell-gpui/releases) (names under [Packages and releases](#packages-and-releases)).

- **macOS:** open `gentle-shell-gpui-<version>-macos-aarch64.dmg` (Apple silicon) or `…-macos-x86_64.dmg` (Intel) and drag **Gentle Shell** onto **Applications**. The app is ad-hoc signed, not notarized, so the first launch is blocked: right-click the app → **Open** → **Open**, or allow it in System Settings → Privacy & Security → **Open Anyway**. From a terminal, `xattr -dr com.apple.quarantine /Applications/"Gentle Shell.app"` does the same. Later versions install from inside the app ([Updating the app](#updating-the-app)).
- **Windows:** unzip and run `gentle-shell-gpui.exe`. SmartScreen may warn about an unknown publisher: **More info** → **Run anyway**.
- **Linux:** extract the tarball and run `gentle-shell-gpui-<version>/gentle-shell-gpui`. Copy the `.desktop` file to `~/.local/share/applications/` (and fix its `Exec=` path) to get a launcher entry.

### From source (no release installed)

1. Install the [requirements](#requirements): Gentle Shell, Node.js, Rust (`curl https://sh.rustup.rs -sSf | sh`), and on Linux the system libraries.
2. Build and run:

   ```bash
   git clone https://github.com/ismaelosuna7824/gentle-shell-gpui.git
   cd gentle-shell-gpui
   cargo run --release -- .
   ```

   The first build takes a few minutes. `.` opens the current folder as a workspace; pass any folder instead, or nothing to reopen the last workspace. `--prompt "first message"` starts a new agent tab with that message.
3. Optional, macOS: to run it as a normal app (rose icon in the Dock, Finder, Spotlight), bundle it and copy it to `~/Applications`:

   ```bash
   packaging/macos/bundle.sh aarch64-apple-darwin 0.1.0   # x86_64-apple-darwin on Intel
   cp -R "dist/Gentle Shell.app" ~/Applications/
   open ~/Applications/"Gentle Shell.app"
   ```

A build from source never updates itself; `git pull` and run `cargo run --release` again.

### Options

```bash
cargo run --release -- [FOLDER] [--prompt "first message"]
```

| Variable | Effect |
| --- | --- |
| `GENTLE_SHELL_BIN` | Path of the launcher to use instead of `gentle-shell` on `PATH`. |
| `GENTLE_SHELL_ARGS` | Launcher options placed before `--mode rpc`, such as `--link` or `--home <dir>`. The app also uses them to find `auth.json` and the config home. |

Only one app runs per user, whatever the build: a second launch hands its folder and `--prompt` to the running window and exits.

Gentle Shell's own variables (`GENTLE_PI_CONFIG_HOME`, `GENTLE_SHELL_NO_AUTO_SETUP`, provider API keys, and the rest) pass through to the backend.

## Packages and releases

The app icon (the Gentle Shell pixel rose) lives in `packaging/icons/`. `generate.py` builds it from the `ROSE` art in `crates/app/src/banner.rs`; run `packaging/icons/generate.sh` on macOS after editing and commit the outputs (`AppIcon.icns`, `icon.ico`, PNGs). A bare `cargo run` shows a generic icon in the Dock; the bundled app shows the rose.

Build packages locally:

- macOS: `packaging/macos/bundle.sh aarch64-apple-darwin 0.1.0` → `dist/Gentle Shell.app`, `dist/gentle-shell-gpui-0.1.0-macos-aarch64.zip` and `.dmg` (`x86_64-apple-darwin` for Intel; it cross-compiles from Apple silicon). `GENTLE_BIN=<path>` packages an existing build instead.
- Linux: `packaging/linux/package.sh x86_64-unknown-linux-gnu 0.1.0` → `dist/gentle-shell-gpui-0.1.0-linux-x86_64.tar.gz`.
- Windows: the icon and version info are embedded in the exe by `crates/app/build.rs`.

Build profiles: `cargo run --release` uses Cargo's default release settings, so local optimized builds stay quick. Packages use `--profile dist` (thin LTO, one codegen unit, stripped): about a third smaller, much slower to build. The scripts and the Release workflow build `dist`.

Workflows: **CI** runs clippy and the tests on macOS, Linux and Windows for pushes to `main` and pull requests (Markdown-only changes skip it). **Release** builds and publishes packages for `v*` tags. **Cache warm** builds the `dist` profile on `main` only when `Cargo.lock`, a `Cargo.toml` or the release workflows change, and saves the dependency cache that tag builds restore (GitHub doesn't let one tag reuse another tag's cache).

Cut a release:

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml`, run `cargo check` so `Cargo.lock` updates, and commit.
2. Optional dry run: Actions → Release → Run workflow. It builds all four targets and uploads the assets and `SHA256SUMS` as an artifact without publishing.
3. `git tag v0.2.0 && git push origin main v0.2.0`. The Release workflow checks the tag against the workspace version, builds macOS (arm64 and x86_64), Windows and Linux, writes `SHA256SUMS` and publishes the GitHub release with generated notes. A version with `-` (`0.2.0-rc.1`) is a prerelease.

Assets: `gentle-shell-gpui-<v>-macos-aarch64.dmg` / `.zip` and `…-macos-x86_64.dmg` / `.zip` (the same ad-hoc signed `Gentle Shell.app`: the DMG to install by dragging, the zip for the in-app updater), `…-windows-x86_64.zip` (the exe), `…-linux-x86_64.tar.gz` (binary, `.desktop`, icon) and `SHA256SUMS`. The in-app updater relies on these names.

### Updating the app

On startup (at most every 6 hours) the app checks this repo's GitHub releases and shows an **Update** pill in the title bar when a newer version exists. Configuration → App shows the current version and has Check for updates, Release notes, **Download & install** and Skip this version. Nothing installs without your click; the download is verified against the release's `SHA256SUMS` first. Then **Restart now** saves your workspaces, quits and opens the new version.

- macOS: the `.app` is replaced in place; if its folder isn't writable (e.g. `/Applications` without permission) the app says so. Builds run with `cargo run` don't self-update.
- Windows: the old exe is kept as `gentle-shell-gpui.exe.old` until the next start.
- Linux: the binary is replaced in place, keeping its permissions.

The check cache and the skipped version are in `<data dir>/gentle-shell-gpui/app-update.json`.

## Limits of an RPC host

- Gentle Shell does not publish model, tokens or cost for live helpers.
- Claude subscription windows come only from response headers inside the terminal UI.
- Helpers and background jobs can only be stopped by the agent.
- Auto permission mode may refuse actions: Gentle Shell does not count RPC input as a live human decision.

## Not done yet

Known gaps and things not tested yet, so you know what to expect:

- **First release:** v0.1.0 is the first one, so the in-app updater (download, checksum, replace, restart) can only be tested end to end once v0.1.1 exists. The Windows and Linux builds first compile in that release's CI; they have only been type-checked on macOS.
- **Signing:** macOS builds are ad-hoc signed, not notarized, and Windows builds are unsigned, hence the first-launch warnings above.
- **Claude usage:** Usage & stats shows nothing for Claude through `pi-claude-bridge`. Gentle Shell keeps those 5h/7d windows in its terminal UI only; the app sees the bridge's high-usage warnings (the composer's `7d 57%` chip) but not the full windows. Reading Claude Code's login from the Keychain to ask Anthropic's usage endpoint is possible but not implemented.
- **Quitting with unsaved files:** closing a pane or tab with unsaved edits asks first, but ⌘Q does not.
- **Commit message generation** has no timeout or cancel; if `gentle-shell` hangs, the spinner stays.
- **Worktrees and submodules** in the Branches tab are shown but have no actions.
- **Symlinked folders:** a workspace opened through a symlink shows no change markers in the file tree (git reports the real path).
- **Workspace switcher** (⌃⌘K) doesn't show that an agent is still working in another workspace.
- **Hidden tabs** restored at launch start their backend only when shown, so their title shows the folder name until then.
- **Markdown preview:** links to other `.md` files open in the default app, not in a pane, and `#anchor` links do nothing.
- **Visual checks:** several screens were only checked from code, not on screen: the chips' cards, the App section of Configuration, the update pill, the Background subagents switch, and the Changes / Branches tabs with real repositories.

## Layout

| Crate / module | Role |
| --- | --- |
| `crates/pi-rpc` | Launcher and home resolution (matching Gentle Shell's launcher), the JSONL client, typed extension UI requests, and the sign-in helper. |
| `app/session` | One tab: RPC state, transcript, composer and its status chips, agents panel. |
| `app/workspace.rs` | The window, tabs, and window-level screens. |
| `app/{providers,profiles,sessions,usage,palette}.rs` | The native screens. |
| `app/{transcript,activity,gentle_state,tools,git,banner}.rs` | Pure state rebuilt from events and files. |
| `app/themes.rs` | Theme catalog, default choice (saved choice, then Pi's `theme` setting, then Gentleman-Cute) and persistence; `ui.rs` resolves it into the palette. |

```bash
cargo test
```
