# Architecture

A version control system designed for the era of agentic work: where snapshots are free, branching is instant, history is dense, and the storage substrate is built for both humans and the millions of parallel agents that will use it.

This document describes the architecture at a conceptual level. It focuses on **v0** — the smallest system that proves the thesis — while noting where future work (notably FUSE) will plug in.

---

## Thesis

Version control today is built around a 2005 mental model: a working directory, a staging area, and commits made by a careful human who remembers to `git add`. That model strains under modern workloads, and breaks entirely when the "user" is a fleet of agents producing thousands of parallel branches per hour.

This project is built on three claims:

1. **Snapshots should be free.** The whole tree, captured at every meaningful moment, costs almost nothing if the storage is content-addressed and copy-on-write. There is no reason to ask the user what to commit. There is no reason for a staging area.
2. **Branching should be instant.** Forking is a pointer write. A thousand agent branches should cost megabytes, not gigabytes.
3. **Time travel should be a primitive operation.** Reading the tree at any past moment should be as cheap as reading it now.

Everything in the architecture follows from these claims.

---

## Design Principles

- **Content-addressed everywhere.** Identity is hash, not path. Two identical files are stored once. Two identical trees are stored once. Reproducibility, deduplication, and integrity fall out for free.
- **Copy-on-write at every layer.** Snapshots share storage with their parents. Only what changes costs anything.
- **Provenance is structured, not a string.** Every snapshot records who (or what) made it, with what tools, from what parent action. Agent-generated history is auditable by construction.
- **No staging, no manual tracking.** The system observes the working tree and decides what matters. Users describe intent at moments they care about; the system records everything else automatically.
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

### Snapshot

A complete record of the entire tree at a moment in time. A snapshot points to the root tree, references its parent snapshot(s), and carries structured metadata: timestamp, author, optional human-readable message, and provenance (what tool or agent produced it, what action it corresponded to).

In the current implementation, snapshots have deterministic canonical bytes and content-addressed IDs. Small golden fixtures lock the v1 snapshot encoding for compatibility. Snapshots form a directed acyclic graph via their parent pointers. A linear sequence of snapshots looks like git's commit history. Merges produce snapshots with multiple parents. Branches are simply named pointers into the graph.

There is no distinction between "auto-snapshots" and "commits" in the storage layer — they are the same object. The difference is only whether a human-readable label was attached. Tooling treats labeled snapshots as the meaningful waypoints and unlabeled ones as the dense history between them.

### Branch

A named, mutable pointer to a snapshot. Creating a branch is writing a small reference; switching branches is changing which snapshot the working directory reflects. Branches are not isolated worlds — they are just labeled positions in the snapshot graph, and they share all underlying storage.

### Working Directory

A materialized view of a particular snapshot at a path on the user's filesystem. The working directory is the only place where files exist as ordinary bytes that ordinary tools can read and write. Everything else lives in the object store as content-addressed objects.

The current state of the working directory may match a known snapshot exactly, or it may have drifted — files added, modified, or deleted relative to the last snapshot. The system continuously observes this drift and turns it into new snapshots automatically.

### Provenance

Structured metadata attached to each snapshot describing how it came to exist. For human work this is minimal (author, timestamp, optional label). For agent work it is rich: the agent's identity, the model used, the prompt or task identifier, the parent action in a chain of agent operations. Provenance is queryable — "show me everything this agent wrote yesterday" is a first-class operation, not a log-grep.

---

## Architectural Layers

The system is composed of four layers, each with a clear responsibility and a narrow interface to the layers above and below.

```
┌─────────────────────────────────────────────┐
│  CLI / Library API                          │
├─────────────────────────────────────────────┤
│  Repository                                 │
├─────────────────────────────────────────────┤
│  Materialization                            │
├─────────────────────────────────────────────┤
│  Object Store                               │
└─────────────────────────────────────────────┘
```

### Object Store

The bottom layer. Knows nothing about paths, branches, or history. Speaks purely in hashes and bytes.

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

