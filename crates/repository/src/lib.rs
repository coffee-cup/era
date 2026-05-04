//! Repository orchestration for branches, snapshots, history, and policy.

mod branch;
mod error;
mod refs;
mod repository;

pub use branch::{BranchName, InvalidBranchName, InvalidBranchNameReason};
pub use error::RepositoryError;
pub use repository::{
    BranchHead, BranchOperationResult, InitResult, Repository, ResolvedSnapshot, RestoreResult,
    SnapshotRequest, SnapshotResult, SwitchResult, TimelineEntry, WorkingTreeStatus,
};

#[cfg(test)]
mod tests;
