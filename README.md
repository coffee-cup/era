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

The implemented foundation now covers content-addressed objects, working-directory capture, and repository snapshots: BLAKE3 object IDs plus deterministic tree/snapshot types in `era-core`, an async local blob/tree/snapshot store in `era-object-store`, a configurable filesystem scanner in `era-materialization`, and repository init/manual snapshot/timeline APIs in `era-repository`. CLI workflows are tracked in [`docs/PLAN.md`](docs/PLAN.md).

## Prerequisites

This repository uses [`mise`](https://mise.jdx.dev/) to manage tools and run tasks.

```sh
mise install
```

## Tracing

Runtime tracing is off by default. Enable it with `ERA_LOG` or `RUST_LOG`:

```sh
ERA_LOG=debug cargo run -p era-cli --bin era
ERA_LOG=era_object_store=trace cargo test -p era-object-store -- --nocapture
```

Tracing output is written to stderr so command stdout stays script-friendly.

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
