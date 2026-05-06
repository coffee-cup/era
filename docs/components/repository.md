# Repository Component

`era-repository` is the orchestration layer. It owns local repository state, refs/cursors, snapshot creation policy, status, timeline traversal, context switching, and restore behavior.

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
│ Refs and workspace cursors                   │
│  - current cursor                            │
│  - named ref heads                           │
│  - current branch implementation             │
├──────────────────────────────────────────────┤
│ Snapshot orchestration                       │
│  - labeled snapshots                         │
│  - changed-only unlabeled snapshots          │
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
   ├─ determine current workspace cursor / parent snapshot
   ├─ collect snapshot metadata and provenance
   ▼
ask materializer to capture current tree
   │
   ▼
root tree ID
   │
   ▼
compare with cursor tip when request is changed-only
   │
   ├─ unchanged changed-only request ──► skip
   ▼
create snapshot object
   │
   ▼
store snapshot through object store
   │
   ▼
advance current cursor/ref
```

Labeled snapshots use the same underlying object type as unlabeled snapshots. A label is optional metadata that makes a state easier to find; it is not the act that makes history exist. Unlabeled snapshot requests are changed-only so agents can call them repeatedly without creating duplicate states.

## Status flow

```text
current cursor/ref
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

The current file state is derived from the working directory by scanning it. The cursor is still needed to identify the parent snapshot for future history and to disambiguate identical file trees that appear in different places in the graph.

## Switch and restore flow

```text
switch or restore request
   │
   ▼
safety snapshot current work if changed
   │
   ▼
resolve target ref or snapshot
   │
   ▼
ask materializer to materialize target tree
   │
   ├─ switch: update current cursor/ref context
   └─ restore: leave current ref unchanged
   │
   ▼
working directory reflects requested target
```

The current implementation exposes branch refs and `switch` as the named-line mechanism. Architecturally, branch refs are one implementation of workspace cursors; future user-facing commands can prefer state/workspace vocabulary without changing the snapshot graph model.

## Responsibilities

- Initialize and open local repositories.
- Manage current cursor/ref state and named references.
- Create snapshot objects with the correct parents and metadata.
- Decide when changed-only snapshot requests should create or skip snapshots.
- Save unsnapped work before context switches and restores.
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
│ refs/cursors, snapshot policy, timeline, restore      │
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
- Timeline traversal is first-parent history from the current ref.
- Branch refs are the current persisted cursor mechanism.
- Merge, garbage collection, sync, and multi-workspace supervision are future work.
- Workspace identity is currently captured as provenance for watch snapshots, not as a full workspace registry.

## Future seams

Repository is where richer policy belongs: merge, diff, tracking heuristics, provenance indexes, workspace registration, and sync coordination. Those features should preserve the boundary that shared repository state is objects, refs, graph metadata, and indexes, while watcher/debounce/hash-cache state remains workspace-scoped.
