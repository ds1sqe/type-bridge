//! Error handling for the public TypeBridge client.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticDetailValue, DiagnosticPathSegment,
};
use type_bridge_orm::match_request::{MatchError, MatchErrorCategory};

/// Stable public classification for TypeBridge client failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCategory {
    /// Connection establishment or connectivity failed.
    Connection,
    /// Generated or installed schema authority failed verification.
    Schema,
    /// Generated input or provider evidence failed model validation.
    ModelValidation,
    /// A typed query was invalid before provider execution.
    QueryAuthoring,
    /// The provider failed while executing an accepted query.
    QueryExecution,
    /// A transaction lifecycle operation failed.
    Transaction,
    /// A remote envelope, reply, transport, or integrity contract failed.
    Remote,
    /// The selected provider or remote executor lacks a required capability.
    Capability,
    /// A canonical client, provider, or remote resource ceiling was exceeded.
    ResourceLimit,
    /// A requested entity or schema element was not found.
    NotFound,
    /// A generated-model lifecycle hook rejected or failed an operation.
    Lifecycle,
    /// An underlying database operation failed outside a narrower category.
    Database,
    /// A client invariant failed outside the stable categories above.
    Other,
}

impl ErrorCategory {
    /// Return the stable language-neutral category spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Schema => "schema",
            Self::ModelValidation => "model_validation",
            Self::QueryAuthoring => "query_authoring",
            Self::QueryExecution => "query_execution",
            Self::Transaction => "transaction",
            Self::Remote => "remote",
            Self::Capability => "capability",
            Self::ResourceLimit => "resource_limit",
            Self::NotFound => "not_found",
            Self::Lifecycle => "lifecycle",
            Self::Database => "database",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stage at which generated-model evidence failed validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelValidationPhase {
    /// Generated constructor input failed before provider execution.
    Input,
    /// Provider row evidence failed while hydrating a generated model.
    Hydration,
}

/// One typed value from a structured engine or remote diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorDetail {
    /// Textual context.
    Text(String),
    /// A signed integer.
    Long(i64),
    /// A boolean fact.
    Boolean(bool),
    /// An ordered list of text values.
    TextList(Vec<String>),
}

/// One typed segment from a structured engine or remote diagnostic path.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorPathSegment {
    /// A named field in the rejected contract.
    Field(String),
    /// An indexed member in the rejected contract.
    Index(u64),
    /// A schema or query identifier.
    Identifier(String),
}

/// Complete typed metadata supplied by one structured engine or remote
/// diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorDiagnostic {
    path: Vec<ErrorPathSegment>,
    details: BTreeMap<String, ErrorDetail>,
}

impl ErrorDiagnostic {
    /// Return the typed diagnostic path.
    #[must_use]
    pub fn path(&self) -> &[ErrorPathSegment] {
        &self.path
    }

    /// Return the deterministic typed detail map.
    #[must_use]
    pub fn details(&self) -> &BTreeMap<String, ErrorDetail> {
        &self.details
    }
}

