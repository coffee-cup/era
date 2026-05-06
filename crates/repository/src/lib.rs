//! Repository orchestration for branches, snapshots, history, and policy.

mod branch;
mod error;
mod refs;
mod repository;
mod workspace;

pub use branch::{BranchName, InvalidBranchName, InvalidBranchNameReason};
pub use era_materialization::{TreeChange, TreeChangeKind};
pub use error::RepositoryError;
pub use repository::{
    AddWorkspaceOptions, AutoSnapshotTrigger, BranchHead, BranchOperationResult, CursorInfo,
    InitResult, Repository, ResolvedSnapshot, RestoreResult, SnapshotGraph, SnapshotRequest,
    SnapshotResult, SwitchResult, TimelineEntry, WorkingTreeStatus, WorkspaceAddResult,
    WorkspaceHead,
};
pub use workspace::{
    DEFAULT_WORKSPACE_ID, InvalidWorkspaceId, InvalidWorkspaceIdReason, WorkspaceId,
};

#[cfg(test)]
mod tests;
