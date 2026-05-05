//! Working-directory materialization and filesystem observation.

mod capture;
mod error;
mod filesystem;
mod hash_cache;
mod watch;
mod working_directory;

pub use capture::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats, MaterializeResult,
    MaterializeStats, Materializer, SymlinkPolicy, TreeChange, TreeChangeKind,
    TreeComparisonResult, TreeScanResult, TreeScanStats,
};
pub use error::MaterializationError;
pub use filesystem::FilesystemMaterializer;
pub use watch::{WatchEvent, WorkingDirectoryWatch};
pub use working_directory::WorkingDirectory;

#[cfg(test)]
mod tests;
