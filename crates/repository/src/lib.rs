//! Repository orchestration for branches, snapshots, history, and policy.

mod branch;
mod error;
mod refs;
mod repository;

pub use branch::{BranchName, InvalidBranchName, InvalidBranchNameReason};
pub use error::RepositoryError;
pub use repository::{InitResult, Repository, SnapshotRequest, SnapshotResult, TimelineEntry};

#[cfg(test)]
mod tests;