The current implementation covers both directions for local workflows. `FilesystemMaterializer` can capture a working directory into blob/tree objects, scan a working directory without storing objects to compute the tree ID that status compares, and materialize a stored tree back into the working directory for branch switching and restore. Capture and scan use configurable exact directory-name exclusions; defaults skip Era metadata, Git metadata, and common generated/transient directories such as `target`, `node_modules`, `.next`, `dist`, `build`, `.cache`, and `__pycache__`. Symlinks are not followed; by default they are skipped and reported, and callers can choose an error policy instead. Materialization preserves excluded directories that are outside the target tree, including `.era`, so repository metadata and generated caches are not deleted during restore.

This layer is intentionally a replaceable component. Future implementations (hardlink-based, reflink-based, FUSE-based) will plug in here without changing anything above. The interface this layer presents to the repository is small and stable: "checkout this snapshot at this path," "what does this path look like now," "tell me when something changes."

### Repository

The orchestration layer. Knows about branches, history, snapshots-in-context, and the policies that govern when and how snapshots are taken.

Responsibilities:

- Manage branches and their references
- Drive the auto-snapshot loop: receive change events from the materializer, decide when to capture a snapshot, write it through the object store
- Implement higher-level operations: diff between snapshots, walk history, merge branches
- Apply tracking heuristics: decide which files should and should not be part of snapshots

The current repository implementation covers local init, open, manual snapshots, first-parent timeline walking, working-tree status comparison, branch listing/creation/switching, and whole-tree restore. Init creates `.era/HEAD`, `.era/refs/heads/main`, and `.era/objects`, captures the working directory through the materializer, stores an initial snapshot, and points `main` at it. Manual snapshots capture the current tree, store a snapshot with the current branch tip as parent, and advance the branch ref. Branch creation writes another ref pointing at the current saved snapshot. Switching branches and restoring snapshots save unsnapped work first, then ask the materializer to reconcile the working directory. Restore materializes a target snapshot without moving the current branch ref. Snapshot metadata includes a timestamp, optional author, optional message, and structured provenance.

The repository is where intelligence lives. It is also where most of the v0 design space is, because the rules for "when do we snapshot, what do we include, how do we merge" are precisely what differentiates this system from git.

### CLI / Library API

The user-facing surface. A thin layer that translates user intent into repository operations and presents results.

The library API is the primary interface; the CLI is a thin shell over it. This ordering matters: the library should be usable directly from agent harnesses, editor plugins, and other tooling without going through a subprocess. Agents are first-class clients.

The current CLI exposes the implemented repository workflows from the current directory: `era init`, `era snap`, `era snap "label"`, `era snap --message "..."`, `era status`, `era branch`, `era branch NAME`, `era switch NAME`, `era restore SNAPSHOT_OR_LABEL`, and `era timeline`. It uses the filesystem materializer and local repository APIs directly, prints clean concise output by default, provides a global `--verbose` flag for debugging details, uses adaptive terminal colors when supported, sends diagnostics and tracing to stderr, and keeps tracing disabled unless `ERA_LOG` or `RUST_LOG` is set. `era snap` is the single user-facing "remember this state" command: it accepts an optional label and defaults to the current local timestamp formatted like `Jan 1, 2024 11:11:11`. `era status` compares the working tree to the current saved snapshot and reports whether changes are detected.

---

## Cross-Cutting Concerns

### Hash Caching

Naively, capturing a snapshot means hashing every file in the working tree. For large repositories this is unacceptably slow, and it makes the auto-snapshot model untenable.

The system maintains a per-file hash cache keyed on stable filesystem identifiers — typically the inode, file size, and modification time. If those have not changed since the file was last hashed, the cached hash is reused. Snapshotting an unchanged 10GB tree should take milliseconds, not seconds.

This cache is the single most important performance component in the system. Without it, "fast" is impossible.

### Filesystem Watching

The auto-snapshot model requires knowing when the working directory has changed. The materialization layer watches the working directory using platform-native facilities (fanotify or inotify on Linux, FSEvents on macOS) and emits change events upward.

Watchers are imperfect: events can be lost, coalesced, or duplicated, and edge cases differ across platforms. The repository layer treats watcher events as hints, not ground truth — a periodic reconciliation pass walks the working tree to confirm what the watcher reported (using the hash cache to keep this cheap).

### Intelligent Tracking

Unlike git, there is no `.gitignore` and no `git add`. The system decides what to track based on a layered set of heuristics:

