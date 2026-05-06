# Era

[![CI](https://github.com/coffee-cup/era/actions/workflows/ci.yml/badge.svg)](https://github.com/coffee-cup/era/actions/workflows/ci.yml)

Era is an experimental Rust workspace for a version control system built for agentic work: cheap snapshots, instant branches, dense local history, structured provenance, and safer local workflows.

The v0 architecture is documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). The implementation plan is tracked in [`docs/PLAN.md`](docs/PLAN.md).

> Status: early v0 prototype. The local snapshot, branch, restore, status, and foreground watch flows work, but Era is not yet a production replacement for git.

## Workspace layout

- `crates/core` (`era-core`) — shared domain types and primitives.
- `crates/object-store` (`era-object-store`) — content-addressed storage abstractions and implementations.
- `crates/materialization` (`era-materialization`) — working-directory materialization, filesystem observation, and in-memory hash caching.
- `crates/repository` (`era-repository`) — branch, snapshot, history, provenance, and policy orchestration.
- `crates/cli` (`era-cli`, binary `era`) — command-line interface over the library APIs.
- `docs/` — architecture and design documentation.

## Current status

The implemented foundation covers:

- BLAKE3 object IDs and deterministic tree/snapshot formats.
- Async local blob/tree/snapshot object storage.
- Working-directory capture, scan, comparison, and restore.
- Path-aware status with added, modified, deleted, and type-changed paths.
- Local branches, branch switching, and snapshot restore.
- Manual snapshots with optional labels.
- Foreground `era watch` auto-snapshots with debounce, periodic reconciliation, and structured provenance.
- A per-materializer in-memory hash cache for long-running watch sessions.

Notable future work includes persistent workspace hash caches, multi-workspace supervision, richer diff/merge flows, tracking heuristics, provenance indexing/querying, and git interoperability.

## Prerequisites

This repository uses [`mise`](https://mise.jdx.dev/) to manage tools and run tasks.

```sh
mise install
```

## Quick start

```sh
git clone https://github.com/coffee-cup/era.git
cd era
mise install
mise run ci
cargo run -p era-cli --bin era -- --help
```

To install the local CLI binary from a checkout:

```sh
cargo install --path crates/cli
```

## CLI usage

Run commands from the working-directory root. Parent-directory repository discovery is future work.

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

`era snap` captures the current state and attaches a human-facing label. If no label is supplied, Era uses the current local timestamp in the form `Jan 1, 2024 11:11:11`.

`era status` reports whether the working tree matches the current saved snapshot and lists added, modified, deleted, and type-changed paths when it does not.

`era branch` lists or creates branches. `era switch` saves current work before switching branches. `era restore` saves current work before restoring a snapshot ID, unique ID prefix, or exact snapshot label.

`era watch` runs in the foreground, treats filesystem events as hints, debounces edits, periodically reconciles the full tree, and creates unlabeled automatic snapshots only when the tree changed. Watch snapshots record structured provenance such as trigger, workspace, agent, task, and model; timeline output renders their timestamp as a synthetic title instead of storing a label.

Use `--verbose` on any command for full object IDs, root tree IDs, timestamps, paths, provenance attributes, and capture/materialization/cache stats:

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

Before making architectural or behavioral changes, read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and any related files in `docs/`. Keep docs updated when code changes alter architecture, workflows, commands, or operational assumptions.

## License

Licensed under the [MIT license](LICENSE).
