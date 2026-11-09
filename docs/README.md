# Documentation

Gentle Shell GPUI is a desktop app in Rust ([GPUI](https://www.gpui.rs) + gpui-kit) that gives [Gentle Shell](https://github.com/Gentleman-Programming/gentle-shell)'s agents a native interface: each agent tab launches `gentle-shell --mode rpc` and talks to it over JSONL. This folder explains how it is built, how it integrates and how to work on it.

## What to read for what you're doing

| You're going to… | Read |
| --- | --- |
| Understand how the code is organized and where something new goes | [`architecture.md`](architecture.md) |
| Build or change a screen, a component or an animation | [`design-system.md`](design-system.md) |
| Talk to Gentle Shell, read or write its configuration, or add an outside service | [`integrations.md`](integrations.md) |
| Build, test without touching your real app, open a PR or publish a release | [`development.md`](development.md) |
| Know why something was decided | [`adr/`](adr/) |
| Install or use the app | [`../README.md`](../README.md) |

## Quick map

```text
crates/
├── app/       # the gentle-shell-gpui binary: GPUI, screens, effects (files, processes, network)
│   └── src/
│       ├── main.rs, app/, cli.rs   # startup and composition root
│       ├── features/<name>/        # one folder per feature (session, workspace, welcome, …)
│       ├── platform/               # single instance, per-OS icons
│       └── shared/                 # ui.rs (design system), themes, banner, assets, …
├── core/      # gentle-core: pure logic, no GPUI or IO; rule and parser tests live here
└── pi-rpc/    # Gentle Shell launcher, homes, JSONL client, provider sign-in
```

## Current decisions

| ADR | In one line |
| --- | --- |
| [0001](adr/0001-modular-monolith.md) | One binary organized by feature, with explicit boundaries. |
| [0002](adr/0002-incremental-pure-core.md) | Pure logic lives in `gentle-core` and grows one complete behavior at a time. |
| [0003](adr/0003-behavior-preserving-refactors.md) | Refactors don't change behavior; fixes go in separate commits. |

For an AI agent or someone new, the short entry point is [`../AGENTS.md`](../AGENTS.md).
