# Merge Component

`era-merge` is the snapshot-agnostic file merge engine. It knows nothing about refs, snapshots, object IDs, repositories, or working directories. Callers provide file versions as bytes plus optional path hints; strategies return resolved bytes/deletion or structured conflicts.

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-merge                                    │
├──────────────────────────────────────────────┤
│ File merge input                             │
│  - optional path hint                        │
│  - base / ours / theirs byte versions        │
│  - absent versions for adds/deletes          │
├──────────────────────────────────────────────┤
│ Strategy chain                               │
│  - semantic strategies can run first         │
│  - unsupported strategies fall through       │
│  - line strategy is the default fallback     │
├──────────────────────────────────────────────┤
│ Structured output                            │
│  - resolved present/deleted file             │
│  - resolved hunks plus conflict hunks        │
│  - conflict kind and side bytes              │
└──────────────────────────────────────────────┘
```

## Responsibilities

- Merge one file from base/ours/theirs byte inputs.
- Keep merge logic independent of Era's current snapshot storage.
- Provide a strategy trait for future semantic merge drivers such as JavaScript, JSON, or language-aware import merging.
- Provide a deterministic line-oriented three-way merge fallback.
- Return structured conflicts instead of embedding conflict markers in core results.

## Boundaries

Merge does not:

- Resolve snapshot targets, refs, or merge bases.
- Traverse trees or decide path-level file/dir conflicts.
- Read blobs from the object store or write merged files to disk.
- Create snapshots or advance cursors.
- Render conflict markers for editors.

Those responsibilities belong to repository orchestration, object storage, materialization, and CLI/editor integrations.

## Current implementation

The line strategy handles fast paths, adds, deletes, add/add conflicts, whole-file modify/delete conflicts, NUL-containing binary conflicts, line splitting that preserves CRLF and missing final newlines, patience-style unique-line anchors with LCS fallback, non-overlapping line edits, identical overlapping edits, insertion conflicts at the same position, modify/delete line conflicts, and structured hunk output.

Semantic merge is represented as a strategy seam only. A future JavaScript or JSON strategy can inspect the path/content, attempt semantic merge, and return `Unsupported` on parse failure so the engine falls back to line merge.

## Future seams

Repository merge should adapt stored snapshots or future patch graphs into `FileMergeInput` values, call this engine per file, and store/materialize results. If Era's underlying snapshotting changes, only that adapter should change.
