# Architecture

A version control system designed for the era of agentic work: where snapshots are free, divergence is natural, history is dense, and the storage substrate is built for both humans and the millions of parallel agents that will use it.

This document describes the architecture at a conceptual level. It focuses on **v0** — the smallest system that proves the thesis — while noting where future work (notably FUSE) will plug in.

---

## Thesis

Version control today is built around a 2005 mental model: a working directory, a staging area, branches, and commits made by a careful human who remembers to `git add`. That model strains under modern workloads, and breaks entirely when the "user" is a fleet of agents producing thousands of parallel futures per hour.

This project is built on three claims:

1. **Snapshots should be free.** The whole tree, captured whenever files change, costs almost nothing if the storage is content-addressed and copy-on-write. There is no reason to ask the user what to commit. There is no reason for a staging area.
2. **Divergence should be natural.** If a workspace goes back to an older state and edits from there, history should fork because the files changed — not because the user predeclared a branch.
3. **Time travel should be a primitive operation.** Reading the tree at any past moment should be as cheap as reading it now.

Everything in the architecture follows from these claims.

---

## Design Principles

- **Content-addressed everywhere.** Identity is hash, not path. Two identical files are stored once. Two identical trees are stored once. Reproducibility, deduplication, and integrity fall out for free.
- **Copy-on-write at every layer.** Snapshots share storage with their parents. Only what changes costs anything.
- **Provenance is structured, not a string.** Every snapshot records who (or what) made it, with what tools, from what parent action. Agent-generated history is auditable by construction.
- **No staging, no manual tracking.** The system observes the working tree and decides what matters. Users and agents can rapid-fire "snapshot if changed"; labels are optional annotations for convenience, not the act that makes history exist.
- **Layers are honest.** What the data is (the object store) and how files appear on disk (materialization) are separate concerns with a narrow interface between them. Future implementations slot into the seam without touching everything else.
- **Minimum effective abstraction.** Build the smallest thing that makes the thesis testable. Add complexity only when a real workload demands it.

---

## Core Concepts

### Blob

The bytes of a single file, addressed by a hash of its contents. Identical bytes have identical hashes and are stored exactly once, regardless of where they appear in the tree, on which branch, or at which point in history.

### Tree

A directory listing — an ordered set of named entries, each pointing to either another tree or a blob. A tree is itself addressed by a hash of its serialized contents. Two directories with identical contents share storage; a directory whose contents have not changed since the previous snapshot is the same tree object as before.

In the current implementation, tree entries use UTF-8 single-segment names. Emoji and non-English characters are supported and preserved exactly; Unicode normalization is not applied. Empty names, `.`, `..`, names containing `/`, and names containing NUL are invalid.

This is the object-level copy-on-write magic: changing a single file in a deeply nested directory writes a new blob for that full file, the tree containing it, and the trees on the path back to the root. Everything else is shared with the prior state. The current implementation does not do filesystem-level CoW, block-level chunking, or delta compression; if one byte changes in a large file, the changed file is stored as a new full blob.

### State

A state is the full content tree of the project at a moment: all tracked files and directories, independent of names like branch or commit. The current state of a working directory can be derived by scanning the files and computing the root tree ID.

### Snapshot

A persisted state with history. A snapshot points to the root tree, references its parent snapshot(s), and carries structured metadata: timestamp, author, optional human-readable message, and provenance (what tool or agent produced it, what action it corresponded to).

In the current implementation, snapshots have deterministic canonical bytes and content-addressed IDs. Small golden fixtures lock the v1 snapshot encoding for compatibility. Snapshots form a directed acyclic graph via their parent pointers. A linear sequence of snapshots is just one path through the graph. `restore` moves the active branch/workspace cursor to an old state; recording new changes after restore naturally creates a divergent future from that restored point.

There is no distinction between "auto-snapshots" and "commits" in the storage layer — they are the same object. The default behavior should be unlabeled, changed-only snapshots. Human-readable labels are optional annotations that make important states easy to find again; they are not required for history to exist.

### Workspace Cursor / Line of Work

The current file state can be derived from the directory, but the next history edge cannot always be derived from files alone. The system still needs a cursor for each workspace that answers: "if these files changed, which snapshot is their parent?" This cursor preserves causality, disambiguates identical file trees that appear at different points in history, and gives future time-travel UX a clear place to fork from the state the user actually chose.

