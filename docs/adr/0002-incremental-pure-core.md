# ADR-0002: Incremental pure core

- Status: accepted
- Date: 2026-11-09

## Context

The application contains parsers, models and transformations that can run without GPUI or IO. It also has a lot of view state that legitimately uses framework types. Creating an empty `core` crate, or moving GPUI-coupled types, would produce artificial adapters.

## Decision

Extract a `core` crate only when there is pure logic with a stable boundary. Until then, pure logic may live in internal modules of `app` and must avoid unnecessary dependencies on GPUI, processes and the filesystem.

Natural candidates are the transcript models, activity, tasks, usage parsers and profile normalization. Extraction is done one complete behavior at a time, together with its tests.

## Consequences

- `core` does not become a generic collection of utilities.
- Traits are not introduced only to satisfy an architectural shape.
- Pure logic is easy to test and reuse once the boundary is real.
- Some features will keep combining state and view when separating them does not improve maintainability.
