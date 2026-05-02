//! Working-directory materialization and filesystem observation.

mod capture;
mod error;
mod filesystem;
mod working_directory;

pub use capture::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats, Materializer,
    SymlinkPolicy,
};
pub use error::MaterializationError;
pub use filesystem::FilesystemMaterializer;
pub use working_directory::WorkingDirectory;

#[cfg(test)]
mod tests;