/// Primary error type for the TypeBridge client SDK.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

    /// A structured engine or remote-contract failure mapped into stable
    /// client-owned categories, codes, and paths.
    #[error("{category} error [{code}]: {message}")]
    Classified {
        category: ErrorCategory,
        phase: Option<ModelValidationPhase>,
        code: String,
        path: Vec<String>,
        diagnostic: Option<Box<ErrorDiagnostic>>,
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

    pub(crate) fn classified(
        category: ErrorCategory,
        phase: Option<ModelValidationPhase>,
        code: impl Into<String>,
        path: Vec<String>,
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    ) -> Self {
        Self::classified_with_diagnostic(category, phase, code, path, None, message, source)
    }

    fn classified_with_diagnostic(
        category: ErrorCategory,
        phase: Option<ModelValidationPhase>,
        code: impl Into<String>,
        path: Vec<String>,
        diagnostic: Option<ErrorDiagnostic>,
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    ) -> Self {
        Self::Classified {
            category,
            phase,
            code: code.into(),
            path,
            diagnostic: diagnostic.map(Box::new),
            message: message.into(),
            source,
        }
    }

    /// Construct one application-owned remote transport failure.
    ///
    /// Transport implementations should use a stable lowercase snake-case
    /// code so callers can handle the failure without parsing its message.
    #[must_use]
    pub fn remote(
        code: impl Into<String>,
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    ) -> Self {
        Self::classified(
            ErrorCategory::Remote,
            None,
            code,
            Vec::new(),
            message,
            source,
        )
    }

    pub(crate) fn from_match(error: MatchError, phase: ModelValidationPhase) -> Self {
        let category = match error.category() {
            MatchErrorCategory::InvalidPlan => ErrorCategory::QueryAuthoring,
            MatchErrorCategory::Cardinality | MatchErrorCategory::ResultDecode => {
                ErrorCategory::ModelValidation
            }
            MatchErrorCategory::UnsupportedCapability => ErrorCategory::Capability,
            MatchErrorCategory::StaleSchema => ErrorCategory::Schema,
            MatchErrorCategory::ResourceLimit => ErrorCategory::ResourceLimit,
            MatchErrorCategory::Provider => ErrorCategory::QueryExecution,
        };
        let model_phase = (category == ErrorCategory::ModelValidation).then_some(phase);
        let code = error.code().as_str().to_owned();
        let path = error
            .path()
            .segments()
            .iter()
            .map(ToString::to_string)
            .collect();
        let message = error.message().to_owned();
        Self::classified(
            category,
            model_phase,
            code,
            path,
            message,
            Some(Box::new(error)),
        )
    }

    pub(crate) fn from_remote_diagnostic(error: Diagnostic) -> Self {
        let category = match error.category() {
            DiagnosticCategory::UnsupportedCapability => ErrorCategory::Capability,
            DiagnosticCategory::ResourceLimit => ErrorCategory::ResourceLimit,
            DiagnosticCategory::InvalidContract | DiagnosticCategory::Integrity => {
                ErrorCategory::Remote
            }
        };
        let code = error.code().as_str().to_owned();
        let path = error
            .path()
            .segments()
            .iter()
            .map(|segment| match segment {
                DiagnosticPathSegment::Field(value) => value.clone(),
                DiagnosticPathSegment::Index(value) => format!("[{value}]"),
                DiagnosticPathSegment::Identifier(value) => value.clone(),
            })
            .collect();
        let diagnostic_path = error
            .path()
            .segments()
            .iter()
            .map(|segment| match segment {
                DiagnosticPathSegment::Field(value) => ErrorPathSegment::Field(value.clone()),
                DiagnosticPathSegment::Index(value) => ErrorPathSegment::Index(*value),
                DiagnosticPathSegment::Identifier(value) => {
                    ErrorPathSegment::Identifier(value.clone())
                }
            })
            .collect();
        let details = error
            .details()
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    DiagnosticDetailValue::Text(value) => ErrorDetail::Text(value.clone()),
                    DiagnosticDetailValue::Long(value) => ErrorDetail::Long(*value),
                    DiagnosticDetailValue::Boolean(value) => ErrorDetail::Boolean(*value),
                    DiagnosticDetailValue::TextList(value) => ErrorDetail::TextList(value.clone()),
                };
                (key.clone(), value)
            })
            .collect();
        let message = error.message().to_owned();
        Self::classified_with_diagnostic(
            category,
            None,
            code,
            path,
            Some(ErrorDiagnostic {
                path: diagnostic_path,
                details,
            }),
            message,
            Some(Box::new(error)),
        )
    }

    pub(crate) fn from_hook(error: crate::hooks::HookError) -> Self {
        let code = match error {
            crate::hooks::HookError::Rejected { .. } => "lifecycle_hook_rejected",
            crate::hooks::HookError::Internal { .. } => "lifecycle_hook_failed",
        };
        Self::classified(
            ErrorCategory::Lifecycle,
            None,
            code,
            Vec::new(),
            error.to_string(),
            Some(Box::new(error)),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn from_orm(err: type_bridge_orm::OrmError) -> Self {
        match err {
            type_bridge_orm::OrmError::Match(error) => {
                Self::from_match(error, ModelValidationPhase::Input)
            }
            error @ type_bridge_orm::OrmError::Connection(_) => Self::Connection {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
            error @ type_bridge_orm::OrmError::QueryExecution(_) => Self::QueryExecution {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
            error @ type_bridge_orm::OrmError::Transaction(_) => Self::Transaction {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
            error @ type_bridge_orm::OrmError::NotFound(_) => Self::NotFound {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
            error @ type_bridge_orm::OrmError::Hydration { .. } => Self::ModelValidation {
                phase: ModelValidationPhase::Hydration,
                code: "invalid_provider_evidence".into(),
                path: vec![],
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
            error => Self::Database {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
        }
    }

    pub(crate) fn from_orm_hydration(err: type_bridge_orm::OrmError) -> Self {
        match err {
            type_bridge_orm::OrmError::Match(error) => {
                Self::from_match(error, ModelValidationPhase::Hydration)
            }
            error => Self::from_orm(error),
        }
    }

    /// Return the stable public failure category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::ModelValidation { .. } => ErrorCategory::ModelValidation,
            Self::Classified { category, .. } => *category,
            Self::SchemaVerification { .. } => ErrorCategory::Schema,
            Self::Connection { .. } => ErrorCategory::Connection,
            Self::QueryExecution { .. } => ErrorCategory::QueryExecution,
            Self::Transaction { .. } => ErrorCategory::Transaction,
            Self::NotFound { .. } => ErrorCategory::NotFound,
            Self::Database { .. } => ErrorCategory::Database,
            Self::Other { .. } => ErrorCategory::Other,
        }
    }

    /// Return the error message string.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::ModelValidation { message, .. }
            | Self::Classified { message, .. }
            | Self::SchemaVerification { message, .. }
            | Self::Connection { message, .. }
            | Self::QueryExecution { message, .. }
            | Self::Transaction { message, .. }
            | Self::NotFound { message, .. }
            | Self::Database { message, .. }
            | Self::Other { message, .. } => message,
        }
    }

    /// Return the stable machine-readable failure code, when available.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::ModelValidation { code, .. } | Self::Classified { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Return the owned structured diagnostic path, when available.
    #[must_use]
    pub fn path(&self) -> Option<&[String]> {
        match self {
            Self::ModelValidation { path, .. } | Self::Classified { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Return the typed diagnostic path, when the source supplied one.
    ///
    /// [`Self::path`] remains available as the compatibility-oriented textual
    /// projection of the same path.
    #[must_use]
    pub fn diagnostic_path(&self) -> Option<&[ErrorPathSegment]> {
        match self {
            Self::Classified { diagnostic, .. } => diagnostic.as_deref().map(ErrorDiagnostic::path),
            _ => None,
        }
    }

    /// Return deterministic typed diagnostic details, when available.
    #[must_use]
    pub fn details(&self) -> Option<&BTreeMap<String, ErrorDetail>> {
        match self {
            Self::Classified { diagnostic, .. } => {
                diagnostic.as_deref().map(ErrorDiagnostic::details)
            }
            _ => None,
        }
    }

    /// Return the model-validation phase, when applicable.
    #[must_use]
    pub const fn model_validation_phase(&self) -> Option<ModelValidationPhase> {
        match self {
            Self::ModelValidation { phase, .. } => Some(*phase),
            Self::Classified { phase, .. } => *phase,
            _ => None,
        }
    }
}

/// Convenience Result type for the TypeBridge client.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::{Error, ErrorCategory};

    #[test]
    fn public_error_categories_and_remote_constructor_are_stable() {
        let categories = [
            (ErrorCategory::Connection, "connection"),
            (ErrorCategory::Schema, "schema"),
            (ErrorCategory::ModelValidation, "model_validation"),
            (ErrorCategory::QueryAuthoring, "query_authoring"),
            (ErrorCategory::QueryExecution, "query_execution"),
            (ErrorCategory::Transaction, "transaction"),
            (ErrorCategory::Remote, "remote"),
            (ErrorCategory::Capability, "capability"),
            (ErrorCategory::ResourceLimit, "resource_limit"),
            (ErrorCategory::NotFound, "not_found"),
            (ErrorCategory::Lifecycle, "lifecycle"),
            (ErrorCategory::Database, "database"),
            (ErrorCategory::Other, "other"),
        ];
        for (category, spelling) in categories {
            assert_eq!(category.as_str(), spelling);
            assert_eq!(category.to_string(), spelling);
        }

        let error = Error::remote("remote_transport", "connection reset", None);
        assert_eq!(error.category(), ErrorCategory::Remote);
        assert_eq!(error.code(), Some("remote_transport"));
        assert_eq!(error.path(), Some(&[][..]));
        assert_eq!(error.message(), "connection reset");
    }
}
