# Era

[![CI](https://github.com/coffee-cup/era/actions/workflows/ci.yml/badge.svg)](https://github.com/coffee-cup/era/actions/workflows/ci.yml)

Era is an experimental Rust workspace for a version control system built for agentic work: cheap snapshots, natural divergence, dense local history, structured provenance, and safer local workflows.

The v0 architecture is documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

> Status: early v0 prototype. Local snapshot, restore, status, branch-ref, and foreground watch flows work, but Era is not yet a production replacement for git.

## Why Era exists

Era explores what version control could look like if it were designed around files changing constantly, often by agents.

Git asks users to stage files, name commits, manage branches, and remember to save work. Era starts from different primitives:

- A **state** is the full project tree at a moment in time.
- A **snapshot** records that state cheaply, usually without requiring a label.
- A **label** is just a convenient name for an important state.
- Divergence is natural: go back to an old state, edit, and history forks from there.
- Provenance is structured so human and agent work can be audited later.

The goal is a system where humans and agents can save aggressively, experiment freely, and recover any previous state without thinking in Git ceremonies.

## Common workflows

Save the current files if anything changed:

```sh
era snap
```

Today this is an explicit command or foreground `era watch`; with FUSE or a custom filesystem, this kind of changed-state capture could happen automatically as files are written.

Mark an important state so it is easy to find later:

```sh
era snap "known good"
era timeline
```

Go back to a previous state by label, full snapshot ID, or unique ID prefix:

```sh
era restore "known good"
era restore abc123
```

Keep dense local history while editing:

```sh
era watch
```

Record agent provenance while watching:

```sh
era watch --workspace agent-1 --agent claude --task fix-parser --model sonnet
```

Use the current v0 named-ref workflow for experiments:

```sh
era branch experiment
era switch experiment
```

`branch` and `switch` expose the current implementation. The intended direction is state/workspace vocabulary: move a workspace to a state, edit files, and let the next snapshot create the new future.

## Direction

Era is moving toward:

- automatic and changed-only snapshots as the normal save path;
- optional labels as bookmarks, not required commit messages;
- workspace cursors instead of user-managed Git-style branches;
- many agent workspaces sharing one content-addressed store;
- queryable provenance for understanding who or what produced each state;
- faster materialization through hardlinks, reflinks, or FUSE.

## Current status

The implemented foundation covers:

- BLAKE3 object IDs and deterministic tree/snapshot formats.
- Async local blob/tree/snapshot object storage.
- Working-directory capture, scan, comparison, and restore.
- Path-aware status with added, modified, deleted, and type-changed paths.
- Local branch refs, branch switching, and snapshot restore.
- Changed-only unlabeled snapshots plus optional labels for important states.
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

`era snap` is a rapid-fire "snapshot if files changed" command. Without a label it creates an unlabeled snapshot only when the working tree differs from the current saved state. `era snap "label"` and `era snap --message "label"` attach a human-facing label to the current state.

`era status` reports whether the working tree matches the current saved snapshot and lists added, modified, deleted, and type-changed paths when it does not.

`era branch` lists or creates branch refs, and `era switch` saves current work before switching refs. These commands expose the current v0 cursor implementation; the architecture is moving toward state/workspace vocabulary. `era restore` saves current work before restoring a snapshot ID, unique ID prefix, or exact snapshot label.

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
