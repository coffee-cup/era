# Implementation Plan

This plan tracks the first non-bootstrap slices for Era. The goal is to build one solid component at a time, with focused tests and enough documentation that another agent can continue from any checkpoint.

## Guiding principles

- Keep each slice small and independently testable.
- Preserve the architecture seams from `docs/ARCHITECTURE.md`.
- Prefer deterministic behavior over background magic until the core is proven.
- Add tests with every behavior change.
- Keep performance in mind from the first primitive: avoid redundant writes, use stable hashes, and keep layouts scalable.
- Use `tracing` for operational and performance visibility. Runtime tracing should be off by default and toggleable through `ERA_LOG` or `RUST_LOG`.

## Implemented slices

### Object identity, blob storage, and tree storage

Status: implemented.

- Package names use the `era-*` prefix:
  - `era-core`
  - `era-object-store`
  - `era-materialization`
  - `era-repository`
  - `era-cli` with binary name `era`
- `era-core` owns BLAKE3-based `ObjectId` parsing, formatting, validation, content hashing, and deterministic tree/snapshot domain types.
- Tree entries use UTF-8 single path-segment names, support emoji and non-English characters, preserve exact bytes, and intentionally do not normalize Unicode.
- `era-object-store` owns one async `ObjectStore` trait plus a local content-addressed implementation:
  - `put_blob(bytes) -> ObjectId`
  - `get_blob(id) -> bytes`
  - `put_tree(tree) -> ObjectId`
  - `get_tree(id) -> Tree`
  - `put_snapshot(snapshot) -> ObjectId`
  - `get_snapshot(id) -> Snapshot`
  - `contains(kind, id) -> bool`
  - deduplication for identical content
  - sharded filesystem layout
  - integrity verification on reads
  - canonical tree and snapshot validation
  - corruption detection instead of silent overwrite

### Materialization scan and restore

Status: implemented.

- `era-materialization` owns an async `Materializer` trait and a copy-based `FilesystemMaterializer`.
- `FilesystemMaterializer` captures a working directory into blob and tree objects and returns the root tree ID, capture stats, and non-fatal issues.
- It can scan a working directory without storing objects, returning the root tree ID that status uses for saved-vs-current comparison.
- It can compare a working directory with a stored tree and return sorted path-level added, modified, deleted, and type-changed entries.
- It can materialize a stored tree back into the working directory for branch switching and restore, preserving excluded directories such as `.era` and generated caches when they are outside the target tree.
- Capture options provide configurable exact directory-name exclusions. Defaults skip `.era`, `.git`, `target`, `node_modules`, `.next`, `dist`, `build`, `.cache`, and `__pycache__`.
- Symlinks are not followed. The default policy skips them and records issues; callers can configure symlinks to return an error.
- The scan handles nested directories, empty directories, empty files, deletes between captures, emoji and non-English UTF-8 paths, and deterministic tree output.
- Filesystem watching and hash caching remain future materialization work.

Important boundary rule: filesystem calls belong inside storage/materialization implementations. Repository and CLI code should depend on async capabilities, not direct `std::fs` access to the working tree. Instrument I/O-heavy paths with `tracing` spans/events so agents can debug behavior and performance without adding ad-hoc prints.

### Repository init and manual snapshot

Status: implemented.

- `era-repository` creates and opens local repositories rooted at a working directory.
- Repository metadata uses:
  - `.era/HEAD`
  - `.era/refs/heads/main`
  - `.era/objects/{blobs,trees,snapshots}`
- Init captures the working directory, writes an initial snapshot with structured provenance, and points `main` at it.
- Manual snapshots capture the current working directory, store a snapshot with the current branch tip as parent, and advance the branch ref.
- Working-tree status compares the current tree with the current branch snapshot and reports both root tree IDs and path-level changes.
- Branch operations can list branches, create branches at the current saved state, and switch branches by materializing the target branch snapshot.
- Restore resolves a full snapshot ID, unique ID prefix, or exact snapshot message from the current timeline and materializes that snapshot without moving the current branch ref.
- Switch and restore save unsnapped work first so context changes do not lose data.
- Timelines walk first-parent history from the current branch newest-to-oldest.
- Snapshot canonical bytes are locked with small golden fixtures under `crates/core/tests/fixtures/`.

## Next slices

### 1. Thin CLI

Status: local workflows implemented.

Implemented commands expose the repository flows needed to play with the system from the current directory:

```sh
era init
era snap
era snap "label"
era snap --message "..."
era status
era branch
era branch NAME
era switch NAME
era restore SNAPSHOT_OR_LABEL
era timeline
```

Commands use clean default output with adaptive terminal coloring and a global `--verbose` flag for full object IDs, root tree IDs, timestamps, repository paths, and capture/materialization stats. `era snap` is the single user-facing "remember this state" command: it captures the current tree and attaches an optional label, defaulting to the current local timestamp formatted like `Jan 1, 2024 11:11:11`.

`era status` compares the working tree to the current saved snapshot and reports either `no changes` or changed paths marked as added (`A`), modified (`M`), deleted (`D`), or type-changed (`T`). `era branch` lists or creates branches at the current saved state. `era switch` saves unsnapped work before switching branches and materializing the target branch. `era restore` saves unsnapped work before restoring a snapshot ID, unique ID prefix, or exact snapshot label into the working tree without moving the current branch pointer.

The CLI is covered by integration tests that run the compiled `era` binary through init/snapshot/status/branch/switch/restore/timeline flows in temporary repositories and verify user-facing errors.

### 2. Agent-facing eval flows

Add markdown evals/scripts that another Pi agent can run against a temp project.

Focus:

- init/snapshot/timeline flow
- branch/switch flow
- restore-by-label flow
- corruption/integrity smoke test
- simple performance smoke test on many unchanged files

## Validation baseline

Use repository-root mise tasks:

```sh
mise run fmt
mise run check
mise run clippy
mise run test
mise run ci
```
