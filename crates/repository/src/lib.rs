//! Repository orchestration for branches, snapshots, history, and policy.

mod branch;
mod error;
mod refs;
mod repository;

pub use branch::{BranchName, InvalidBranchName, InvalidBranchNameReason};
pub use era_materialization::{TreeChange, TreeChangeKind};
pub use error::RepositoryError;
pub use repository::{
    AutoSnapshotTrigger, BranchHead, BranchOperationResult, DEFAULT_WORKSPACE_ID, InitResult,
    Repository, ResolvedSnapshot, RestoreResult, SnapshotRequest, SnapshotResult, SwitchResult,
    TimelineEntry, WorkingTreeStatus,
};

#[cfg(test)]
mod tests;
