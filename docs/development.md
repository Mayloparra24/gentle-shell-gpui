# Development

How to build, run, test and release Gentle Shell GPUI, and how to work on the project day to day without touching the developer's real app or data. Boundaries and code rules live in [`architecture.md`](architecture.md) and in the [ADRs](adr/); this page only links to them.

## Requirements

| Requirement | Details |
| --- | --- |
| Rust | Stable, 1.88 or newer (README). The workspace uses `edition = "2024"` and the code uses `if let … && …` (let chains). There is no `rust-toolchain` or `rust-version`: CI installs `dtolnay/rust-toolchain@stable`. |
| Components | `clippy` (CI installs it explicitly) and `rustfmt`. |
| Gentle Shell | Needed for agent tabs: `npm install -g gentle-pi --allow-scripts=gentle-pi` and `gentle-shell setup`, or the standalone install in `~/.gentle-shell/bin/gentle-shell`. Without it the app still opens and shows the setup screen. |
| Node.js | On the PATH (or the one the app installs in `<data dir>/gentle-shell-gpui/toolchain`). |
| Linux | GPUI and WebKitGTK libraries, the same ones CI installs (`.github/workflows/ci.yml`). |
| macOS (packaging) | `codesign`, `ditto`, `hdiutil`, `plutil` (ship with the system/Xcode CLT). Regenerating icons: `python3`, `swift`, `iconutil`. |
| Windows (installer) | Inno Setup 6 (`ISCC.exe`). |

Linux packages (Debian/Ubuntu), copied from CI:

```bash
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev
```

## Workspace

Root `Cargo.toml`: `resolver = "2"`, members `crates/core`, `crates/pi-rpc`, `crates/app`. Version, edition and license live in `[workspace.package]` and the crates inherit them (`version.workspace = true`). Shared dependencies (`anyhow`, `async-channel`, `regex`, `serde`, `serde_json`) are declared in `[workspace.dependencies]`.

| Crate | Package | What it is |
| --- | --- | --- |
| `crates/app` | `gentle-shell-gpui` (binary, `src/main.rs`) | The GPUI app. `build.rs` embeds the icon and version in the Windows `.exe`; on other OSes it does nothing. |
| `crates/core` | `gentle-core` (lib `gentle_core`) | Pure logic. Depends only on `regex`, `serde`, `serde_json`. |
| `crates/pi-rpc` | `pi-rpc` | Launcher resolution, homes, JSONL client, login (`login-helper.mjs`). No GPUI. |

Profiles (`Cargo.toml`):

| Profile | Use |
| --- | --- |
| `dev` | `debug = "line-tables-only"`; dependencies (`[profile.dev.package."*"]`) use `opt-level = 2`, so a `cargo run` without `--release` is usable. |
| `release` | Cargo default: fast to build, for local testing. |
| `dist` | Inherits `release` with `lto = "thin"`, `codegen-units = 1`, `strip = true`. Used by the `packaging/` scripts and the workflows. Binary is ~1/3 smaller, build is much slower. |

## Build and run

```bash
cargo run                         # dev
cargo run --release -- .          # optimized, opens the current folder
cargo build --profile dist -p gentle-shell-gpui   # as in a release
```

### CLI

`crates/app/src/cli.rs`: `gentle-shell-gpui [FOLDER] [-p|--prompt TEXT]`.

| Argument | Effect |
| --- | --- |
| `FOLDER` | Selects (or creates) the workspace with that root. Ignored if it is not a directory. Without a folder, the last workspace is restored. |
| `-p TEXT`, `--prompt TEXT` | Opens a new agent tab and, once the backend starts, runs the text as if it had been typed in the composer (`SessionView::run_text` in `features/session/submit.rs`). |
| `-p "!cmd"` | Runs `cmd` as a shell command in that tab; `-p "!!cmd"` keeps it out of the context (Pi convention). |

`-p` is useful during development to bring the app to a specific state without typing into it.

### Single instance

`crates/app/src/platform/instance.rs`: one app per user, whatever the build. The primary instance listens on a loopback TCP port and registers it with a random token in `<data dir>/gentle-shell-gpui/instance.json` (mode 0600 on Unix). A second launch sends its `OpenRequest { folder, prompt }` to the primary and exits (`Role::Forwarded`); if there is no live listener, it becomes the primary.

