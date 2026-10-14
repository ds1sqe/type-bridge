use crate::error::PipelineError;

/// Whether TypeDB rejected a commit or its outcome could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitCertainty {
    /// TypeDB rejected the commit; this transaction stored no changes.
    Rejected,
    /// The commit may have succeeded; retrying can duplicate changes.
    Indeterminate,
}

impl std::fmt::Display for CommitCertainty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rejected => "rejected",
            Self::Indeterminate => "indeterminate",
        })
    }
}

/// A failed driver commit, retaining its certainty and original driver text.
#[derive(Debug, thiserror::Error)]
#[error("Commit {certainty}: {message}")]
#[non_exhaustive]
pub struct CommitFailure {
    /// Whether the transaction is known to have been rejected.
    pub certainty: CommitCertainty,
    /// Original driver diagnostic, without additional prefixes.
    pub message: String,
}

/// Error returned by [`super::TypeDBClient::execute_classified`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExecuteError {
    /// Configuration, transaction-open, or query execution failure.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// Driver commit failure with its outcome certainty.
    #[error(transparent)]
    Commit(#[from] CommitFailure),
}

impl From<ExecuteError> for PipelineError {
    fn from(error: ExecuteError) -> Self {
        match error {
            ExecuteError::Pipeline(error) => error,
            ExecuteError::Commit(error) => {
                Self::QueryExecution(format!("Commit failed: {}", error.message))
            }
        }
    }
}