The current implementation supports both branch refs and workspace refs. Repository-root commands still expose `era branch` / `era switch` for v0 named-line workflows. External workspaces use `.era/refs/workspaces/<workspace-id>` cursors so multiple agents can advance independent lines of work against the same object store without contending on global `HEAD`.

### Working Directory / Workspace

A workspace is a materialized directory plus its local execution context. The working directory is the place where files exist as ordinary bytes that tools can read and write. Everything else lives in the object store as content-addressed objects.

The current state of the working directory may match a known snapshot exactly, or it may have drifted — files added, modified, or deleted relative to the workspace cursor. The system observes this drift and turns it into new snapshots automatically or when asked to "snapshot if changed."

### Provenance

Structured metadata attached to each snapshot describing how it came to exist. For human work this is minimal (author, timestamp, optional label). For agent work it is rich: the agent's identity, the model used, the prompt or task identifier, the parent action in a chain of agent operations. Provenance is queryable — "show me everything this agent wrote yesterday" is a first-class operation, not a log-grep.

---

## Architectural Layers

The system is composed of four layers, each with a clear responsibility and a narrow interface to the layers above and below. `era-core` sits beside those layers as the shared domain model: object identity, trees, snapshots, and provenance.

```
┌─────────────────────────────────────────────┐      ┌─────────────────────────────┐
│  CLI / Library API                          │─────▶│  Core domain model          │
├─────────────────────────────────────────────┤      │                             │
│  Repository                                 │─────▶│  ObjectId                   │
├─────────────────────────────────────────────┤      │  Tree                       │
│  Materialization                            │─────▶│  Snapshot                   │
├─────────────────────────────────────────────┤      │  Provenance                 │
│  Object Store                               │─────▶│                             │
└─────────────────────────────────────────────┘      │  shared vocabulary          │
                                                     └─────────────────────────────┘
```

### Component References

These companion documents expand the architecture by component and should stay aligned with this document:

- [Core](components/core.md) — shared object identity, tree, snapshot, and provenance model.
- [Object Store](components/object-store.md) — content-addressed persistence and integrity verification.
- [Materialization](components/materialization.md) — working-directory capture, comparison, restore, watching, and hash caching.
- [Repository](components/repository.md) — refs/cursors, snapshot policy, status, timeline, switch, and restore orchestration.
- [CLI](components/cli.md) — command surface, terminal output, tracing setup, and foreground watch loop.

### Object Store

The bottom layer. Knows nothing about paths, workspace cursors, or history. Speaks purely in hashes and bytes.

Responsibilities:

- Store and retrieve content-addressed blobs, trees, and snapshots
- Verify integrity (a hash always matches the bytes it names)
- Deduplicate transparently
- Eventually: garbage-collect unreachable objects

The object store has no notion of "current state" or "working directory." It is a key-value store where keys are content hashes and values are the immutable bytes of objects. The same object store can back any number of repositories or working directories.

The implemented object-store slices cover an async object-store interface plus a local filesystem-backed implementation for blobs, trees, and snapshots. It uses BLAKE3 object IDs, stores objects under sharded `<kind>/<prefix>/<object-id>` directories, deduplicates identical bytes, and verifies hashes on read so corruption is surfaced immediately. Tree and snapshot objects are stored as deterministic canonical bytes and decoded with canonical-order validation. Local repositories place this store under `.era/objects`.

### Materialization

The seam between the abstract world of snapshots and the concrete world of files on disk. This layer translates between "snapshot X should be visible at path P" and the actual file operations that make it so.

Responsibilities:

- Read a snapshot from the object store and produce a working directory matching it
- Observe a working directory and report how it differs from a known snapshot
- Watch for changes in the working directory and notify higher layers

In v0, materialization works by ordinary file operations — copying bytes from the object store onto the filesystem, walking the working tree to detect changes, and using a platform-appropriate filesystem watcher to observe writes. The materialization API should still be async and capability-oriented from the start, so repository code does not depend on the copy-based implementation detail.

