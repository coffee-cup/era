//! Shared domain types and primitives for Era.

mod object_id;
mod object_kind;
mod snapshot;
mod tree;

pub use object_id::{OBJECT_ID_BYTES, OBJECT_ID_HEX_LENGTH, ObjectId, ParseObjectIdError};
pub use object_kind::ObjectKind;
pub use snapshot::{Snapshot, SnapshotError, SnapshotProvenance};
pub use tree::{EntryKind, InvalidTreeEntryName, Tree, TreeEntry, TreeError};
