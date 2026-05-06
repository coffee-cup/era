# CLI Component

`era-cli` is the user-facing command-line surface over the repository library APIs. It translates command intent into repository operations, formats results for humans, and owns the current foreground watch loop.

The CLI should stay thin. Durable behavior belongs in the repository, materialization, object store, and core components so agents, editor integrations, and other tools can use Era without shelling out.

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-cli                                      │
├──────────────────────────────────────────────┤
│ Command parsing                              │
│  - init                                      │
│  - snap                                      │
│  - status                                    │
│  - branch                                    │
│  - switch                                    │
│  - restore                                   │
│  - watch                                     │
│  - timeline snapshot-tree view               │
│  - workspace add/list                        │
├──────────────────────────────────────────────┤
│ Output rendering                             │
│  - concise default output                    │
│  - verbose diagnostics                       │
│  - adaptive color                            │
├──────────────────────────────────────────────┤
│ Runtime wiring                               │
│  - current-directory repository/workspace access │
│  - filesystem materializer construction      │
│  - tracing setup                             │
│  - foreground watch loop                     │
└──────────────────────────────────────────────┘
```

The current command surface still includes `branch` and `switch` because repository-root named lines are branch refs. That vocabulary is not sacred: future CLI work can introduce state-oriented commands such as `go`, `mark`, or `resume` while keeping the repository APIs underneath. External agent workspaces use `workspace add` and per-workspace refs instead of global `HEAD`.

## Command flow

```text
user command
   │
   ▼
parse arguments and global flags
   │
   ▼
open or initialize repository/workspace in current directory
   │
   ▼
construct filesystem materializer with workspace capture cache
   │
   ▼
call repository API
   │
   ▼
format structured result for terminal output
```

## Snapshot command flow

```text
era snap [optional label]
   │
   ▼
capture current tree using the workspace cache
   │
   ├─ no label and unchanged ──► print "No changes"
   ├─ no label and changed ────► create unlabeled snapshot
   └─ label supplied ──────────► create labeled snapshot for current state
   │
   ▼
print snapshot result
```

`era snap` without a label is intentionally rapid-fire and changed-only. Agents can call it after each tool action without worrying about duplicate snapshots. `era snap "label"` and `era snap --message "..."` are convenience forms for making a state easy to find by name. `snap`, `status`, `restore`, `watch`, and `timeline` accept `--repo REPO --workspace ID` to lazily connect the current directory to a shared repository before running; when `--workspace` is omitted in that mode, the directory basename is used.

## Restore flow

```text
era restore TARGET
   │
   ▼
resolve TARGET to a snapshot
   │
   ├─ save dirty current files as an automatic safety snapshot
   ├─ materialize TARGET into the working directory
   └─ move the active branch/workspace cursor to TARGET
   │
   ▼
print restored snapshot and cursor position
```

Restore intentionally keeps the active cursor identity. Running `era restore feature` while on `main` moves `main` to feature's snapshot; it does not switch the command context to the `feature` branch.

## Workspace add flow

```text
era workspace add PATH [--repo REPO] [--workspace ID] [--from TARGET]
   │
   ▼
infer omitted repo, workspace ID, and base target
   │
   ├─ missing path ───────► create and materialize base snapshot
   ├─ empty path ─────────► connect and materialize base snapshot
   ├─ non-empty path ─────► connect/adopt without overwriting files
   └─ nested path ────────► reject by default
```

`workspace add` is intentionally idempotent for the same workspace/path pair. It is the single public command for both creating a new workspace directory and adopting an existing directory.

## Watch flow

```text
era watch
   │
   ▼
start materializer watcher
   │
   ├─ receive filtered path hints
   ├─ invalidate affected capture-cache entries
   ├─ debounce bursts of edits
   ├─ request hinted capture for watch-triggered snapshots
   └─ periodically reconcile full tree
   │
   ▼
request changed-only automatic snapshot
   │
   ▼
print snapshot activity and continue foreground loop
```

Watch snapshots carry structured provenance such as trigger, workspace, agent, task, and model when provided by the caller.

## Timeline flow

```text
era timeline
   │
   ▼
open current repository/workspace
   │
   ├─ load snapshots from the repository snapshot index
   ├─ compare the working directory against the current cursor
   ├─ mark the cursor snapshot and any snapshots with matching root trees
   └─ render an undo-tree-style snapshot tree
      └─ collapse long linear runs of unlabeled automatic snapshots
```

Timeline output is intentionally graph-shaped instead of a raw first-parent log. It shows where the current cursor will attach future snapshots and, separately, which saved snapshot(s) match the files on disk. Because the tree is index-backed, previous futures remain visible after `restore` moves a cursor backward. This keeps watch-heavy histories readable without explicit paging flags.

## Responsibilities

- Provide the public `era` command surface.
- Keep command output clear, concise, and script-friendly.
- Expose verbose diagnostics without making normal output noisy.
- Wire repository operations to a filesystem materializer using the current workspace capture cache.
- Connect or lazily adopt external workspace directories through repository APIs.
- Run the foreground watch/debounce/reconcile loop.
- Configure tracing so diagnostics go to stderr and remain disabled unless explicitly requested.

## Boundaries

CLI does not:

- Define object formats.
- Store objects directly.
- Walk or restore the working directory directly.
- Own ref/cursor or snapshot policy beyond command-level intent.
- Act as the only supported integration path for agents or tools.

The library APIs remain the primary integration surface.

## Component relationships

```text
┌───────────────────────┐
│ humans / agents       │
│ shell commands        │
└───────────┬───────────┘
            │
            ▼
┌───────────────────────┐
│ era-cli               │
│ command translation   │
└───────────┬───────────┘
            │ Repository API
            ▼
┌───────────────────────┐
│ era-repository        │
│ durable behavior      │
└───────────┬───────────┘
            │
            ├──────────────► era-materialization
            └──────────────► era-object-store
```

## v0 constraints

- Commands operate from a repository root or connected workspace root; parent-directory discovery remains future work.
- The watch loop runs in the foreground.
- One-shot commands construct fresh materializer instances backed by the workspace's persistent capture cache.
- Branch/switch commands expose the repository-root branch-ref implementation.
- `workspace add` and `workspace list` exist, but background daemons and fleet supervision are future work.
- `timeline` renders the indexed snapshot graph and collapses linear unlabeled auto-snapshot runs; richer filtering and compact graph/provenance indexes are future work.

## Future seams

As the library API matures, CLI commands should remain small wrappers around reusable repository operations. Agent harnesses, editor plugins, and automation should be able to reproduce CLI behavior through library calls without depending on terminal parsing. Future merge commands should render structured conflicts produced by repository orchestration and `era-merge`, not make conflict marker text the durable model.