The current implementation covers both directions for local workflows. `FilesystemMaterializer` can capture a working directory into blob/tree objects, scan a working directory without storing objects to compute the tree ID that status compares, compare a working directory with a stored tree to produce path-level added/modified/deleted/type-changed status entries, watch a working directory for filesystem change hints, and materialize a stored tree back into the working directory for context switching and restore. Capture, scan, and compare use a per-materializer in-memory hash cache so a long-running watcher can reuse unchanged file hashes. Capture, scan, compare, and watch filtering use configurable exact directory-name exclusions; defaults skip Era metadata, Git metadata, and common generated/transient directories such as `target`, `node_modules`, `.next`, `dist`, `build`, `.cache`, and `__pycache__`. External workspace pointer files named `.era` are skipped and preserved like metadata. Symlinks are not followed; by default they are skipped and reported, and callers can choose an error policy instead. Materialization preserves excluded entries that are outside the target tree, including `.era`, so repository metadata, workspace pointers, and generated caches are not deleted during restore.

This layer is intentionally a replaceable component. Future implementations (hardlink-based, reflink-based, FUSE-based) will plug in here without changing anything above. The interface this layer presents to the repository is small and stable: "checkout this snapshot at this path," "what does this path look like now," "tell me when something changes."

### Repository

The orchestration layer. Knows about history, snapshots-in-context, workspace cursors/refs, and the policies that govern when and how snapshots are taken.

Responsibilities:

- Manage refs that anchor workspace cursors or named lines of work
- Apply snapshot policy for labeled, unlabeled, automatic, and safety snapshots
- Implement higher-level operations: diff between snapshots, walk history, merge divergent lines of work
- Coordinate tracking policy with the materialization layer's include/exclude behavior

The current repository implementation covers local init, open, labeled user-requested snapshots, changed-only unlabeled snapshots, changed-only automatic snapshots, first-parent timeline walking, indexed snapshot graph traversal, path-aware working-tree status comparison, branch listing/creation/switching, workspace add/list, lazy workspace connection through CLI `--repo`, and whole-tree restore. Init creates `.era/HEAD`, `.era/refs/heads/main`, `.era/objects`, and `.era/index/snapshots`, captures the working directory through the materializer, stores an initial snapshot, records it in the snapshot index, and points `main` at it. External workspaces store a pointer file at `<workspace>/.era`, registry path metadata under `.era/workspaces/<id>/path`, and an independent cursor under `.era/refs/workspaces/<id>`. Labeled snapshots capture the current tree, store a snapshot with the current cursor tip as parent, index it, and advance the cursor even if the tree did not change so the label has a durable place to live. Unlabeled snapshot requests capture the current tree and advance the cursor only when the root tree differs from the current snapshot, avoiding duplicate history entries. Status compares the working tree to the current cursor snapshot and reports the root tree comparison plus sorted path-level changes. Branch creation writes another ref pointing at the current saved snapshot. Switching branches and restoring snapshots save unsnapped work first, then ask the materializer to reconcile the working directory; switching inside an external workspace advances that workspace cursor instead of global `HEAD`. Restore resolves its target before the safety snapshot, materializes the target tree, and moves the active branch/workspace cursor to the restored snapshot. Snapshot metadata includes a timestamp, optional author, optional message, and structured provenance. Object writes remain lock-free; mutable refs, workspace registry records, and snapshot index rebuilds are protected with scoped lock files and atomic ref replacement.

The repository is where intelligence lives. It is also where most of the v0 design space is, because the rules for "when do we snapshot, what do we include, how do we merge" are precisely what differentiates this system from git.

### CLI / Library API

The user-facing surface. A thin layer that translates user intent into repository operations and presents results.

The library API is the primary interface; the CLI is a thin shell over it. This ordering matters: the library should be usable directly from agent harnesses, editor plugins, and other tooling without going through a subprocess. Agents are first-class clients.

