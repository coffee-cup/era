# Era

Era is a Rust workspace for exploring a version control system built for agentic work: cheap snapshots, instant branches, dense history, and structured provenance.

The v0 architecture is documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Workspace layout

- `crates/core` (`era-core`) — shared domain types and primitives.
- `crates/object-store` (`era-object-store`) — content-addressed storage abstractions and implementations.
- `crates/materialization` (`era-materialization`) — working-directory materialization and filesystem observation.
- `crates/repository` (`era-repository`) — branch, snapshot, history, and policy orchestration.
- `crates/cli` (`era-cli`, binary `era`) — command-line interface over the library APIs.
- `docs/` — architecture and design documentation.

## Current status

The implemented foundation now covers content-addressed objects, working-directory capture/restore, repository snapshots, local branch workflows, path-aware status, foreground auto-snapshot watching, and initial CLI workflows: BLAKE3 object IDs plus deterministic tree/snapshot types in `era-core`, an async local blob/tree/snapshot store in `era-object-store`, a configurable filesystem materializer with an in-memory hash cache in `era-materialization`, repository init/snapshot/status/branch/restore/auto-snapshot APIs in `era-repository`, and a thin `era` CLI over those APIs. CLI workflow expansion is tracked in [`docs/PLAN.md`](docs/PLAN.md).

## Prerequisites

This repository uses [`mise`](https://mise.jdx.dev/) to manage tools and run tasks.

```sh
mise install
```

## CLI usage

Run commands from the working-directory root:

```sh
era init
era snap
era snap "manual checkpoint"
era snap --message "manual checkpoint"
era status
era branch
era branch experiment
era switch experiment
era restore "manual checkpoint"
era watch
era watch --once
era watch --workspace agent-1 --agent claude --task fix-parser --model sonnet
era timeline
```

`era snap` captures the current state and attaches a human-facing label. If no label is supplied, Era uses the current local timestamp in the form `Jan 1, 2024 11:11:11`. `era status` reports whether the working tree matches the current saved snapshot and lists added, modified, deleted, and type-changed paths when it does not. `era branch` lists or creates branches, `era switch` saves current work before switching branches, and `era restore` saves current work before restoring a snapshot ID, unique ID prefix, or exact snapshot label. `era watch` runs in the foreground, treats filesystem events as hints, debounces edits, periodically reconciles the full tree, and creates unlabeled automatic snapshots only when the tree changed. Watch snapshots record structured provenance such as trigger, workspace, agent, task, and model; the timeline renders their timestamp as a synthetic title instead of storing a label. Use `--verbose` on any command for full object IDs, root tree IDs, timestamps, paths, provenance attributes, and capture/materialization/cache stats:

```sh
era --verbose status
era timeline --verbose
```

When running from the workspace without installing the binary, use `cargo run -p era-cli --bin era -- <command>`.

## Tracing

Runtime tracing is off by default. Enable it with `ERA_LOG` or `RUST_LOG`:

```sh
ERA_LOG=debug era timeline
ERA_LOG=era_object_store=trace cargo test -p era-object-store -- --nocapture
```

Tracing output is written to stderr. Human-facing command output uses terminal colors when supported and automatically strips ANSI escapes when output is redirected or captured.

## Development

Run commands from the repository root through `mise`:

```sh
mise run fmt      # format all Rust crates
mise run check    # type-check all Rust crates and targets
mise run clippy   # lint all Rust crates and targets
mise run test     # run all tests
mise run ci       # run the full local validation suite
```

## Documentation

Before making architectural or behavioral changes, read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and any related files in `docs/`. Keep docs updated when code changes alter architecture, workflows, commands, or operational assumptions.
