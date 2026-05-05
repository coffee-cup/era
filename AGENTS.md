# Agent Instructions

## Project context

This project is a Rust workspace for a version control system designed around cheap snapshots, instant branches, dense history, and agent-friendly provenance.

Before making architectural or behavioral changes:

1. Read `docs/ARCHITECTURE.md`.
2. Read any related files in `docs/` that cover the area you are changing.
3. Update `docs/ARCHITECTURE.md` and related docs when code changes alter the architecture, workflows, or operational assumptions.

## Workspace layout

- `crates/core` — shared domain types and primitives.
- `crates/object-store` — content-addressed storage abstractions and implementations.
- `crates/materialization` — working-directory materialization and filesystem observation.
- `crates/repository` — branch, snapshot, history, and policy orchestration.
- `crates/cli` — command-line interface over the library APIs.
- `docs/` — architecture and design documentation.

## Commands

Use `mise` tasks from the repository root instead of invoking toolchain commands directly:

- `mise run fmt` — format all Rust crates.
- `mise run check` — type-check all Rust crates and targets.
- `mise run clippy` — lint all Rust crates and targets.
- `mise run test` — run all tests.
- `mise run ci` — run the full local validation suite.

If Rust is not installed, run `mise install` first.

## Expectations

- Keep changes small and aligned with the v0 architecture.
- Prefer clear component boundaries over premature abstraction.
- Add or update tests for behavior changes.
- Keep documentation current when public commands, workspace structure, or architectural responsibilities change.
- Don't let technical details get in the way of user experience. Always consider how the user or agent will interact with the system.

## Agent and workspace design notes

- Treat a workspace as the per-directory execution context for a human or agent: materialized path, watcher/debounce state, hash cache, current checkout context, and workspace ID.
- Keep shared repository state limited to objects, refs, snapshot graph, and future indexes. Do not put per-workspace watcher/cache state in the shared object store.
- Automatic snapshots should remain unlabeled; use structured provenance attributes such as `trigger`, `workspace`, `agent`, `task`, and `model` for auditability.
- Preserve the path toward thousands of agent workspaces by keeping materialization APIs path-oriented and avoiding assumptions that one repository has exactly one working directory forever.