The current CLI exposes the implemented repository workflows from the current directory: `era init`, `era snap`, `era snap "label"`, `era snap --message "..."`, `era status`, `era branch`, `era branch NAME`, `era switch NAME`, `era restore SNAPSHOT_OR_LABEL`, `era watch`, `era watch --once`, `era timeline`, `era workspace add PATH`, and `era workspace list`. It uses the filesystem materializer and local repository APIs directly, prints clean concise output by default, provides a global `--verbose` flag for debugging details, uses adaptive terminal colors when supported, sends diagnostics and tracing to stderr, and keeps tracing disabled unless `ERA_LOG` or `RUST_LOG` is set. `era snap` without a label is the rapid-fire "snapshot if files changed" command and creates an unlabeled snapshot only when the current tree differs from the current cursor. `era snap "label"` / `era snap --message "..."` attaches a human-readable label to the current state. `era status` compares the working tree to the current saved snapshot, reports whether changes are detected, and lists changed paths with `A`, `M`, `D`, or `T` markers when dirty. `era workspace add PATH` creates a missing workspace directory, materializes the inferred base state into an empty workspace, or adopts a non-empty directory as dirty relative to the base; nested workspaces inside another workspace are rejected by default. `era snap/status/restore/watch/timeline --repo REPO --workspace ID` can lazily connect the current directory to a shared repo when an agent starts working without prior setup. `era restore` saves dirty current work as an automatic safety snapshot, materializes the target snapshot, and moves the active branch/workspace cursor so later snapshots branch from the restored point. `era timeline` renders the indexed snapshot tree, marks the current cursor with `@`, marks saved snapshots matching the working tree with `◎`, and collapses long linear runs of unlabeled automatic snapshots. `era watch` runs in the foreground, debounces filesystem events, periodically reconciles the full tree, and creates unlabeled automatic snapshots when the tree changed. Watch snapshots use structured provenance attributes for `trigger`, `workspace`, and optional agent/task/model fields; the timestamp shown in timeline output is a display title, not a stored label. The `branch` and `switch` commands expose the current branch-ref implementation; they are useful for v0 but not sacred user vocabulary.

---

## Cross-Cutting Concerns

### Hash Caching

Naively, capturing a snapshot means hashing every file in the working tree. For large repositories this is unacceptably slow, and it makes the auto-snapshot model untenable.

The system maintains a per-file hash cache keyed on stable filesystem identifiers — typically the inode, file size, and modification time. If those have not changed since the file was last hashed, the cached hash is reused. Snapshotting an unchanged 10GB tree should take milliseconds, not seconds.

The current implementation has a per-materializer in-memory hash cache. That makes long-running `era watch` sessions cheap after the first capture and keeps cache state naturally scoped to a single workspace. A persistent cache under repository/workspace metadata remains future work and should preserve that same boundary: shared object stores are global, but hash caches belong to materialized workspaces.

This cache is the single most important performance component in the system. Without it, "fast" is impossible.

### Filesystem Watching

The auto-snapshot model requires knowing when the working directory has changed. The materialization layer watches the working directory using platform-native facilities and emits change events upward.

The current user-facing form is foreground `era watch`, not a daemon started by `era init`. It debounces watcher events, invalidates affected hash-cache entries, and periodically reconciles the full working tree to catch missed events. Watchers are imperfect: events can be lost, coalesced, or duplicated, and edge cases differ across platforms. The watch flow treats events as hints, not ground truth — reconciliation still walks the working tree to confirm what changed, using the hash cache to keep this cheap.

### Intelligent Tracking

Unlike git, there is no `.gitignore` and no `git add`. The system decides what to track based on a layered set of heuristics:

- File type and content (build artifacts, binaries with known signatures, compiled outputs)
- Conventional locations (common build directories, dependency caches, transient state)
- Repository-local rules (an optional override file for explicit include/exclude)

The goal is that the system is correct without configuration on the vast majority of repositories, and configurable when it isn't. This is a v0 problem with substantial v1 and v2 refinement ahead — the heuristics will evolve as real workloads expose what they get wrong.

### Provenance Capture

When the repository layer creates a snapshot, it accepts structured provenance metadata. For human-driven snapshots this is minimal and may be absent. For agent-driven snapshots, the agent harness supplies an identity, a model, a parent action ID, and any task context that should be associated with the work.

This metadata is stored as part of the snapshot object and indexed for queryability. Provenance is the substrate that makes auditing agent work possible.

### Concurrency and Locks

The object store is immutable and content-addressed, so object writes do not take a repository-wide lock. Concurrent agents can store the same blob, tree, or snapshot and the local object store deduplicates the winner safely.

