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
│  - named branch refs                         │
│  - workspace refs and registry records       │
│  - scoped lock files for mutable metadata    │
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
│  - indexed snapshot graph                    │
│  - snapshot target resolution                │
├──────────────────────────────────────────────┤
│ Workspace workflows                          │
│  - add or adopt workspace paths              │
│  - write external workspace pointer files    │
│  - list workspace cursors                    │
└──────────────────────────────────────────────┘
```

## Snapshot flow

```text
repository operation
   │
   ├─ determine current workspace cursor / parent snapshot
   ├─ collect snapshot metadata and provenance
   ▼
ask materializer to capture current tree without holding a ref lock
   │
   ▼
root tree ID
   │
   ▼
lock only the current cursor ref and re-read its tip
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
record snapshot ID in repository graph index
   │
   ▼
atomically advance current cursor/ref
   │
   ▼
release cursor lock
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
switch request
   │
   ├─ safety snapshot current work if changed
   ├─ resolve target branch
   ├─ materialize target tree
   └─ update current cursor/ref context

restore request
   │
   ├─ resolve target ref or snapshot before saving dirty work
   ├─ safety snapshot current work if changed
   ├─ materialize target tree
   └─ move active branch/workspace cursor to target snapshot
   │
   ▼
working directory reflects requested target and future snapshots branch from it
```

The current implementation exposes branch refs and `switch` as the repository-root named-line mechanism. External workspaces have their own refs under `.era/refs/workspaces/<id>`; switching inside such a workspace materializes the branch target and advances the workspace ref instead of changing global `HEAD`. Restore keeps the active cursor identity (for example `main` or `agent-1`) but moves that cursor to the restored snapshot.

## Workspace add flow

```text
era workspace add PATH
   │
   ├─ infer repo, workspace ID, and base snapshot when omitted
   ├─ reject nested workspaces inside another workspace
   ├─ create missing target directory if needed
   ├─ safety-snapshot dirty source workspace when using inferred base
   ├─ lock `.era/locks/workspaces/<id>.lock`
   ├─ create `.era/refs/workspaces/<id>` when missing
   ├─ write `.era/workspaces/<id>/path`
   ├─ write `<workspace>/.era` pointer file
   └─ materialize base tree only for missing or empty directories
```

A non-empty target directory is adopted as dirty relative to the base snapshot; Era does not overwrite it during add.

## Responsibilities

- Initialize and open local repositories or connected workspaces.
- Manage current cursor/ref state, workspace cursors, and named references.
- Create snapshot objects with the correct parents and metadata.
- Decide when changed-only snapshot requests should create or skip snapshots.
- Save unsnapped work before context switches and restores.
- Resolve snapshot targets by full ID, branch/workspace ref name, unique prefix, or exact label in indexed history.
- Maintain a lightweight snapshot graph index under `.era/index/snapshots` and rebuild it from local snapshot objects when missing.
- Provide first-parent timelines for cursor-focused history and indexed snapshot graphs for tree renderers.
- Provide structured results for CLI and library clients.
- Serialize mutable ref, workspace registry, and index rebuild updates with scoped locks while leaving object writes lock-free.

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
- The CLI can open from a repository root, a connected workspace pointer, or lazily connect the current directory with `--repo`.
- First-parent timeline traversal remains available from the current cursor ref.
- Snapshot graph traversal loads snapshots from the repository-owned snapshot index, including unnamed futures left behind by restore.
- The v0 snapshot index is a filesystem-backed ID index; richer compact graph/provenance indexes can replace it for very large histories without changing snapshot objects.
- Branch refs remain the repository-root named-line mechanism; workspace refs are the per-directory agent mechanism.
- Repository-level merge orchestration, garbage collection, sync, and fleet supervision are future work. The snapshot-agnostic `era-merge` file engine exists for future merge orchestration.
- Workspace registry records are lightweight path metadata; watcher loops and hash caches remain outside shared repo state.

## Future seams

Repository is where richer policy belongs: merge-base selection, tree merge planning, diff, tracking heuristics, provenance indexes, workspace fleet supervision, and sync coordination. File-level merge logic belongs behind the snapshot-agnostic `era-merge` adapter boundary so future snapshot storage changes do not rewrite merge strategies. Those features should preserve the boundary that shared repository state is objects, refs, graph metadata, workspace registry records, and indexes, while watcher/debounce/hash-cache state remains workspace-scoped.
