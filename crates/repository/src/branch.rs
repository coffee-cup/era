use std::{error::Error, fmt};

/// A named mutable pointer to a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Creates a branch name after validating it is safe to use as a ref file name.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidBranchName> {
        let value = value.into();
        validate_branch_name(&value)?;
        Ok(Self(value))
    }

    /// Returns the default branch name.
    pub fn main() -> Self {
        Self("main".to_owned())
    }

    /// Returns the branch name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Error returned for invalid branch names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidBranchName {
    name: String,
    reason: InvalidBranchNameReason,
}

impl InvalidBranchName {
    fn new(name: String, reason: InvalidBranchNameReason) -> Self {
        Self { name, reason }
    }

    /// Returns the invalid branch name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns why the branch name is invalid.
    pub fn reason(&self) -> InvalidBranchNameReason {
        self.reason
    }
}

impl fmt::Display for InvalidBranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid branch name {:?}: {}",
            self.name, self.reason
        )
    }
}

impl Error for InvalidBranchName {}

/// Reason a branch name is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidBranchNameReason {
    /// Branch names cannot be empty.
    Empty,
    /// `.` is not a valid branch name.
    CurrentDirectory,
    /// `..` is not a valid branch name.
    ParentDirectory,
    /// Slash is rejected in v0 so branch names map to one ref-file segment.
    ContainsSlash,
    /// Backslash is rejected so ref names are portable across platforms.
    ContainsBackslash,
    /// Branch names cannot contain NUL.
    ContainsNul,
}

impl fmt::Display for InvalidBranchNameReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("branch name is empty"),
            Self::CurrentDirectory => formatter.write_str("branch name cannot be '.'"),
            Self::ParentDirectory => formatter.write_str("branch name cannot be '..'"),
            Self::ContainsSlash => formatter.write_str("branch name contains '/'"),
            Self::ContainsBackslash => formatter.write_str("branch name contains '\\'"),
            Self::ContainsNul => formatter.write_str("branch name contains NUL"),
        }
    }
}

fn validate_branch_name(name: &str) -> Result<(), InvalidBranchName> {
    let reason = if name.is_empty() {
        Some(InvalidBranchNameReason::Empty)
    } else if name == "." {
        Some(InvalidBranchNameReason::CurrentDirectory)
    } else if name == ".." {
        Some(InvalidBranchNameReason::ParentDirectory)
    } else if name.contains('/') {
        Some(InvalidBranchNameReason::ContainsSlash)
    } else if name.contains('\\') {
        Some(InvalidBranchNameReason::ContainsBackslash)
    } else if name.contains('\0') {
        Some(InvalidBranchNameReason::ContainsNul)
    } else {
        None
    };

    match reason {
        Some(reason) => Err(InvalidBranchName::new(name.to_owned(), reason)),
        None => Ok(()),
    }
}
