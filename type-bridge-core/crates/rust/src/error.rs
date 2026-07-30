//! Error handling for the public TypeBridge client.

use std::error::Error as StdError;

/// Stage at which generated-model evidence failed validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelValidationPhase {
    /// Generated constructor input failed before provider execution.
    Input,
    /// Provider row evidence failed while hydrating a generated model.
    Hydration,
}

/// Primary error type for the TypeBridge client SDK.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Generated-model evidence did not match the installed schema projection.
    #[error("Model validation failed during {phase:?}: {message}")]
    ModelValidation {
        phase: ModelValidationPhase,
        code: String,
        path: Vec<String>,
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Schema verification or installation failed.
    #[error("Schema verification failed: {message}")]
    SchemaVerification {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Connection to the database failed.
    #[error("Connection error: {message}")]
    Connection {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Database query or operation failed.
    #[error("Query execution error: {message}")]
    QueryExecution {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Database transaction failed.
    #[error("Transaction error: {message}")]
    Transaction {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Requested schema element or database entity was not found.
    #[error("Entity not found: {message}")]
    NotFound {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Underlying database error.
    #[error("Database error: {message}")]
    Database {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Client request or execution error.
    #[error("Client error: {message}")]
    Other {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },
}

impl Error {
    #[allow(dead_code)]
    pub(crate) fn model_validation(
        phase: ModelValidationPhase,
        code: impl Into<String>,
        path: Vec<String>,
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    ) -> Self {
        Self::ModelValidation {
            phase,
            code: code.into(),
            path,
            message: message.into(),
            source,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn from_orm(err: type_bridge_orm::OrmError) -> Self {
        let message = err.to_string();
        match &err {
            type_bridge_orm::OrmError::Connection(_) => Self::Connection {
                message,
                source: Some(Box::new(err)),
            },
            type_bridge_orm::OrmError::QueryExecution(_) => Self::QueryExecution {
                message,
                source: Some(Box::new(err)),
            },
            type_bridge_orm::OrmError::Transaction(_) => Self::Transaction {
                message,
                source: Some(Box::new(err)),
            },
            type_bridge_orm::OrmError::NotFound(_) => Self::NotFound {
                message,
                source: Some(Box::new(err)),
            },
            type_bridge_orm::OrmError::Hydration { .. } => Self::ModelValidation {
                phase: ModelValidationPhase::Hydration,
                code: "invalid_provider_evidence".into(),
                path: vec![],
                message,
                source: Some(Box::new(err)),
            },
            _ => Self::Database {
                message,
                source: Some(Box::new(err)),
            },
        }
    }

    /// Return the error message string.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::ModelValidation { message, .. }
            | Self::SchemaVerification { message, .. }
            | Self::Connection { message, .. }
            | Self::QueryExecution { message, .. }
            | Self::Transaction { message, .. }
            | Self::NotFound { message, .. }
            | Self::Database { message, .. }
            | Self::Other { message, .. } => message,
        }
    }

    /// Return the stable model-validation code, when applicable.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::ModelValidation { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Return the owned structured model-validation path, when applicable.
    #[must_use]
    pub fn path(&self) -> Option<&[String]> {
        match self {
            Self::ModelValidation { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Return the model-validation phase, when applicable.
    #[must_use]
    pub const fn model_validation_phase(&self) -> Option<ModelValidationPhase> {
        match self {
            Self::ModelValidation { phase, .. } => Some(*phase),
            _ => None,
        }
    }
}

/// Convenience Result type for the TypeBridge client.
pub type Result<T, E = Error> = std::result::Result<T, E>;
