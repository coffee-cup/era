# Object Store Component

`era-object-store` is the content-addressed persistence layer. It stores immutable blob, tree, and snapshot objects by their core object IDs and verifies that retrieved bytes still match the ID that names them.

The object store is deliberately unaware of branches, working directories, labels, and snapshot policy. It answers one question: given an immutable object, can Era store it, retrieve it, deduplicate it, and detect corruption?

## High-level structure

```text
┌──────────────────────────────────────────────┐
│ era-object-store                             │
├──────────────────────────────────────────────┤
│ ObjectStore capability                       │
│  - put/get blobs                             │
│  - put/get trees                             │
│  - put/get snapshots                         │
│  - contains checks                           │
├──────────────────────────────────────────────┤
│ Local object store implementation            │
│  - sharded content-addressed layout          │
│  - race-safe writes                          │
│  - transparent deduplication                 │
│  - hash verification on read                 │
└──────────────────────────────────────────────┘
```

## Storage flow

```text
caller supplies object data
   │
   ├─ blob bytes, or
   ├─ tree domain object, or
   └─ snapshot domain object
   │
   ▼
canonical bytes where needed
   │
   ▼
content hash / expected object ID
   │
   ▼
local content-addressed storage
   │
   ├─ object already exists ──► reuse existing bytes
   │
   └─ object is new ─────────► write immutable bytes
   │
   ▼
return object ID
```

## Read and verification flow

```text
requested object ID
   │
   ▼
load stored bytes
   │
   ▼
verify content hash matches requested ID
   │
   ├─ mismatch ──► surface corruption
   │
   ▼
parse domain object when reading trees/snapshots
   │
   ▼
return verified object
```

## Responsibilities

- Persist blobs, trees, and snapshots as immutable content-addressed objects.
- Deduplicate identical object bytes.
- Verify object integrity on reads.
- Preserve deterministic tree and snapshot encodings provided by the core model.
- Expose an async capability interface for higher layers.

## Boundaries

The object store does not:

- Know which snapshot is current.
- Manage branch names, refs, or HEAD state.
- Walk, watch, or restore working directories.
- Decide whether a snapshot is meaningful or automatic.
- Apply tracking or ignore policy.

Those decisions are made above this layer.

## Component relationships

```text
┌───────────────────────┐
│ era-repository        │
│ refs and policies     │
└───────────┬───────────┘
            │ stores snapshots
            ▼
┌───────────────────────┐
│ era-object-store      │
│ immutable objects     │
└───────────▲───────────┘
            │ stores blobs/trees during capture
┌───────────┴───────────┐
│ era-materialization   │
│ working tree bridge   │
└───────────────────────┘
```

Materialization writes blobs and trees while capturing a working directory. Repository writes snapshots and reads historical objects while serving branch, status, timeline, switch, and restore operations.

## v0 constraints

- The implemented store is local filesystem-backed.
- Objects are grouped by kind and sharded by object ID prefix.
- Blobs are whole-file objects; there is no block-level chunking or delta compression.
- Garbage collection, network sync, encryption, and remote storage are outside v0.

## Future seams

The object-store capability can grow additional implementations for remote stores, encrypted stores, or sync-aware stores. Those additions should preserve the same boundary: immutable objects go in and verified immutable objects come out.
