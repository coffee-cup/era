# Core Component

`era-core` is the shared domain model for Era. It defines the immutable values that every other component agrees on: object identity, object kinds, trees, snapshots, and snapshot provenance.

The core component has no filesystem responsibility, no repository state, and no background behavior. It is the vocabulary that lets storage, materialization, repository orchestration, and clients talk about the same objects without sharing implementation details.

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-core                                     │
├──────────────────────────────────────────────┤
│ Object identity                              │
│  - content-addressed IDs                     │
│  - object kinds                              │
├──────────────────────────────────────────────┤
│ Tree model                                   │
│  - directory entries                         │
│  - blob/tree entry kinds                     │
│  - deterministic ordering                    │
├──────────────────────────────────────────────┤
│ Snapshot model                               │
│  - root tree                                 │
│  - parent snapshots                          │
│  - timestamp, author, label                  │
│  - structured provenance attributes          │
└──────────────────────────────────────────────┘
```

## Core object flow

```text
file bytes
   │
   ├─ hash content
   ▼
Blob object ID

ordered directory entries
   │
   ├─ canonical tree encoding
   ├─ hash encoded bytes
   ▼
Tree object ID

root tree + parents + metadata + provenance
   │
   ├─ canonical snapshot encoding
   ├─ hash encoded bytes
   ▼
Snapshot object ID
```

## Responsibilities

- Provide stable object identifiers for blobs, trees, and snapshots.
- Define valid tree entries and deterministic tree ordering.
- Define snapshot data: root tree, parents, timestamp, optional author, optional message, and structured provenance.
- Provide deterministic canonical bytes for tree and snapshot objects.
- Validate object-level invariants before data crosses component boundaries.

## Boundaries

Core does not:

- Store objects on disk.
- Read or write working-directory files.
- Manage branches, refs, or repository metadata.
- Decide when snapshots should be created.
- Render user-facing CLI output.

Those responsibilities belong to the object store, materialization, repository, and CLI components.

## Component relationships

```text
             ┌──────────────────────┐
             │       era-cli        │
             └──────────▲───────────┘
                        │
             ┌──────────┴───────────┐
             │   era-repository     │
             └──────────▲───────────┘
                        │
┌───────────────────────┼───────────────────────┐
│                       │                       │
│          ┌────────────┴────────────┐          │
│          │        era-core         │          │
│          │ ObjectId / Tree /       │          │
│          │ Snapshot / Provenance   │          │
│          └────────────▲────────────┘          │
│                       │                       │
└───────────▲───────────┴───────────▲───────────┘
            │                       │
┌───────────┴───────────┐ ┌─────────┴───────────┐
│ era-materialization   │ │ era-object-store    │
└───────────────────────┘ └─────────────────────┘
```

Every component uses the core domain model. Core remains intentionally small so those shared types stay easy to audit and preserve.

## v0 constraints

- Tree names are UTF-8 single path segments.
- Tree ordering is deterministic and based on exact entry names.
- Snapshots are immutable content-addressed records.
- Provenance is structured metadata on snapshots, not a free-form log convention.

## Future seams

Core can grow with additional object metadata, richer provenance fields, or compatibility encodings, but it should remain independent of storage layout, workspace state, and command behavior.