Mutable metadata is different. Branch refs, workspace refs, and workspace registry records are serialized with narrow lock files under `.era/locks/`, and refs are replaced through temporary files plus atomic rename. Snapshotting holds no ref lock while hashing the working tree; it acquires the specific cursor lock only to re-read the parent, decide whether a changed-only snapshot still needs saving, store the snapshot object, and advance that cursor. Different workspaces therefore snapshot concurrently, while duplicate snapshots in the same workspace collapse after the second process sees the updated cursor.

### Tracing and Diagnostics

Era uses structured `tracing` instrumentation for debugging, performance testing, and future operational visibility. Tracing is off by default for script-friendly command behavior and can be enabled with `ERA_LOG` or `RUST_LOG`. I/O-heavy components such as object storage and materialization should emit spans/events with object IDs, paths, byte counts, and reuse/write decisions, while avoiding raw file contents.

---

## User Flows

How the system actually feels to use, contrasted with git where instructive. These flows describe intent and outcomes; the exact commands shown are illustrative rather than final.

### Starting Work on a Project

A user runs an init command in a directory. The system creates a hidden metadata directory and takes an initial snapshot of whatever is already there. In the current implementation, the user then runs `era watch` when they want foreground automatic snapshots; a future daemon can make watching implicit after init.

There is no equivalent to `git add` for telling the system which files matter. The intelligent tracking layer decides, with an optional repository-local override file for cases where the heuristics get it wrong.

_Contrast with git:_ no separate init, add, and commit ceremony. One step, and the system is alive.

### Day-to-Day Editing

The user edits files while `era watch` is running. The system observes. After a brief debounce window of inactivity, an unlabeled snapshot is taken automatically. The user runs no per-change command; the snapshot just appears in the timeline.

The user cannot lose work to forgotten commits. Their attention stays on the work, not on version-control hygiene. A dense history accumulates in the background, and tooling decides how much of it to surface.

_Contrast with git:_ no `add`, no `commit`, no commit-message-for-every-change. Editor plugins that auto-commit on save approximate this but produce a noisy, unstructured history; here, density is the design, and the timeline UI handles presentation.

### Remembering a Meaningful Moment

When the user or an agent wants to make sure work is recorded, they can rapid-fire:

```
era snap
```

This means "snapshot if files changed." If the current tree matches the workspace cursor, Era does nothing. If the files differ, Era creates an unlabeled snapshot and advances the current line of work.

When the user finishes something they want to find by name later — a feature, a refactor, a known-good state before risky changes — the label is optional:

```
era snap "feature: cellular automata loading spinner"
```

This means "attach a human-readable name to this state." If the tree has not been saved yet, Era saves it and attaches the label. If the exact tree is already saved, Era can represent the label without making the user think about capture versus metadata. In the current implementation, this creates a snapshot object with the current root tree and the supplied message, so the timeline shows the label as the headline.

_Contrast with git:_ the closest analog is `git commit -m`, but Era does not require a message for every saved state and does not have a staging area.

### Going Back in Time

The user wants the tree as it was an hour ago, or before yesterday's refactor, or at the labeled "feature complete" moment.

```
era restore "feature complete"
era restore abc123def456
```

Time travel is a primitive operation. The current implementation restores whole trees by exact label, unique snapshot ID prefix, branch/workspace ref, or full snapshot ID. The user does not check out a commit and then remember to come back — they ask for what they want, and the working directory becomes that.

Before restore changes the working directory, Era saves any unsnapped current work, so nothing is lost by traveling. Restore then moves the active branch/workspace cursor to the selected state, so later edits and snapshots naturally diverge from that restored point.

_Contrast with git:_ `reflog` and `checkout` exist for this with caveats — uncommitted work blocks switching, the reflog is technical and ephemeral, restoring a single file requires `git show <commit>:path > path`.

### Diverging for an Experiment

The user wants to try something risky from an older or current state. Conceptually, they do not need to create a branch first: they go to the state they want, edit files, and the next snapshot naturally creates a new future from that parent.

```text
A -- B -- C
 \
  D -- E
```

The fork exists because state `A` gained another child, not because the user performed a special branch ceremony. In the current v0 CLI, `era branch NAME` still exposes named refs for convenience and for context switching, but the architecture should treat those as named cursors over the graph rather than the core user model.

_Contrast with git:_ no need to predeclare a branch before experimentation, and no uncommitted-change blocker before moving through history.

### Switching Contexts

The user is mid-task, gets pulled into something else, and needs to move the workspace to another named line of work.

