# Repository Component

`era-repository` is the orchestration layer. It owns local repository state, branch refs, snapshot creation policy, status, timeline traversal, branch switching, and restore behavior.

Repository code coordinates the object store and materializer, but it should not take over their responsibilities. It stores immutable objects through the object store and asks the materializer to inspect or change the working directory.

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-repository                               │
├──────────────────────────────────────────────┤
│ Local repository lifecycle                   │
│  - initialize repository metadata            │
│  - open existing repository                  │
├──────────────────────────────────────────────┤
│ Refs and branches                            │
│  - current branch                            │
│  - branch heads                              │
│  - branch creation and switching             │
├──────────────────────────────────────────────┤
│ Snapshot orchestration                       │
│  - manual snapshots                          │
│  - changed-only automatic snapshots          │
│  - safety snapshots before context changes   │
│  - structured provenance                     │
├──────────────────────────────────────────────┤
│ Read workflows                               │
│  - working-tree status                       │
│  - first-parent timeline                     │
│  - snapshot target resolution                │
└──────────────────────────────────────────────┘
```

## Snapshot flow

```text
repository operation
   │
   ├─ determine current branch and parent snapshot
   ├─ collect snapshot metadata and provenance
   ▼
ask materializer to capture current tree
   │
   ▼
root tree ID
   │
   ▼
create snapshot object
   │
   ▼
store snapshot through object store
   │
   ▼
advance current branch ref
```

Automatic snapshots use the same flow, but first compare the captured root tree with the current branch tip. If the tree did not change, no new snapshot is written.

## Status flow

```text
current branch ref
   │
   ▼
current snapshot root tree
   │
   ▼
ask materializer to compare working directory
   │
   ▼
root tree comparison + path-level changes
   │
   ▼
return structured status to caller
```

## Switch and restore flow

```text
switch or restore request
   │
   ▼
safety snapshot current work if changed
   │
   ▼
resolve target branch or snapshot
   │
   ▼
ask materializer to materialize target tree
   │
   ├─ switch: update current branch context
   └─ restore: leave branch ref unchanged
   │
   ▼
working directory reflects requested target
```

## Responsibilities

- Initialize and open local repositories.
- Manage current branch state and branch references.
- Create snapshot objects with the correct parents and metadata.
- Decide when automatic snapshot requests should create or skip snapshots.
- Save unsnapped work before branch switches and restores.
- Resolve snapshot targets by full ID, unique prefix, or exact label in the current timeline.
- Provide structured results for CLI and library clients.

## Boundaries

Repository does not:

- Hash file contents directly.
- Walk, watch, or rewrite working-directory files directly.
- Implement object storage mechanics.
- Own per-workspace watcher loops or hash caches.
- Render terminal output.

Those responsibilities belong to materialization, object-store, workspace-level clients, and CLI code.

## Component relationships

```text
                    ┌───────────────────────┐
                    │ era-cli / clients     │
                    │ user intent           │
                    └───────────┬───────────┘
                                │ Repository API
                                ▼
┌───────────────────────────────────────────────────────┐
│ era-repository                                        │
│ refs, branches, snapshot policy, timeline, restore    │
└───────────────┬───────────────────────┬───────────────┘
                │                       │
                │ Materializer          │ ObjectStore
                ▼                       ▼
┌───────────────────────┐     ┌───────────────────────┐
│ era-materialization   │     │ era-object-store      │
│ working tree state    │     │ immutable objects     │
└───────────────────────┘     └───────────────────────┘
```

## v0 constraints

- Repository state is local-only.
- The current CLI opens repositories from the working-directory root.
- Timeline traversal is first-parent history.
- Merge, garbage collection, sync, and multi-workspace supervision are future work.
- Workspace identity is currently captured as provenance for watch snapshots, not as a full workspace registry.

## Future seams

Repository is where richer policy belongs: merge, diff, tracking heuristics, provenance indexes, workspace registration, and sync coordination. Those features should preserve the boundary that shared repository state is objects, refs, graph metadata, and indexes, while watcher/debounce/hash-cache state remains workspace-scoped.
