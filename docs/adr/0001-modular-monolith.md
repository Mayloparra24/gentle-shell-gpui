# ADR-0001: Modular monolith by feature

- Status: accepted
- Date: 2026-11-09

## Context

The application is a GPUI desktop binary with tightly integrated features: windows, sessions, files, terminal, settings and updates. Some code has grown into large modules, but the product does not need independent deployments or distributed communication.

## Decision

Keep a modular monolith and organize it by feature. Each feature has a small public API. Internally, it may split state, effects and view when those responsibilities are large enough.

`pi-rpc` stays a separate crate because it represents a real boundary: a child process and a protocol, with no GPUI dependency.

## Consequences

- Cross-feature changes stay simple and are compiled together.
- Boundaries are expressed through module and crate privacy where that adds value.
- No crate is created per screen.
- Large modules are split by cohesive responsibilities, not only by line count.
- The architecture can evolve without a one-shot full migration.
