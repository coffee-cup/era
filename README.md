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

The first implemented component is the content-addressed blob layer: BLAKE3 object IDs in `era-core` and an async local blob store in `era-object-store`. Higher-level trees, snapshots, repository metadata, and CLI workflows are tracked in [`docs/PLAN.md`](docs/PLAN.md).

## Prerequisites

This repository uses [`mise`](https://mise.jdx.dev/) to manage tools and run tasks.

```sh
mise install
```

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
