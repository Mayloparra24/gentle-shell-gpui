# ADR-0003: Behavior-preserving refactors

- Status: accepted
- Date: 2026-11-09

## Context

The application already offers broad behavior on several platforms. The architectural reorganization must not change interactions, persistence, protocol, shortcuts or observable presentation.

## Decision

Run the migration in small stages with no functional changes. Before modifying logic that is not sufficiently covered, add a characterization test that captures the relevant behavior. Each stage must run the applicable checks and a smoke test of the modified path.

Do not add tests for wiring, field copies, trivial delegation or source text. Tests cover rules, limits, transitions, precedence and observable errors.

## Consequences

- Changes are smaller and easier to review.
- An extraction may add tests before moving code.
- Functional improvements are kept separate from architectural refactors.
- A stage is not considered verified if the environment prevents running its checks; that limitation is reported explicitly.
