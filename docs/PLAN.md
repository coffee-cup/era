# Implementation Plan

This plan tracks the first non-bootstrap slices for Era. The goal is to build one solid component at a time, with focused tests and enough documentation that another agent can continue from any checkpoint.

## Guiding principles

- Keep each slice small and independently testable.
- Preserve the architecture seams from `docs/ARCHITECTURE.md`.
- Prefer deterministic behavior over background magic until the core is proven.
- Add tests with every behavior change.
- Keep performance in mind from the first primitive: avoid redundant writes, use stable hashes, and keep layouts scalable.
- Use `tracing` for operational and performance visibility. Runtime tracing should be off by default and toggleable through `ERA_LOG` or `RUST_LOG`.

## Current slice: object identity and blob storage

Status: implemented in the first non-bootstrap pass.

- Package names use the `era-*` prefix:
  - `era-core`
  - `era-object-store`
  - `era-materialization`
  - `era-repository`
  - `era-cli` with binary name `era`
- `era-core` owns BLAKE3-based `ObjectId` parsing, formatting, validation, and content hashing.
- `era-object-store` owns an async blob-store trait plus a local content-addressed implementation:
  - `put_blob(bytes) -> ObjectId`
  - `get_blob(id) -> bytes`
  - `contains_blob(id) -> bool`
  - deduplication for identical content
  - sharded filesystem layout
  - integrity verification on reads
  - corruption detection instead of silent overwrite

Important boundary rule: filesystem calls belong inside storage/materialization implementations. Repository and CLI code should depend on async capabilities, not direct `std::fs` access to the working tree. Instrument I/O-heavy paths with `tracing` spans/events so agents can debug behavior and performance without adding ad-hoc prints.

## Next slices

### 1. Tree object model

Add deterministic tree data structures in `era-core` and storage support in `era-object-store`.

Focus:

- `Tree`, `TreeEntry`, and entry kind/type metadata.
- Stable entry sorting.
- Deterministic serialization.
- Tree IDs derived from serialized tree content.
- Tests proving hash stability independent of construction order.

### 2. Materialization scan

Teach `era-materialization` to scan a working directory into blob and tree objects. Start this slice by defining a narrow async materializer trait before adding the copy-based implementation.

Focus:

- Keep the trait capability-oriented: materialize a tree/snapshot, report current tree state, and surface changes.
- Exclude `.era/`, `.git/`, `target/`, and common transient directories.
- Preserve relative paths safely.
- Handle added, modified, deleted, empty, and nested files.
- Return useful capture stats.
- Add a simple hash cache after the scan path is correct.

### 3. Repository init and manual snapshot

Teach `era-repository` to create repository metadata and capture explicit snapshots.

Focus:

- `.era/` layout.
- `HEAD` and `refs/heads/main`.
- snapshot objects with parent pointers and provenance.
- initial snapshot and subsequent manual snapshots.
- timeline walking from the current branch.

### 4. Thin CLI

Expose only the workflows needed to play with the system.

Initial commands:

```sh
era init
era snap --message "..."
era timeline
```

Then add:

```sh
era mark "label"
era branch NAME
era switch NAME
era restore SNAPSHOT_OR_LABEL
```

### 5. Agent-facing eval flows

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
