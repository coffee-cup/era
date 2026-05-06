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
│  - capture from dirty-path hints             │
│  - scan working directory                    │
│  - compare with stored tree                  │
│  - materialize stored tree                   │
│  - watch for filesystem hints                │
├──────────────────────────────────────────────┤
│ Filesystem materializer                      │
│  - copy-based local implementation           │
│  - configurable directory exclusions         │
│  - symlink policy                            │
│  - workspace-scoped persistent capture cache │
│  - filesystem watcher adapter                │
└──────────────────────────────────────────────┘
```

## Capture flow

```text
working directory
   │
   ▼
walk included paths, or use dirty-path hints when provided
   │
   ├─ skip excluded directories and `.era` workspace pointer files
   ├─ apply symlink policy
   ├─ reuse cached file hashes when fingerprints match
   └─ reuse cached stored tree entries for unchanged directories
   │
   ▼
hash changed file bytes
   │
   ▼
store new blobs through object store
   │
   ▼
build changed trees bottom-up
   │
   ▼
store new trees through object store and reuse cached stored trees
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
   ├─ skip files whose bytes already match
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
higher layer debounces and requests hinted capture
   │
   ▼
periodic reconciliation still scans the full tree
```

Filesystem watcher events are hints, not proof. Watch-triggered snapshots can use hints to avoid full walks, while periodic reconciliation scans the working directory to catch missed events.

## Responsibilities

- Capture a working directory into blob and tree objects.
- Capture from dirty-path hints when cached stored tree structure is available.
- Scan a working directory without storing new objects when callers only need comparison.
- Compare a working directory with a stored tree and report path-level changes.
- Materialize a stored tree into a working directory.
- Watch a working directory and emit filtered change hints.
- Maintain capture-cache state scoped to one materialized workspace.

The persistent cache is an indexed redb database at the workspace cache path. Full tree operations may bulk-load cache records because they already walk the filesystem, while hinted captures use point lookups/updates and prefix invalidation so one dirty path does not require decoding or rewriting the entire cache. Cache writes skip fsync-level durability because the cache is rebuildable after corruption or loss.

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
- Capture caching is workspace-scoped, redb-backed when a cache path is supplied, and falls back to an in-memory cache otherwise.
- Default tracking behavior is implemented as exact directory exclusions, preservation of `.era` metadata/pointer entries, plus symlink policy.
- Symlinks are not followed.
- Watchers are best-effort hints and must be paired with reconciliation.

## Future seams

Hardlink, reflink, and FUSE materializers should plug into the same capability boundary. Capture caches should remain workspace-scoped rather than becoming shared object-store state; global storage efficiency belongs in object-store packing, indexes, and GC.
