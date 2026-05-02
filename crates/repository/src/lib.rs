//! Repository orchestration for branches, snapshots, history, and policy.

/// A named mutable pointer to a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Creates a branch name.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the branch name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_exposes_inner_value() {
        let branch = BranchName::new("main");

        assert_eq!(branch.as_str(), "main");
    }
}