- File type and content (build artifacts, binaries with known signatures, compiled outputs)
- Conventional locations (common build directories, dependency caches, transient state)
- Repository-local rules (an optional override file for explicit include/exclude)

The goal is that the system is correct without configuration on the vast majority of repositories, and configurable when it isn't. This is a v0 problem with substantial v1 and v2 refinement ahead — the heuristics will evolve as real workloads expose what they get wrong.

### Provenance Capture

When the repository layer creates a snapshot, it accepts structured provenance metadata. For human-driven snapshots this is minimal and may be absent. For agent-driven snapshots, the agent harness supplies an identity, a model, a parent action ID, and any task context that should be associated with the work.

This metadata is stored as part of the snapshot object and indexed for queryability. Provenance is the substrate that makes auditing agent work possible.

### Tracing and Diagnostics

Era uses structured `tracing` instrumentation for debugging, performance testing, and future operational visibility. Tracing is off by default for script-friendly command behavior and can be enabled with `ERA_LOG` or `RUST_LOG`. I/O-heavy components such as object storage and materialization should emit spans/events with object IDs, paths, byte counts, and reuse/write decisions, while avoiding raw file contents.

---

## User Flows

How the system actually feels to use, contrasted with git where instructive. These flows describe intent and outcomes; the exact commands shown are illustrative rather than final.

### Starting Work on a Project

A user runs an init command in a directory. The system creates a hidden metadata directory, takes an initial snapshot of whatever is already there, and begins watching for changes. From this moment on, every meaningful change is captured automatically.

There is no equivalent to `git add` for telling the system which files matter. The intelligent tracking layer decides, with an optional repository-local override file for cases where the heuristics get it wrong.

_Contrast with git:_ no separate init, add, and commit ceremony. One step, and the system is alive.

### Day-to-Day Editing

The user edits files. The system observes. After a brief debounce window of inactivity, an unlabeled snapshot is taken automatically. The user runs no command; the snapshot just appears in the timeline.

The user cannot lose work to forgotten commits. Their attention stays on the work, not on version-control hygiene. A dense history accumulates in the background, and tooling decides how much of it to surface.

_Contrast with git:_ no `add`, no `commit`, no commit-message-for-every-change. Editor plugins that auto-commit on save approximate this but produce a noisy, unstructured history; here, density is the design, and the timeline UI handles presentation.

### Remembering a Meaningful Moment

When the user finishes something they want to remember — a feature, a refactor, a known-good state before risky changes — they use one command:

```
era snap "feature: cellular automata loading spinner"
```

From the user's point of view, `era snap` means "make this current state easy to get back to." If the tree has not been saved yet, Era saves it and attaches the label. If the exact tree is already saved, Era can represent the label without making the user think about capture versus metadata. In the current implementation, this command creates a snapshot object with the current root tree and the supplied message, so the timeline shows the label as the headline.

_Contrast with git:_ the closest analog is `git commit -m`, but there is no staging area and no separate `mark` command for users to learn.

### Going Back in Time

The user wants the tree as it was an hour ago, or before yesterday's refactor, or at the labeled "feature complete" moment.

```
era restore "feature complete"
era restore abc123def456
```

Time travel is a primitive operation. The current implementation restores whole trees by exact label, unique snapshot ID prefix, or full snapshot ID. The user does not check out a commit and then remember to come back — they ask for what they want, and the working directory becomes that.

Before restore changes the working directory, Era saves any unsnapped current work, so nothing is lost by traveling.

_Contrast with git:_ `reflog` and `checkout` exist for this with caveats — uncommitted work blocks switching, the reflog is technical and ephemeral, restoring a single file requires `git show <commit>:path > path`.

### Branching for an Experiment

The user wants to try something risky without affecting their main line.

```
era branch try-new-approach
```

A branch is a named pointer to a saved snapshot. Creating it is instantaneous and costs essentially nothing; if the working tree has unsnapped changes, Era saves them first and then creates the branch at that saved state. The user works on the branch; snapshots accumulate on it. If the experiment works, they merge. If not, they discard the branch — and even then, the work remains reachable through the timeline until garbage-collected.

_Contrast with git:_ `git checkout -b`, but without uncommitted changes blocking the switch and without the worktree/disk overhead of multiple branches existing simultaneously.

### Switching Contexts

The user is mid-task, gets pulled into something else, and needs to switch branches.

