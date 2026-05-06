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

Initialize a project and save the current files when anything changed:

```sh
cd my-project
era init
era snap
```

Today this is an explicit command or foreground `era watch`; with FUSE or a custom filesystem, this kind of changed-state capture could happen automatically as files are written.

Mark an important state so it is easy to find later:

```sh
era snap "known good"
era timeline
```

Restore files from a previous state by label, branch/workspace name, full snapshot ID, or unique ID prefix:

```sh
era restore "known good"
era restore main
era restore abc123
```

`restore` first saves any dirty working tree as an automatic safety snapshot, then materializes the requested tree and moves the current branch/workspace cursor to it. Later snapshots branch from the restored point.

Keep dense local history while editing:

```sh
era watch
```

Create an external workspace that shares the same `.era/objects` tree:

```sh
cd my-project
era workspace add ../runs/agent-1
cd ../runs/agent-1
era status
```

`workspace add` creates the target directory if it is missing, connects it to the source repo, writes a small `.era` pointer file, and materializes the current saved state. Workspaces should live outside the source workspace; nested workspaces are rejected by default.

Start an agent in that workspace and record provenance while it works:

```sh
cd ../runs/agent-1
era watch --agent claude --task fix-parser --model sonnet
```

Adopt an existing non-empty directory as a workspace without overwriting its files:

```sh
cd ../scratch-agent
era snap --repo ../my-project --workspace scratch-agent
```

This lazily connects the current directory to `../my-project`, treats the existing files as dirty relative to the repo's current saved state, and creates a snapshot on the workspace cursor.

Create several independent workspaces from a known base:

```sh
cd my-project
era workspace add ../runs/parser-1 --from main
era workspace add ../runs/parser-2 --from main
era workspace list
```

Each workspace has its own cursor under `.era/refs/workspaces/<id>`, while blobs, trees, and snapshots are shared through the same object store.

Use the current v0 named-ref workflow for experiments in the repository root:

```sh
era branch experiment
era switch experiment
```

`branch` and `switch` expose the repository-root branch-ref implementation. Connected workspaces use workspace refs, so agents can diverge without contending on global `HEAD`.

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
- Local branch refs, workspace refs, branch/workspace switching, and cursor-moving snapshot restore.
- Snapshot-tree timeline rendering over indexed history with cursor/worktree markers and automatic collapse of long auto-snapshot runs.
- `era workspace add` / `era workspace list` for many materialized workspaces sharing one `.era/objects` tree.
- Scoped metadata locks and atomic ref updates so concurrent agents can snapshot different workspaces safely.
- Changed-only unlabeled snapshots plus optional labels for important states.
- Foreground `era watch` auto-snapshots with debounce, dirty-path hinted capture, periodic reconciliation, and structured provenance.
- Workspace-scoped indexed capture caches for one-shot commands and watch sessions.

The foundation also includes a snapshot-agnostic text merge engine with structured conflicts; repository/CLI merge workflows are not wired yet.

Notable future work includes object packing/deltas, garbage collection, snapshot retention policy, workspace fleet supervision, repository-level diff/merge flows, semantic merge strategies, tracking heuristics, provenance indexing/querying, and git interoperability.

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
mise run bench # optional local perf run
mise run bench-large # optional 10k-file perf run
cargo run -p era-cli --bin era -- --help
```

To install the local CLI binary from a checkout:

```sh
cargo install --path crates/cli
```

## CLI usage

Run commands from the repository root or a connected workspace root. Parent-directory repository discovery is future work.

Core repository commands:

```sh
era init
era snap
era snap "manual checkpoint"
era snap --message "manual checkpoint"
era status
era restore "manual checkpoint"
era watch
era watch --once
era timeline
```

Workspace commands:

```sh
era workspace add ../runs/agent-1
era workspace add ../runs/agent-2 --from main
era workspace add . --repo ../project --workspace agent-1
era workspace list
```

Branch-ref commands:

```sh
era branch
era branch experiment
era switch experiment
```

Lazy workspace connection works on commands that need workspace context:

```sh
era snap --repo ../project --workspace agent-1
era status --repo ../project --workspace agent-1
era restore main --repo ../project --workspace agent-1
era watch --repo ../project --workspace agent-1 --agent claude --task fix-parser --model sonnet
era timeline --repo ../project --workspace agent-1
```

`era snap` is a rapid-fire "snapshot if files changed" command. Without a label it creates an unlabeled snapshot only when the working tree differs from the current saved state. `era snap "label"` and `era snap --message "label"` attach a human-facing label to the current state.

`era status` reports whether the working tree matches the current saved snapshot and lists added, modified, deleted, and type-changed paths when it does not.

`era workspace add PATH` is the single command for creating a missing workspace directory, connecting an empty directory, or adopting a non-empty directory as dirty relative to the inferred base snapshot. By default it infers the repository from the current repository/workspace, infers the workspace ID from the target directory name, uses the current saved state as the base, and rejects nested workspaces inside another workspace. Use `--from TARGET` to start from a specific branch, workspace, label, ID, or unique ID prefix.

`era branch` lists or creates branch refs, and `era switch` saves current work before switching refs. These commands expose the current v0 repository-root cursor implementation; connected workspaces use workspace refs. `era restore` saves current work before materializing a snapshot ID, unique ID prefix, branch/workspace name, or exact snapshot label, then moves the active branch/workspace cursor to that snapshot so later snapshots branch from the restored point.

`era timeline` renders the indexed snapshot tree, oldest to newest, with `@` marking the current cursor and `◎` marking saved snapshots whose tree matches the working directory. History that no branch or workspace currently names remains visible, so restoring an old snapshot does not hide the previous future. Long linear runs of unlabeled automatic snapshots are collapsed automatically so watch-heavy histories stay readable.

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
