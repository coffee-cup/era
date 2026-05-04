//! Working-directory materialization and filesystem observation.

mod capture;
mod error;
mod filesystem;
mod working_directory;

pub use capture::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats, MaterializeResult,
    MaterializeStats, Materializer, SymlinkPolicy, TreeScanResult, TreeScanStats,
};
pub use error::MaterializationError;
pub use filesystem::FilesystemMaterializer;
pub use working_directory::WorkingDirectory;

#[cfg(test)]
mod tests;
