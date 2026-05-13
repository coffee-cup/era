//! Content-addressed object storage abstractions.

mod blob_delta;
mod error;
mod local;

use async_trait::async_trait;
use era_core::{ObjectId, ObjectKind, Snapshot, Tree};

pub use error::ObjectStoreError;
pub use local::LocalObjectStore;

/// Async object storage interface used by higher layers.
///
/// The local implementation stores objects on disk, but callers depend on this
/// capability rather than on a concrete filesystem layout. Future object stores
/// can back the same interface with remote storage or another database.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Stores blob bytes and returns their content-addressed object ID.
    async fn put_blob(&self, bytes: &[u8]) -> Result<ObjectId, ObjectStoreError>;

    /// Stores blob bytes, optionally using another stored blob as a physical delta base.
    async fn put_blob_with_base(
        &self,
        bytes: &[u8],
        base_id: Option<&ObjectId>,
    ) -> Result<ObjectId, ObjectStoreError> {
        let _ = base_id;
        self.put_blob(bytes).await
    }

    /// Reads blob bytes by object ID and verifies their integrity.
    async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError>;

    /// Stores a tree and returns the ID of its canonical serialized bytes.
    async fn put_tree(&self, tree: &Tree) -> Result<ObjectId, ObjectStoreError>;

    /// Reads a tree by object ID and verifies its integrity and canonical form.
    async fn get_tree(&self, id: &ObjectId) -> Result<Tree, ObjectStoreError>;

    /// Stores a snapshot and returns the ID of its canonical serialized bytes.
    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<ObjectId, ObjectStoreError>;

    /// Reads a snapshot by object ID and verifies its integrity and canonical form.
    async fn get_snapshot(&self, id: &ObjectId) -> Result<Snapshot, ObjectStoreError>;

    /// Lists snapshot object IDs present in this store without decoding them.
    async fn list_snapshot_ids(&self) -> Result<Vec<ObjectId>, ObjectStoreError>;

    /// Returns `true` when a valid object exists in the store.
    async fn contains(&self, kind: ObjectKind, id: &ObjectId) -> Result<bool, ObjectStoreError>;
}

#[cfg(test)]
mod tests;
