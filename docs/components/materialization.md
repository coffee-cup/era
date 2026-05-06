# Materialization Component

`era-materialization` is the bridge between content-addressed trees and ordinary files on disk. It turns a stored tree into a working directory, turns a working directory into stored blob/tree objects, compares saved state with current state, and emits filesystem change hints.

This component owns working-directory filesystem behavior. Higher layers should ask it to capture, scan, compare, materialize, or watch paths instead of walking files directly.

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-materialization                          │
├──────────────────────────────────────────────┤
│ Materializer capability                      │
│  - capture working directory                 │
│  - scan working directory                    │
│  - compare with stored tree                  │
│  - materialize stored tree                   │
│  - watch for filesystem hints                │
├──────────────────────────────────────────────┤
│ Filesystem materializer                      │
│  - copy-based local implementation           │
│  - configurable directory exclusions         │
│  - symlink policy                            │
│  - per-materializer hash cache               │
│  - filesystem watcher adapter                │
└──────────────────────────────────────────────┘
```

## Capture flow

```text
working directory
   │
   ▼
walk included paths
   │
   ├─ skip excluded directories and `.era` workspace pointer files
   ├─ apply symlink policy
   └─ reuse cached file hashes when fingerprints match
   │
   ▼
hash changed file bytes
   │
   ▼
store new blobs through object store
   │
   ▼
build trees bottom-up
   │
   ▼
store new trees through object store
   │
   ▼
return root tree ID + capture stats + issues
```

## Restore flow

```text
stored root tree ID
   │
   ▼
load trees and blobs from object store
   │
   ▼
compare target tree with working directory
   │
   ├─ write missing or changed files
   ├─ create needed directories
   ├─ remove tracked paths absent from target
   └─ preserve excluded local state
   │
   ▼
working directory matches target tree
```

## Watch flow

```text
filesystem event
   │
   ▼
filter excluded paths
   │
   ▼
emit relative path hint
   │
   ▼
higher layer invalidates affected cache entries
   │
   ▼
higher layer debounces and reconciles full tree
```

Filesystem watcher events are hints, not proof. The repository and CLI flows reconcile by scanning the working directory before creating snapshots.

## Responsibilities

- Capture a working directory into blob and tree objects.
- Scan a working directory without storing new objects when callers only need comparison.
- Compare a working directory with a stored tree and report path-level changes.
- Materialize a stored tree into a working directory.
- Watch a working directory and emit filtered change hints.
- Maintain hash-cache state scoped to one materializer instance and workspace.

## Boundaries

Materialization does not:

- Create snapshot objects.
- Advance refs or workspace cursors.
- Decide whether an automatic snapshot should be skipped.
- Own shared repository metadata.
- Interpret labels, authors, or provenance beyond data passed through by callers.

Those policies belong to the repository and CLI layers.

## Component relationships

```text
┌───────────────────────┐
│ era-repository        │
│ asks for tree state   │
└───────────┬───────────┘
            │ Materializer capability
            ▼
┌───────────────────────┐       ┌───────────────────────┐
│ era-materialization   │◄─────►│ working directory     │
│ filesystem bridge     │       │ ordinary files        │
└───────────┬───────────┘       └───────────────────────┘
            │ stores/loads blobs and trees
            ▼
┌───────────────────────┐
│ era-object-store      │
└───────────────────────┘
```

## v0 constraints

- The current implementation is copy-based and local-filesystem-backed.
- Hash caching is in-memory and scoped to one materializer instance.
- Default tracking behavior is implemented as exact directory exclusions, preservation of `.era` metadata/pointer entries, plus symlink policy.
- Symlinks are not followed.
- Watchers are best-effort hints and must be paired with reconciliation.

## Future seams

Hardlink, reflink, and FUSE materializers should plug into the same capability boundary. Persistent hash caches should remain workspace-scoped rather than becoming shared object-store state.