Practical consequence: with your installed app open, `cargo run` **does not open a new window**; it sends the folder and prompt to the app that is already running. To run a development build at the same time, change the data dir (see [Isolated testing](#isolated-testing)).

### Data dir

Everything the app stores lives under `dirs::data_dir()/gentle-shell-gpui/`:

| File | Owner |
| --- | --- |
| `instance.json` | `platform/instance.rs` |
| `workspaces.json` | `features/workspaces/store.rs` |
| `settings.json` | `shared/themes.rs` (`store_path`): theme, transparency and the `onboarding` key (`{ "done": true, "version": … }`) that decides the first run (`features/setup/mod.rs`: `first_run`, `mark_done`). |
| `app-update.json` | `features/app_update/cache.rs` |
| `toolchain/` | `features/setup/mod.rs` (Node and npm prefix installed by the app) |

`dirs::data_dir()` is `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux and `%APPDATA%` on Windows. On macOS and Linux it is derived from `HOME`.

### Environment variables

| Variable | Where it is read | Effect |
| --- | --- | --- |
| `GENTLE_SHELL_BIN` | `pi-rpc/src/launch.rs` (`resolve_backend`), `home.rs` | Launcher to use. Normal order: `GENTLE_SHELL_BIN`, standalone install (`bundled_launcher`: `~/.gentle-shell/bin/gentle-shell`; on Windows `%LOCALAPPDATA%\gentle-shell\bin\gentle-shell.cmd` or `%USERPROFILE%\.gentle-shell-bundle\bin\gentle-shell.cmd`), `gentle-shell` on the combined PATH. If set, the config home is not derived from the standalone launcher. |
| `GENTLE_SHELL_ARGS` | `launch.rs` (`extra_args`, `split_args`), `home.rs` | Options before `--mode rpc`, separated by spaces; `"…"` or `'…'` keep a path with spaces in one argument, and backslashes stay literal: `--link`, `--isolated`, `--home "<dir>"`. They also decide which home the app uses for `auth.json` and config. |
| `GENTLE_SHELL_HOME` | `home.rs` | Isolated home (default `~/.gentle-shell/agent`). |
| `GENTLE_SHELL_USER_PI_HOME`, `PI_CODING_AGENT_DIR` | `home.rs` | Pi home for `--link` (default `~/.pi/agent`). |
| `GENTLE_PI_CONFIG_HOME` | `home.rs` (`config_home`) | Wins over any other Gentle Shell config folder. |
| `GENTLE_SHELL_CONFIG` | `pi-rpc/src/home.rs` (`launcher_config_path`), used by `resolve_homes`, `features/updates/` and `features/setup/gentle.rs` | Path to the launcher's `config.json` when set and non-empty, like the launcher's `resolveConfigPath` (default `~/.gentle-shell/config.json`). |
| `GENTLE_PI_BACKGROUND_SUBAGENTS` | `features/settings/snapshot.rs` | Source of the background subagents policy shown in Settings. |
| `GENTLE_SHELL_GENTLE_AI_PIN` | `features/setup/gentle.rs` | Gentle AI pin used by the setup check. |
| `ENGRAM_URL`, `ENGRAM_PORT`, `ENGRAM_HTTP_TOKEN` | `features/memory/client.rs` | Engram server (default port `7437`). |
| `ENGRAM_BIN` | `features/setup/extras.rs` | Engram binary for the check. |
| `SHELL` | `launch.rs` (`login_shell_path`), `features/terminal/mod.rs` | Login shell whose PATH is combined, and the shell for terminal panels. |
| `HOME` / `USERPROFILE` | `home.rs` (`home_dir`), `dirs` | Base of all homes and, on macOS/Linux, of the data dir. |

The app always passes `GENTLE_SHELL_INTERACTIVE_HOST=1` and the combined `PATH` to the backend (`LaunchSpec.env`). The rest of the environment (provider keys, `GENTLE_SHELL_NO_AUTO_SETUP`, …) is inherited.

### Child process PATH

An app launched from Finder or a `.desktop` file inherits a minimal PATH, without nvm/fnm/mise. That is why `pi-rpc` (`launch.rs`, `search_path`) builds its own PATH:

1. Extra directories registered by the app (`pi_rpc::set_extra_path`, called by `setup::apply_toolchain_path` at startup and before each check).
2. The PATH of an interactive login shell: `$SHELL -l -i -c "printf '__GENTLE_PATH_START__%s__GENTLE_PATH_END__' \"$PATH\""`, with a 5 s timeout (Unix only).
3. The inherited PATH, without duplicates.

It is cached until `pi_rpc::forget_cached()`. Every child process that needs the user's tools must use `pi_rpc::child_path()` and be created with `pi_rpc::command` (on Windows it adds `CREATE_NO_WINDOW`).

## Isolated testing

Rule: **never** test against the real app or data. Do not send keystrokes or clicks to an app the user has open, and do not edit their `workspaces.json`/`settings.json`.

### Temporary HOME (first run)

```bash
cargo build
HOME="$(mktemp -d)" ./target/debug/gentle-shell-gpui
```

With an empty `HOME` the data dir is different, so:

- `instance.json` is different: the build becomes the primary even if your app is open.
- There is no `settings.json`: the welcome screen opens (`features/welcome/`), and without `onboarding.done` it opens again on every launch with that same `HOME`.
- There is no `~/.gentle-shell` or `auth.json`: Gentle Shell shows up unconfigured and signed out.

On Linux, if `XDG_DATA_HOME` is set, point it at the temporary directory too. On Windows `HOME` does not change `%APPDATA%`; this method is for macOS and Linux.

### Clean machine (minimal PATH)

A temporary `HOME` is not enough to simulate a machine without Node or Gentle Shell: `pi-rpc` combines the login shell's PATH, which brings nvm, Homebrew, etc. back. To remove it, use a fake `SHELL` that prints a minimal PATH with the markers `login_shell_path` expects:

```bash
sandbox="$(mktemp -d)"
cat > "$sandbox/fake-shell" <<'EOF'
#!/bin/sh
printf '__GENTLE_PATH_START__%s__GENTLE_PATH_END__' "/usr/bin:/bin:/usr/sbin:/sbin"
EOF
chmod +x "$sandbox/fake-shell"
HOME="$sandbox" SHELL="$sandbox/fake-shell" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  ./target/debug/gentle-shell-gpui
```

The inherited `PATH` is also reduced because it is combined after the login one. Side effects: terminal panels also use `$SHELL` (`features/terminal/mod.rs`), so they do not work in this mode; and the initial setup installs really write inside `$sandbox`.

Warning about the standalone launcher: `~/.gentle-shell/bin/gentle-shell` is a generated script with **absolute paths** to the real prefix (`<prefix>/runtime/node-<v>/bin/node`, `<prefix>/current/node_modules/gentle-pi/…`). Copying it into the temporary `HOME` or pointing `GENTLE_SHELL_BIN` at it runs the real Node and package; `bundled_install` and `config_home` derive the prefix from the launcher's location, so a copy does not find `runtime/` and detection is only partial. That environment is not a clean machine.

### Visual checks

Take screenshots of the sandbox window, not of the user's screen. On macOS:

```bash
screencapture -l <windowid> -o /tmp/shot.png
```

Get the window id from `CGWindowListCopyWindowInfo` (for example, a `swift` script that filters by the sandbox process PID). To reach a specific state, use `-p "text"` / `-p "!cmd"` and the CLI folder instead of simulating input.

If reaching a screen needs temporary code (forcing a state, an `eprintln!`, fake data), remove it before committing.

## Before committing

```bash
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs clippy and the tests on macOS, Linux and Windows, but **not** `cargo fmt --check`: formatting is a local responsibility. `--locked` fails if `Cargo.lock` is out of date; commit the lock together with any dependency change. Clippy runs with `-D warnings`, so a `#[cfg(windows)]` with variables unused on macOS breaks CI on another platform (the existing pattern is `#[cfg_attr(not(windows), allow(unused_mut))]`, `features/setup/mod.rs`).

[`architecture.md`](architecture.md) also asks for a smoke test of the changed path; do it in an isolated environment.

### Testing conventions

| Kind | Where |
| --- | --- |
| Pure logic | In `crates/core`, a `#[cfg(test)] mod tests` module at the end of each file. If a rule can live in `core`, its test goes there, not in `app`. |
| `app` logic | `#[cfg(test)] mod tests` in the same file (`features/app_update/release.rs`, `features/notices/store.rs`, …). |
| File effects | Its own `tests.rs` file (`features/profiles/tests.rs`, `features/sessions/tests.rs`, `features/workspaces/tests.rs`) or shared helpers (`features/app_update/mod.rs`: `mod test_support` with `temp_dir`, `release`, `fake_bundle`). |
| `pi-rpc` | `mod tests` in `launch.rs`, `home.rs`, etc.; some Unix only (`#[cfg(all(test, unix))]`). |

There are no integration tests in `tests/` and no GPUI UI tests. File-based tests use `std::env::temp_dir()` with a unique name (`gs-…` prefix, PID and sometimes the time) and delete it at the end; see `platform/instance.rs` (`second_launch_is_forwarded_to_the_primary`). Functions with effects take paths as parameters (for example `bundled_candidates(home, local_app_data, windows)`) so they do not depend on the real `HOME`. What to test and what not to: [ADR-0003](adr/0003-behavior-preserving-refactors.md).

## Code conventions

They are in [`architecture.md`](architecture.md); the essentials:

- One folder per feature in `crates/app/src/features/<name>/` with `mod.rs` as the API; children with `impl` blocks and `pub(super)`.
- `shared/` only for pieces used by several features with no product logic; `platform/` for OS integration.
- Pure logic moves to `gentle-core` one complete behavior at a time ([ADR-0002](adr/0002-incremental-pure-core.md)); no traits for a single implementation.
- Behavior-preserving refactors, kept separate from behavior changes ([ADR-0003](adr/0003-behavior-preserving-refactors.md)); the history marks them with `(no behavior change)`.
- Modular monolith ([ADR-0001](adr/0001-modular-monolith.md)).

## Commits and pull requests

Prefixes used in `git log`:

| Prefix | Use |
| --- | --- |
| `feat(<area>):` | New behavior (`feat(welcome): …`). |
| `fix(<area>):` | Fix (`fix(windows): …`, `fix(session): …`). |
| `refactor(<area>):` | Reorganization; ends in `(no behavior change)`. |
| `test(<area>):`, `docs:`, `perf:`, `ci:` | Less frequent. |
| `release: X.Y.Z` | Only the version bump (`Cargo.toml` + `Cargo.lock`). |

The area is usually the feature name (`session`, `providers`, `app_update`) or the platform. The subject describes the observable effect, in English and lowercase after the colon.

One PR per change, from a `feat/…` or `refactor/…` branch. PRs are merged with a merge commit (no squash): the branch history is kept and the merge carries the PR title with its number (`Welcome screen: … (#9)`).

## CI

| Workflow | Triggers | What it does |
| --- | --- | --- |
| `ci.yml` (CI) | `push` to `main` and `pull_request`, both with `paths-ignore: ["**.md", "packaging/icons/**"]` | Matrix `macos-latest`, `ubuntu-latest`, `windows-latest` (`fail-fast: false`): `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked`. Cache `Swatinem/rust-cache@v2`. |
| `cache-warm.yml` (Cache warm) | `push` to `main` touching `Cargo.lock`, `**/Cargo.toml`, `cache-warm.yml` or `release.yml`; `workflow_dispatch` | Builds `--profile dist` for the four targets and saves the cache with `shared-key: dist-<target>`. It exists because a tag can only restore caches from the default branch. |
| `release.yml` (Release) | Tags `v*`; `workflow_dispatch` (dry run) | See [Releases](#releases). |

Concurrency: CI uses `group: ci-${{ github.ref }}` with `cancel-in-progress: true` (a new push to the same branch cancels the previous one); Cache warm also cancels; Release uses `cancel-in-progress: false`.

A change that only touches Markdown or `packaging/icons/` does not run CI.

## Releases

Assets produced by `release.yml` (the updater consumes these names):

| Platform | Runner / target | Assets |
| --- | --- | --- |
| macOS Apple silicon | `macos-latest`, `aarch64-apple-darwin` | `gentle-shell-gpui-<v>-macos-aarch64.dmg`, `.zip` |
| macOS Intel | `macos-latest` (cross-compiled), `x86_64-apple-darwin` | `gentle-shell-gpui-<v>-macos-x86_64.dmg`, `.zip` |
| Windows | `windows-latest`, `x86_64-pc-windows-msvc` | `gentle-shell-gpui-<v>-windows-x86_64.zip` (exe), `…-windows-x86_64-setup.exe` |
| Linux | `ubuntu-22.04`, `x86_64-unknown-linux-gnu` | `gentle-shell-gpui-<v>-linux-x86_64.tar.gz` |
| All | `ubuntu-latest` | `SHA256SUMS` (`sha256sum` of the seven assets) |

Workflow steps:

1. `version`: reads `[workspace.package] version` from `Cargo.toml`. On a tag, it fails if `GITHUB_REF_NAME != v<version>`; on `workflow_dispatch`, the optional `version` input must match.
2. `build`: restores the `dist-<target>` cache (`save-if: false`), installs libraries on Linux and packages: `packaging/macos/bundle.sh <target> <v>` (with `MACOSX_DEPLOYMENT_TARGET=11.0`, ad hoc signing `codesign --sign -`, zip with `ditto` without `._*`, DMG with a shortcut to `/Applications`), `packaging/linux/package.sh <target> <v>` (binary, `.desktop`, `icon-512.png`), and on Windows `cargo build --profile dist`, `Compress-Archive` and `ISCC` on `packaging/windows/installer.iss` (installs Inno Setup with `choco` if missing).
3. `release`: checks that all seven assets are present, writes `SHA256SUMS` and, only on `push`, publishes with `gh release create --verify-tag --generate-notes`, title `Gentle Shell <v>`. A version with `-` (`0.2.0-rc.1`) is published as `--prerelease`. On `workflow_dispatch` it uploads everything as a `release-dry-run-<v>` artifact without publishing.

### Cutting a release

1. On an up-to-date `main`, change `version` in `[workspace.package]` of the root `Cargo.toml` and run `cargo check` so `Cargo.lock` updates.
2. Commit only those two files: `release: X.Y.Z`. Push to `main`.
3. Wait for CI on `main` to pass (and Cache warm, which triggers because `Cargo.lock` changed, so the tag reuses the cache).
4. Optional: dry run from Actions → Release → Run workflow.
5. `git tag vX.Y.Z && git push origin vX.Y.Z`.
6. Check that the release has all eight files.

### How the updater consumes them

`crates/app/src/features/app_update/`:

| Module | Role |
| --- | --- |
| `network.rs` | `GET https://api.github.com/repos/ismaelosuna7824/gentle-shell-gpui/releases/latest`, download with SHA-256 and progress. |
| `release.rs` | Only counts published releases, not `draft`, not `prerelease`, with a stable `vX.Y.Z`. `asset_name(version, os, arch)` picks `macos-<arch>.zip`, `windows-x86_64.zip` or `linux-x86_64.tar.gz` (there is no asset for Linux/Windows aarch64). `expected_digest` reads `SHA256SUMS`. `is_dev`: `0.0.0` or a non-stable version does not check at startup. |
| `cache.rs` | `app-update.json`: 6 h cache (`TTL_MS`) and skipped version. |
| `archive.rs` | Safe zip and tar.gz extraction. |
| `install.rs` | `install_target`: on macOS replaces the `.app` that contains the executable (without a bundle, or under `AppTranslocation`, it does not install); on Linux the executable (keeps permissions); on Windows the running exe. On Windows and Linux it rejects executables under `target/[<triple>/]{debug,release,dist}/` (`in_cargo_target`). |
| `relaunch.rs` | Process that reopens the app on exit; `remove_leftovers` deletes at startup the `gentle-shell-gpui.exe.old` left by an update on Windows. |

Therefore: renaming an asset, changing the `SHA256SUMS` format or the binary name (`BINARY = "gentle-shell-gpui"`) breaks updates for versions already installed. Prereleases are never offered inside the app. The updater does not use the DMG or the `setup.exe`.

## Platform notes

### macOS

- `packaging/macos/Info.plist` (identifier `io.github.ismaelosuna7824.gentle-shell-gpui`, `@VERSION@` and `@MIN_MACOS@` replaced by `bundle.sh`). `GENTLE_BIN=<path>` packages an already built binary.
- Outside a `.app` (`cargo run`), `platform/icons.rs` (`set_dock_icon`) sets the Dock icon at runtime from `icon-512.png`; inside the bundle it uses `AppIcon.icns`.
- Ad hoc signing, not notarized: Gatekeeper blocks the first launch (README). `bundle.sh` clears xattrs before signing because they would break the seal.

### Windows

- `main.rs`: `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`. A release build has no console (no visible `stderr`); a debug build does.
- Every child process goes through `pi_rpc::command` (`CREATE_NO_WINDOW`) so it does not open console windows.
- `crates/app/build.rs` embeds `packaging/icons/icon.ico` (resource 1, the one GPUI uses for the window) and the package version with `embed-resource`.
- `installer.iss`: per user (`PrivilegesRequired=lowest`), in `{localappdata}\Programs\Gentle Shell`, `CloseApplications=yes`, `RestartApplications=yes`. Do not change `AppId`: Windows identifies updates and the uninstaller by it.

### Linux

- GPUI only applies the window icon on X11: `platform/icons.rs` (`window_icon`) scales it down to 128 px because of `_NET_WM_ICON` limits. `app_id` is `dev.gentle-shell.gpui`, the same as `StartupWMClass` in `packaging/linux/gentle-shell-gpui.desktop`.
- The built-in browser uses WebKitGTK as a child of the X11 window (`wry` with `x11`, `gtk`); Wayland has no child windows.
- At runtime `libwebkit2gtk-4.1-0` is required.

### Icons

Source: the `ROSE` art in `crates/app/src/shared/banner.rs`. After editing it, on macOS:

```bash
packaging/icons/generate.sh
```

It runs `generate.py` (SVG), `render.swift` (PNG in several sizes), `iconutil` (`AppIcon.icns`) and `ico.py` (`icon.ico`), and writes `icon-1024.png`, `icon-512.png`. CI never runs it: commit the results. `icon-512.png` is embedded in the binary with `include_bytes!`, so an icon change requires a rebuild.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `cargo run` opens no window and exits | Another instance (your installed app) received the request. Use a temporary `HOME`. |
| `gentle-shell was not found on your PATH` | `resolve_backend` found no launcher. Install `gentle-pi`, or set `GENTLE_SHELL_BIN`. |
| Node is not found when launched from Finder/`.desktop` | The app inherits a minimal PATH. Check that your login shell (`$SHELL -l -i`) exports the nvm/fnm/mise PATH and responds in under 5 s; extra output does not matter (it is trimmed between markers). After installing something, `pi_rpc::forget_cached()` recomputes the PATH. |
| Profiles, model routes or permissions stopped applying after updating Gentle Shell to 4.7 | The standalone install reads its config from `<prefix>/config/gentle-ai`, not from `~/.gentle-shell/gentle-ai`. `main.rs` calls `pi_rpc::adopt_standalone_config()` at startup: it copies what is missing once (does not overwrite; leaves the `.adopted-standalone-config` marker) and reports on `stderr`. |
| A global `gentle-pi` on the PATH is not used | The standalone install (`bundled_launcher`) wins unless you set `GENTLE_SHELL_BIN`. |
| The welcome screen appears on every launch | `onboarding.done` is missing from `settings.json` in the data dir (normal with a temporary `HOME`). |
| The app does not offer or does not install an update | Not offered: development version (`is_dev`), 6 h cache in `app-update.json`, skipped version, or the latest release is a prerelease. Not installed: binary outside a `.app` (macOS) or under `target/` (`install_target`). |
| Signing in to or out of a provider restarts the tabs | The backend only reads credentials at startup; this is intentional (README, Providers). |
| CI fails only on Linux | Missing system libraries, or code under `cfg(target_os = "linux")` that is not compiled on macOS; check the clippy warnings for that platform. |
| `--locked` fails in CI | `Cargo.lock` was not committed after changing dependencies or the version. |

## Checklist

### Pull request

- [ ] One feature or fix per PR; refactors separate and marked `(no behavior change)`.
- [ ] Code in its feature's folder; pure logic in `gentle-core` with tests.
- [ ] Tests for new rules, limits or errors; file-based ones use unique temporary directories.
- [ ] `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- [ ] Smoke test in a temporary `HOME`; screenshots only of the sandbox window.
- [ ] No temporary code for reaching states.
- [ ] `Cargo.lock` included if it changed.
- [ ] `architecture.md` or an ADR updated if a boundary changes; README if something visible changes.
- [ ] Commits prefixed `feat|fix|refactor|test|docs(<area>):`.

### Release

- [ ] `main` green in CI.
- [ ] `version` changed in `[workspace.package]`, `cargo check` run, `release: X.Y.Z` commit with `Cargo.toml` and `Cargo.lock`.
- [ ] CI for that commit green (and Cache warm finished).
- [ ] Optional: Release dry run.
- [ ] Tag `vX.Y.Z` matching the workspace version, pushed.
- [ ] Release published with 7 assets and `SHA256SUMS`; names unchanged.
- [ ] If stable, the previously installed version offers the update (Settings → App → Check for updates).