```
era switch main
```

Era saves the current state — including any in-progress work — on the current branch before materializing the target branch. Switching loses nothing. When the user returns later, they pick up exactly where they left off.

_Contrast with git:_ no `git stash` dance, no "your local changes would be overwritten" error, no half-staged work caught between branches.

### Reviewing Recent Activity

The user wants to know what changed recently — in the last hour, since they last looked, or by a particular author.

```
era timeline                      # all snapshots, newest first
era timeline --labeled            # only the meaningful moments
era timeline --since "9am"
era timeline --by claude-sonnet
```

Provenance makes the last form possible. Every snapshot knows its author, including agents, and the timeline can be filtered on any provenance field.

_Contrast with git:_ `git log` exists, but author is a free-form string, and "everything an agent did" requires conventions the user enforces themselves.

### Working with an Agent Fleet

A user (or another agent) spawns ten coding agents to attempt the same task in parallel. Each gets its own branch, forked from the current state.

```
era fanout agent-{1..10} from main
```

Each agent works in its own materialized working directory (eventually, its own FUSE mount), reading and writing freely. Each write produces an auto-snapshot labeled with that agent's provenance. When the agents finish, the user reviews the resulting branches, picks the winner, merges it, and discards the rest.

The total storage cost is dominated by the actual changes the agents made — typically kilobytes each — not by ten copies of the repository. This is the model that makes agent-scale parallelism economical.

_Contrast with git:_ `git worktree` and ten clones each cost real disk space and real I/O. Fleets at this scale aren't practical in git's model.

### Recovering from a Mistake

A script destroys half the files in the user's working directory. In git, any uncommitted work is lost and the user has to remember what was on disk.

Here, every state was captured. The user restores a snapshot from before the mistake and the working directory returns to before the disaster. The destructive script's effects remain in the timeline as snapshots the user can ignore or examine, but nothing is irrecoverable.

_Contrast with git:_ hope you committed. If you didn't, you're using your editor's local history.

### Resolving a Conflict

When a merge can't be resolved automatically, the system produces a structured conflict. For v0 this is at the file level: "these files were modified on both branches in incompatible ways, here are both versions."

The user resolves the conflicts in their editor (conflict files appear as ordinary files in the working directory). Once resolved, the user marks the merge complete; the system snapshots the resolved state with both branches as parents.

In v1+, syntax-aware merge will collapse entire categories of "conflict" that aren't really conflicts — two branches both adding imports, two branches modifying different functions in the same file. For v0, file-level is the boundary.

---

## Workflow: A Snapshot's Life

To make the layers concrete, here is what happens when a file changes:

1. **An editor (or agent) writes to a file** in the working directory.
2. **The materialization layer's watcher** observes the write and emits a change event.
3. **The repository layer** debounces a series of such events and decides a snapshot is warranted.
4. **The repository asks the materializer** to compute the current tree state. The materializer walks the working directory, using the hash cache to skip unchanged files once that cache exists.
5. **For any file whose hash is new**, the materializer hands the bytes to the object store, which stores them and returns a hash. (Most files are unchanged and produce no new objects.)
6. **The materializer builds tree objects** from the bottom up, again writing only new trees to the object store. Most trees are reused from the previous snapshot.
7. **The repository builds a snapshot object** referencing the new root tree, the previous snapshot as its parent, the current timestamp, and any provenance supplied by the agent or user.
8. **The repository updates the current branch's reference** to point at the new snapshot.

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
- **Multiple working directories from one repository.** v0 assumes one working directory per repository. The architecture does not preclude many — the materialization interface takes a path as a parameter for exactly this reason — but the workflows aren't built out yet.

---

## The FUSE Future

The materialization layer exists, in part, so that FUSE can eventually be added without disrupting anything else.

In v0, materialization works by copying bytes between the object store and ordinary files on disk. This is simple, portable, debuggable, and fast enough for normal use. It has one significant limitation: every working directory costs real disk space, and switching branches costs real I/O.

A FUSE-based materializer would change this. The working directory becomes a virtual filesystem mount backed directly by the object store. Switching branches is a pointer change with no I/O. Multiple working directories on different branches share storage automatically. A hundred agents, each in its own branch, each in its own working directory, become essentially free.

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
