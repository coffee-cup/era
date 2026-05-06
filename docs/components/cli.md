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
│  - timeline                                  │
├──────────────────────────────────────────────┤
│ Output rendering                             │
│  - concise default output                    │
│  - verbose diagnostics                       │
│  - adaptive color                            │
├──────────────────────────────────────────────┤
│ Runtime wiring                               │
│  - current-directory repository access       │
│  - filesystem materializer construction      │
│  - tracing setup                             │
│  - foreground watch loop                     │
└──────────────────────────────────────────────┘
```

The current command surface still includes `branch` and `switch` because v0 persists named lines as branch refs. That vocabulary is not sacred: future CLI work can introduce state/workspace-oriented commands such as `go`, `mark`, `resume`, or workspace management while keeping the repository APIs underneath.

## Command flow

```text
user command
   │
   ▼
parse arguments and global flags
   │
   ▼
open or initialize repository in current directory
   │
   ▼
construct filesystem materializer
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
capture current tree
   │
   ├─ no label and unchanged ──► print "No changes"
   ├─ no label and changed ────► create unlabeled snapshot
   └─ label supplied ──────────► create labeled snapshot for current state
   │
   ▼
print snapshot result
```

`era snap` without a label is intentionally rapid-fire and changed-only. Agents can call it after each tool action without worrying about duplicate snapshots. `era snap "label"` and `era snap --message "..."` are convenience forms for making a state easy to find by name.

## Watch flow

```text
era watch
   │
   ▼
start materializer watcher
   │
   ├─ receive filtered path hints
   ├─ invalidate affected hash-cache entries
   ├─ debounce bursts of edits
   └─ periodically reconcile full tree
   │
   ▼
request changed-only automatic snapshot
   │
   ▼
print snapshot activity and continue foreground loop
```

Watch snapshots carry structured provenance such as trigger, workspace, agent, task, and model when provided by the caller.

## Responsibilities

- Provide the public `era` command surface.
- Keep command output clear, concise, and script-friendly.
- Expose verbose diagnostics without making normal output noisy.
- Wire repository operations to a filesystem materializer for local workflows.
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

- Commands operate from the working-directory root.
- The watch loop runs in the foreground.
- One-shot commands construct fresh materializer instances, so hash-cache reuse primarily benefits long-running watch sessions.
- Branch/switch commands expose the current branch-ref implementation.
- Parent-directory discovery, background daemons, and multi-workspace supervision are future work.

## Future seams

As the library API matures, CLI commands should remain small wrappers around reusable repository operations. Agent harnesses, editor plugins, and automation should be able to reproduce CLI behavior through library calls without depending on terminal parsing.