```
era switch main
```

Era saves the current state — including any in-progress work — on the current line before materializing the target line. Switching loses nothing. When the user returns later, they pick up exactly where they left off. The command name is current implementation vocabulary; future UX may prefer `go`, `resume`, or workspace commands.

_Contrast with git:_ no `git stash` dance, no "your local changes would be overwritten" error, no half-staged work caught between branches.

### Reviewing Recent Activity

The user wants to know what changed recently — in the last hour, since they last looked, or by a particular author.

```
era timeline                      # snapshot tree with cursor/worktree markers
era timeline --verbose            # include full IDs, roots, timestamps, and provenance
# Future filters:
era timeline --labeled            # only the meaningful moments
era timeline --since "9am"
era timeline --by claude-sonnet
```

Provenance makes the filtered forms possible. Every snapshot knows its author, including agents, and the timeline can be filtered on any provenance field. Current timeline output collapses long linear runs of unlabeled automatic snapshots so watch-heavy histories remain readable.

_Contrast with git:_ `git log` exists, but author is a free-form string, and "everything an agent did" requires conventions the user enforces themselves.

### Working with an Agent Fleet

A user (or another agent) spawns ten coding agents to attempt the same task in parallel. Each gets its own workspace cursor starting from the same state.

```
era workspace add ../runs/agent-1 --from abc123
for i in {2..10}; do era workspace add ../runs/agent-$i --from abc123; done
```

Each agent works in its own materialized workspace (eventually, its own FUSE mount), reading and writing freely. Each write produces an unlabeled auto-snapshot with structured provenance for the workspace, agent, model, and task. When the agents finish, the user reviews the resulting futures, picks the winner, merges it, and discards the rest.

The total storage cost is dominated by the actual changes the agents made — typically kilobytes each — not by ten copies of the repository. This is the model that makes agent-scale parallelism economical.

The architecture uses **workspace** for the per-directory execution context. A shared repository owns object storage, branch refs, workspace refs, the snapshot graph, and lightweight workspace registry records. Each workspace owns a materialized path, its watcher/debounce loop, hash cache, current checkout context, and workspace ID. The current CLI records `workspace=default` for repository-root watch snapshots unless `era watch --workspace ...` overrides it; connected workspace commands infer the workspace provenance from the pointer file.

_Contrast with git:_ `git worktree` and ten clones each cost real disk space and real I/O. Fleets at this scale aren't practical in git's model.

### Recovering from a Mistake

A script destroys half the files in the user's working directory. In git, any uncommitted work is lost and the user has to remember what was on disk.

Here, every state was captured. The user restores a snapshot from before the mistake and the working directory returns to before the disaster. The destructive script's effects remain in the timeline as snapshots the user can ignore or examine, but nothing is irrecoverable.

_Contrast with git:_ hope you committed. If you didn't, you're using your editor's local history.

### Resolving a Conflict

When a merge can't be resolved automatically, the system produces a structured conflict. For v0 this is at the file level: "these files were modified on both divergent futures in incompatible ways, here are both versions."

The user resolves the conflicts in their editor (conflict files appear as ordinary files in the working directory). Once resolved, the user marks the merge complete; the system snapshots the resolved state with both parent snapshots recorded.

In v1+, syntax-aware merge will collapse entire categories of "conflict" that aren't really conflicts — two futures both adding imports, two futures modifying different functions in the same file. For v0, file-level is the boundary.

---

## Workflow: A Snapshot's Life

To make the layers concrete, here is what happens when a file changes:

1. **An editor (or agent) writes to a file** in the working directory.
2. **The materialization layer's watcher** observes the write and emits a change event.
3. **The watch loop** debounces a series of such events and asks the repository for a changed-only automatic snapshot.
4. **The repository asks the materializer** to compute the current tree state. The materializer walks the working directory, using the hash cache to skip unchanged files during long-running watch sessions.
5. **For any file whose hash is new**, the materializer hands the bytes to the object store, which stores them and returns a hash. (Most files are unchanged and produce no new objects.)
6. **The materializer builds tree objects** from the bottom up, again writing only new trees to the object store. Most trees are reused from the previous snapshot.
7. **The repository builds a snapshot object** referencing the new root tree, the previous snapshot as its parent, the current timestamp, and any provenance supplied by the agent or user.
8. **The repository updates the current workspace cursor / ref** to point at the new snapshot.

