# AGENTS.md

Short context for anyone working in this repository, person or AI agent. The details live in [`docs/`](docs/README.md); this file only covers what you need to know before touching code.

## What it is

A desktop app in Rust (GPUI + gpui-kit) for Gentle Shell's coding agents. Each agent tab is a `gentle-shell --mode rpc` process the app talks to over JSONL (`crates/pi-rpc`). It runs on macOS, Windows and Linux.

## Commands

```bash
cargo run -p gentle-shell-gpui -- [FOLDER] [-p "text"]   # -p "!cmd" runs a shell command
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test
```

Before every commit: `fmt`, `clippy` and `test` must pass. CI runs `clippy` and `test` on all three operating systems.

## Where things go

| You need… | It goes in |
| --- | --- |
| A new screen or feature | `crates/app/src/features/<name>/` (its own folder, `mod.rs` as the entry) |
| Pure logic: parsers, rules, calculations, formatting | `crates/core/src/` (`gentle-core`), with its tests |
| Talking to the Gentle Shell process, resolving its launcher or its homes | `crates/pi-rpc/src/` |
| A visual piece used by several screens | `crates/app/src/shared/` (mostly `ui.rs`) |
| Something specific to one operating system | `crates/app/src/platform/` |

Dependency rules and the anatomy of a feature: [`docs/architecture.md`](docs/architecture.md).

## Rules

- **Design:** use the tokens and components in `crates/app/src/shared/ui.rs`; don't invent one-off colors, borders or animations. The window chrome has no lines: separate with space and surface changes. Inside an overlay screen, the only lines are the thin ones from `ui::screen_header` and the footers. See [`docs/design-system.md`](docs/design-system.md).
- **Behavior-preserving refactors** ([ADR 0003](docs/adr/0003-behavior-preserving-refactors.md)): moving code doesn't change what it does; a fix found along the way goes in its own commit.
- **No single-implementation abstractions:** don't create traits, adapters or new crates without a real second need.
- **Tests:** pure logic is tested in `gentle-core`; file effects use temporary directories. No tests that only restate the implementation.
- **Gentle Shell configuration:** the app reads and writes files that Gentle Shell also owns. Before touching one, check who owns it in [`docs/integrations.md`](docs/integrations.md).
- **Windows and Linux:** code under `#[cfg(windows)]` or `#[cfg(target_os = "linux")]` doesn't compile on macOS; be careful with imports and paths there, and rely on CI to check it.

## Testing without breaking anything

- **Never test on someone's real app:** never send clicks or keystrokes to an app a person is using.
- **Open an isolated copy (macOS and Linux):** `HOME=$(mktemp -d) target/release/gentle-shell-gpui` opens the app as a first run (welcome included), without touching real data or clashing with the running instance. On Windows the data goes to `%APPDATA%`, so a temporary `HOME` doesn't isolate it.
- **Temporary scaffolding:** if reaching a state needs a code change (a timer, an environment variable), remove it before committing and check with `grep` that nothing is left.

Details, "clean machine" variants and screenshots: [`docs/development.md`](docs/development.md).

## Releasing

`[workspace.package] version` in `Cargo.toml` → commit `release: X.Y.Z` → `main` CI green → tag `vX.Y.Z` → the `release.yml` workflow builds and publishes. Full steps in [`docs/development.md`](docs/development.md).