The total cost: a few hashes, a few small writes, a pointer update. Sub-100ms even on a large repository, well under the auto-snapshot debounce window.

Time travel is the inverse of this process: the repository looks up a snapshot by hash, hands it to the materializer, and the materializer reconciles the working directory to match — writing only the files that differ from the current state.

---

## What's Out of Scope for v0

These are real and important problems that v0 explicitly does not solve, in service of shipping something testable quickly.

- **Network sync.** v0 is local-only. Sync is a v1+ concern, and the design of the object store leaves the door open for content-addressed transports without prejudging which one.
- **Garbage collection.** Auto-snapshotting will grow the store steadily. v0 lets it grow; v1 introduces reachability-based and time-based collection policies.
- **Encryption.** Plain bytes on local disk for v0.
- **Capability-based access control.** Agents and humans share the same store; isolation is a future concern.
- **Syntax-aware merge.** v0 does file-level three-way merge with structured conflicts. Tree-sitter-based AST-aware merge is the obvious v1 wedge but is a substantial subsystem in its own right.
- **Git interoperability.** v0 stands alone. Importing from and exporting to git repositories is well-defined but tedious work and is deferred.
- **Workspace fleet supervision.** v0 can register/list connected workspaces and run commands from each workspace, but it does not yet provide a daemon or fanout supervisor for launching, monitoring, and summarizing large agent fleets.

---

## The FUSE Future

The materialization layer exists, in part, so that FUSE can eventually be added without disrupting anything else.

In v0, materialization works by copying bytes between the object store and ordinary files on disk. This is simple, portable, debuggable, and fast enough for normal use. It has one significant limitation: every working directory costs real disk space, and moving a workspace between states costs real I/O.

A FUSE-based materializer would change this. The working directory becomes a virtual filesystem mount backed directly by the object store. Moving a workspace cursor is a pointer change with no I/O. Multiple workspaces on different states share storage automatically. A hundred agents, each with its own workspace and cursor, become essentially free.

The FUSE materializer will be a peer of the copy-based one, not a replacement: each has its tradeoffs (FUSE costs syscall overhead and brings platform-specific complexity; copying costs disk and I/O). Users and agents pick what suits their workload.

Between v0 and the FUSE future, intermediate materializers — using hardlinks or filesystem-native reflinks (APFS, btrfs) — provide most of the storage benefit with none of the FUSE complexity. These are a likely v1 addition.

The architectural commitment is only this: nothing in the layers above the materializer assumes how files arrive on disk. As long as that holds, every materialization strategy is additive.

---

## Beyond v0: Where This Wants to Go

The following are not commitments but the shape of the long arc, included so that v0 decisions can be made with eventual goals in mind.

- **Syntax-aware diff and merge.** Treating files as byte streams is the universal lowest common denominator and an obvious place to do better. AST-level diff and merge collapse entire categories of "conflict" that are not really conflicts.
- **Sync as a first-class operation.** Snapshots, being content-addressed, sync efficiently — only missing objects need to move. The substrate is ready; the policies (push/pull, conflict resolution across machines, multi-master sync) are the work.
- **Queryable provenance.** A side index over the object store that makes "show me everything agent X did yesterday" or "find the snapshot where this bug was introduced" instant.
- **Structured files.** The most heretical idea in the long arc: treating files as typed records with schemas rather than opaque bytes. This is a v3+ direction and is mentioned only because it shapes some choices about how the object store represents tree entries (extensible metadata fields, future type information).
- **Multi-working-directory workflows.** A first-class concept of "many workspaces, one repository," with each workspace materializing a different snapshot. Critical for agent fleets.

These directions inform the shape of v0 — they explain why certain abstractions exist now even when they aren't strictly required — but none of them are blockers for proving the core thesis.

---

## A Note on Discipline

The point of this architecture is to make the thesis testable as quickly as possible. Every layer described here exists because it earns its keep in v0; nothing here is speculative.

The traits and seams are cheap insurance against painting into a corner. They are not licenses to over-engineer. When in doubt, build the simpler thing, and rewrite when a real workload tells you to. The fastest path to knowing whether this works is a working v0 with real agents pointed at it — not a perfect design document.
