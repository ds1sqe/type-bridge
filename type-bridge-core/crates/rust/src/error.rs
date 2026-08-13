//! Error handling for the public TypeBridge client.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticPathSegment,
    SdkExecutionDiagnostic, SdkProjectionEvidenceSlotPresence, SdkQueryDiagnosticCategory,
    SdkQueryDiagnosticPathKind,
};
use type_bridge_orm::match_request::MatchError;
use type_bridge_orm::{
    ProjectedCrudCompatibilityCause, ProjectedCrudCompatibilityFailure,
    ProjectedCrudCompatibilityStage,
};

use crate::hooks::{CrudOperation, ModelKind};

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
    /// Generated package or provider evidence failed an integrity contract.
    Integrity,
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
    /// Cooperative cancellation interrupted the operation.
    Cancelled,
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
            Self::Integrity => "integrity",
            Self::QueryAuthoring => "query_authoring",
            Self::QueryExecution => "query_execution",
            Self::Transaction => "transaction",
            Self::Remote => "remote",
            Self::Capability => "capability",
            Self::ResourceLimit => "resource_limit",
            Self::Cancelled => "cancelled",
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
    /// The exact canonical typed-query category.
    QueryCategory(QueryDiagnosticCategory),
    /// One bounded canonical query or contract identity.
    QueryIdentity(String),
    /// An ordered bounded list of canonical query or contract identities.
    QueryIdentityList(Vec<String>),
}

/// Stable public typed-query diagnostic categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum QueryDiagnosticCategory {
    /// The immutable query plan is invalid.
    InvalidPlan,
    /// A terminal cardinality contract was not satisfied.
    Cardinality,
    /// The provider lacks a required query capability.
    UnsupportedCapability,
    /// Request-relevant schema authority changed.
    StaleSchema,
    /// A query resource ceiling was crossed.
    ResourceLimit,
    /// Cooperative cancellation interrupted execution.
    Cancelled,
    /// The provider failed before complete evidence was available.
    Provider,
    /// Result evidence did not match the validated invocation.
    ResultDecode,
}

/// Stable structural locations within a typed query request or result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum QueryDiagnosticPathKind {
    /// The request envelope.
    Request,
    /// The graph plan.
    Plan,
    /// The selected terminal operation.
    Operation,
    /// The predicate tree.
    Predicate,
    /// The declared output shape.
    Output,
    /// Provider solution or hydration evidence.
    ProviderEvidence,
    /// The validated result envelope.
    Result,
}

/// One typed segment from a structured engine or remote diagnostic path.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorPathSegment {
    /// A named public operation argument.
    Argument(String),
    /// A named field in the rejected contract.
    Field(String),
    /// An indexed member in the rejected contract.
    Index(u64),
    /// A schema or query identifier.
    Identifier(String),
    /// A structural typed-query request or result location.
    Query(QueryDiagnosticPathKind),
    /// A plan-local typed-query binding ordinal.
    QueryBinding(u16),
    /// A descriptor-qualified binding-facing query field.
    QueryField {
        /// Kind-qualified registry descriptor identity.
        owner: String,
        /// Binding-facing field name.
        name: String,
    },
    /// A descriptor-qualified query role.
    QueryRole {
        /// Kind-qualified registry descriptor identity.
        owner: String,
        /// Binding-facing role name.
        name: String,
    },
    /// A plan-local typed-query role-edge ordinal.
    QueryRoleEdge(u16),
    /// A zero-based positional query output slot.
    QueryOutputSlot(u64),
    /// A declared generated query output member.
    QueryOutputName(String),
    /// An object field retained from an authenticated contract diagnostic.
    ContractField(String),
    /// An identity retained from an authenticated contract diagnostic.
    ContractIdentity(String),
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
        /// Validation stage at which the evidence failed.
        phase: ModelValidationPhase,
        /// Stable language-neutral failure code.
        code: String,
        /// Canonical path to the rejected value or model evidence.
        path: Vec<String>,
        /// Human-readable failure summary.
        message: String,
        /// Optional underlying error that caused the validation failure.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// A structured engine or remote-contract failure mapped into stable
    /// client-owned categories, codes, and paths.
    #[error("{category} error [{code}]: {message}")]
    Classified {
        /// Stable public error category.
        category: ErrorCategory,
        /// Model-validation phase when applicable to this failure.
        phase: Option<ModelValidationPhase>,
        /// Stable language-neutral failure code.
        code: String,
        /// Canonical flattened path to the rejected contract member.
        path: Vec<String>,
        /// Typed engine or remote diagnostic metadata, when supplied.
        diagnostic: Option<Box<ErrorDiagnostic>>,
        /// Human-readable failure summary.
        message: String,
        /// Optional underlying error that caused the classified failure.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Schema verification or installation failed.
    #[error("Schema verification failed: {message}")]
    SchemaVerification {
        /// Human-readable schema verification failure summary.
        message: String,
        /// Optional underlying schema decoding or installation error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Connection to the database failed.
    #[error("Connection error: {message}")]
    Connection {
        /// Human-readable connection failure summary.
        message: String,
        /// Optional underlying transport or provider error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Database query or operation failed.
    #[error("Query execution error: {message}")]
    QueryExecution {
        /// Human-readable query execution failure summary.
        message: String,
        /// Optional underlying provider or remote execution error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Database transaction failed.
    #[error("Transaction error: {message}")]
    Transaction {
        /// Human-readable transaction failure summary.
        message: String,
        /// Optional underlying provider transaction error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Requested schema element or database entity was not found.
    #[error("Entity not found: {message}")]
    NotFound {
        /// Human-readable description of the missing resource.
        message: String,
        /// Optional underlying lookup error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Underlying database error.
    #[error("Database error: {message}")]
    Database {
        /// Human-readable database failure summary.
        message: String,
        /// Optional underlying database driver error.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    },

    /// Client request or execution error.
    #[error("Client error: {message}")]
    Other {
        /// Human-readable client failure summary.
        message: String,
        /// Optional underlying error outside the narrower variants.
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

    pub(crate) fn projection_evidence_rejection(
        presence: SdkProjectionEvidenceSlotPresence,
    ) -> Self {
        let error = SdkExecutionDiagnostic::classify_detached_semantic_schema_fingerprint_rejection(
            presence,
        );
        let code = error.code().as_str().to_owned();
        let message = error.message().as_str().to_owned();
        let path = error.path().iter().map(flatten_sdk_path).collect();
        let diagnostic = ErrorDiagnostic {
            path: error
                .path()
                .iter()
                .map(|segment| match segment {
                    SdkDiagnosticPathSegment::Argument(value) => {
                        ErrorPathSegment::Argument(value.as_str().to_owned())
                    }
                    other => typed_sdk_path(other),
                })
                .collect(),
            details: error
                .details()
                .iter()
                .map(|(name, value)| (name.as_str().to_owned(), flatten_sdk_detail(value)))
                .collect(),
        };
        Self::classified_with_diagnostic(
            ErrorCategory::Integrity,
            None,
            code,
            path,
            Some(diagnostic),
            message,
            Some(Box::new(error)),
        )
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
        Self::from_sdk_execution(type_bridge_orm::lower_match_error(&error), phase)
    }

    pub(crate) fn from_sdk_execution(
        error: SdkExecutionDiagnostic,
        model_phase: ModelValidationPhase,
    ) -> Self {
        let code = error.code().as_str().to_owned();
        let message = error.message().as_str().to_owned();
        let path = error.path().iter().map(flatten_sdk_path).collect();
        let diagnostic_path = error.path().iter().map(typed_sdk_path).collect();
        let details = error
            .details()
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), flatten_sdk_detail(value)))
            .collect();
        let diagnostic = ErrorDiagnostic {
            path: diagnostic_path,
            details,
        };
        if let Some(query_category) = sdk_query_category(&error) {
            let (category, phase) = match query_category {
                SdkQueryDiagnosticCategory::InvalidPlan => (ErrorCategory::QueryAuthoring, None),
                SdkQueryDiagnosticCategory::Cardinality
                | SdkQueryDiagnosticCategory::ResultDecode => {
                    (ErrorCategory::ModelValidation, Some(model_phase))
                }
                SdkQueryDiagnosticCategory::UnsupportedCapability => {
                    (ErrorCategory::Capability, None)
                }
                SdkQueryDiagnosticCategory::StaleSchema => (ErrorCategory::Schema, None),
                SdkQueryDiagnosticCategory::ResourceLimit => (ErrorCategory::ResourceLimit, None),
                SdkQueryDiagnosticCategory::Cancelled => (ErrorCategory::Cancelled, None),
                SdkQueryDiagnosticCategory::Provider => (ErrorCategory::QueryExecution, None),
                _ => (ErrorCategory::Other, None),
            };
            return Self::classified_with_diagnostic(
                category,
                phase,
                code,
                path,
                Some(diagnostic),
                message,
                Some(Box::new(error)),
            );
        }
        match error.category() {
            SdkDiagnosticCategory::InvalidInput | SdkDiagnosticCategory::Integrity => {
                Self::ModelValidation {
                    phase: model_phase,
                    code,
                    path,
                    message,
                    source: Some(Box::new(error)),
                }
            }
            SdkDiagnosticCategory::Provider => Self::QueryExecution {
                message,
                source: Some(Box::new(error)),
            },
            SdkDiagnosticCategory::Transaction => Self::Transaction {
                message,
                source: Some(Box::new(error)),
            },
            SdkDiagnosticCategory::UnsupportedCapability => Self::classified_with_diagnostic(
                ErrorCategory::Capability,
                None,
                code,
                path,
                Some(diagnostic),
                message,
                Some(Box::new(error)),
            ),
            SdkDiagnosticCategory::ResourceLimit => Self::classified_with_diagnostic(
                ErrorCategory::ResourceLimit,
                None,
                code,
                path,
                Some(diagnostic),
                message,
                Some(Box::new(error)),
            ),
            SdkDiagnosticCategory::Cancelled => Self::classified_with_diagnostic(
                ErrorCategory::Cancelled,
                None,
                code,
                path,
                Some(diagnostic),
                message,
                Some(Box::new(error)),
            ),
            SdkDiagnosticCategory::Internal => Self::classified_with_diagnostic(
                ErrorCategory::Other,
                None,
                code,
                path,
                Some(diagnostic),
                message,
                Some(Box::new(error)),
            ),
            _ => Self::Other {
                message,
                source: Some(Box::new(error)),
            },
        }
    }

    pub(crate) fn from_projected_crud(
        error: ProjectedCrudCompatibilityFailure,
        kind: ModelKind,
        operation: Option<CrudOperation>,
    ) -> Self {
        let (diagnostic, stage, cause) = error.into_parts();
        let code = diagnostic.code().as_str();

        if code == "mutation_rehydration_missing" {
            let message = match (kind, operation) {
                (ModelKind::Entity, Some(CrudOperation::Update)) => {
                    "updated entity was not returned"
                }
                (ModelKind::Entity, _) => "written entity was not returned",
                (ModelKind::Relation, _) => "written relation was not returned",
            };
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "missing_post_write_row",
                vec!["iid".into()],
                message,
                None,
            );
        }
        if code == "relation_hydration_ambiguous" {
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "ambiguous_provider_row",
                vec!["iid".into()],
                "provider returned multiple coalesced rows for one exact IID",
                None,
            );
        }
        if code == "hydrated_type_mismatch" {
            let message = match kind {
                ModelKind::Entity => "provider entity row has the wrong exact concrete type",
                ModelKind::Relation => "provider relation row has the wrong exact concrete type",
            };
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_concrete_type",
                vec!["type".into()],
                message,
                None,
            );
        }
        if code == "hydrated_iid_missing" {
            let message = match kind {
                ModelKind::Entity => "provider entity row omitted its IID",
                ModelKind::Relation => "provider relation row omitted its IID",
            };
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "missing_iid",
                vec!["iid".into()],
                message,
                None,
            );
        }
        if code == "hydrated_iid_mismatch" {
            let message = match kind {
                ModelKind::Entity => "provider entity row contains a noncanonical IID",
                ModelKind::Relation => "provider relation row contains a noncanonical IID",
            };
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "noncanonical_iid",
                vec!["iid".into()],
                message,
                None,
            );
        }
        if kind == ModelKind::Relation
            && code == "noncanonical_iid"
            && diagnostic
                .path()
                .iter()
                .any(|segment| matches!(segment, SdkDiagnosticPathSegment::Role(_)))
        {
            return Self::model_validation(
                ModelValidationPhase::Input,
                "noncanonical_player_iid",
                projected_role_path(&diagnostic, "iid"),
                "relation reference IID must be canonical",
                None,
            );
        }
        if kind == ModelKind::Relation
            && diagnostic
                .path()
                .iter()
                .any(|segment| matches!(segment, SdkDiagnosticPathSegment::Role(_)))
            && matches!(
                cause.as_ref(),
                Some(ProjectedCrudCompatibilityCause::Orm(
                    type_bridge_orm::OrmError::Hydration { .. }
                ))
            )
        {
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "invalid_player_attributes",
                projected_role_path(&diagnostic, "attributes"),
                "provider role player attributes are outside the projected descriptor",
                cause.and_then(projected_cause_source),
            );
        }
        if kind == ModelKind::Relation
            && code == "runtime_projection_mismatch"
            && diagnostic
                .path()
                .iter()
                .any(|segment| matches!(segment, SdkDiagnosticPathSegment::Role(_)))
        {
            return Self::model_validation(
                ModelValidationPhase::Hydration,
                "invalid_player_attributes",
                projected_role_path(&diagnostic, "attributes"),
                "provider role player attributes are outside the projected descriptor",
                cause.and_then(projected_cause_source),
            );
        }
        if kind == ModelKind::Relation {
            let mapped = match code {
                "hydrated_player_iid_missing" => Some((
                    "missing_player_iid",
                    "iid",
                    "provider role player omitted its IID",
                )),
                "hydrated_player_iid_invalid" => Some((
                    "noncanonical_player_iid",
                    "iid",
                    "provider role player contains a noncanonical IID",
                )),
                "hydrated_player_type_missing" => Some((
                    "missing_player_type",
                    "type",
                    "provider role player omitted its concrete type",
                )),
                "hydrated_role_player_not_accepted" => Some((
                    "player_not_allowed",
                    "type",
                    "provider role player type is outside the role",
                )),
                "projected_player_type_ambiguous" => Some((
                    "invalid_installed_projection",
                    "type",
                    "projected player authority is ambiguous",
                )),
                _ => None,
            };
            if let Some((legacy_code, suffix, message)) = mapped {
                return Self::model_validation(
                    ModelValidationPhase::Hydration,
                    legacy_code,
                    projected_role_path(&diagnostic, suffix),
                    message,
                    None,
                );
            }
        }

        if let Some(cause) = cause {
            return match cause {
                ProjectedCrudCompatibilityCause::Orm(error) => {
                    if stage == ProjectedCrudCompatibilityStage::Hydration {
                        Self::from_orm_hydration(error)
                    } else {
                        Self::from_orm(error)
                    }
                }
                ProjectedCrudCompatibilityCause::Commit(error) => {
                    Self::from_orm(error.into_orm_error())
                }
            };
        }
        Self::from_sdk_execution(
            diagnostic,
            if stage == ProjectedCrudCompatibilityStage::Hydration {
                ModelValidationPhase::Hydration
            } else {
                ModelValidationPhase::Input
            },
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

fn flatten_sdk_path(segment: &SdkDiagnosticPathSegment) -> String {
    match segment {
        SdkDiagnosticPathSegment::Argument(value) => value.as_str().to_owned(),
        SdkDiagnosticPathSegment::Index(value) => format!("[{value}]"),
        SdkDiagnosticPathSegment::Type(_) => "type".into(),
        SdkDiagnosticPathSegment::Field(value) => value.attribute().label().as_str().to_owned(),
        SdkDiagnosticPathSegment::Role(value) => value.label().as_str().to_owned(),
        SdkDiagnosticPathSegment::Query(value) => value.as_str().to_owned(),
        SdkDiagnosticPathSegment::QueryBinding(value) => format!("binding[{value}]"),
        SdkDiagnosticPathSegment::QueryField { owner, name } => {
            format!("{}.{}", owner.as_str(), name.as_str())
        }
        SdkDiagnosticPathSegment::QueryRole { owner, name } => {
            format!("{}.{}", owner.as_str(), name.as_str())
        }
        SdkDiagnosticPathSegment::QueryRoleEdge(value) => format!("role_edge[{value}]"),
        SdkDiagnosticPathSegment::QueryOutputSlot(value) => format!("output[{value}]"),
        SdkDiagnosticPathSegment::QueryOutputName(value)
        | SdkDiagnosticPathSegment::ContractField(value)
        | SdkDiagnosticPathSegment::ContractIdentity(value) => value.as_str().to_owned(),
        _ => "diagnostic".into(),
    }
}

fn projected_role_path(diagnostic: &SdkExecutionDiagnostic, suffix: &'static str) -> Vec<String> {
    let role = diagnostic.path().iter().find_map(|segment| {
        let SdkDiagnosticPathSegment::Role(role) = segment else {
            return None;
        };
        Some(role.label().as_str())
    });
    let index = diagnostic.path().iter().find_map(|segment| {
        let SdkDiagnosticPathSegment::Index(index) = segment else {
            return None;
        };
        Some(*index)
    });
    let head = match (role, index) {
        (Some(role), Some(index)) => format!("{role}[{index}]"),
        (Some(role), None) => role.to_owned(),
        _ => "roles".to_owned(),
    };
    vec![head, suffix.to_owned()]
}

fn projected_cause_source(
    cause: ProjectedCrudCompatibilityCause,
) -> Option<Box<dyn StdError + Send + Sync + 'static>> {
    match cause {
        ProjectedCrudCompatibilityCause::Orm(error) => Some(Box::new(error)),
        ProjectedCrudCompatibilityCause::Commit(error) => Some(Box::new(error)),
    }
}

fn typed_sdk_path(segment: &SdkDiagnosticPathSegment) -> ErrorPathSegment {
    match segment {
        SdkDiagnosticPathSegment::Argument(value) => {
            ErrorPathSegment::Field(value.as_str().to_owned())
        }
        SdkDiagnosticPathSegment::Index(value) => ErrorPathSegment::Index(*value),
        SdkDiagnosticPathSegment::Type(value) => ErrorPathSegment::Identifier(format!(
            "{}:{}",
            match value.kind() {
                type_bridge_contract::id::TypeKind::Entity => "entity",
                type_bridge_contract::id::TypeKind::Relation => "relation",
                type_bridge_contract::id::TypeKind::Attribute => "attribute",
                type_bridge_contract::id::TypeKind::Struct => "struct",
            },
            value.label().as_str()
        )),
        SdkDiagnosticPathSegment::Field(value) => ErrorPathSegment::Identifier(format!(
            "{}:{}",
            value.owner().label().as_str(),
            value.attribute().label().as_str()
        )),
        SdkDiagnosticPathSegment::Role(value) => ErrorPathSegment::Identifier(format!(
            "{}:{}",
            value.declaring_relation().as_str(),
            value.label().as_str()
        )),
        SdkDiagnosticPathSegment::Query(value) => ErrorPathSegment::Query(query_path_kind(*value)),
        SdkDiagnosticPathSegment::QueryBinding(value) => ErrorPathSegment::QueryBinding(*value),
        SdkDiagnosticPathSegment::QueryField { owner, name } => ErrorPathSegment::QueryField {
            owner: owner.as_str().to_owned(),
            name: name.as_str().to_owned(),
        },
        SdkDiagnosticPathSegment::QueryRole { owner, name } => ErrorPathSegment::QueryRole {
            owner: owner.as_str().to_owned(),
            name: name.as_str().to_owned(),
        },
        SdkDiagnosticPathSegment::QueryRoleEdge(value) => ErrorPathSegment::QueryRoleEdge(*value),
        SdkDiagnosticPathSegment::QueryOutputSlot(value) => {
            ErrorPathSegment::QueryOutputSlot(*value)
        }
        SdkDiagnosticPathSegment::QueryOutputName(value) => {
            ErrorPathSegment::QueryOutputName(value.as_str().to_owned())
        }
        SdkDiagnosticPathSegment::ContractField(value) => {
            ErrorPathSegment::ContractField(value.as_str().to_owned())
        }
        SdkDiagnosticPathSegment::ContractIdentity(value) => {
            ErrorPathSegment::ContractIdentity(value.as_str().to_owned())
        }
        _ => ErrorPathSegment::Identifier("diagnostic".into()),
    }
}

fn flatten_sdk_detail(value: &SdkDiagnosticDetailValue) -> ErrorDetail {
    match value {
        SdkDiagnosticDetailValue::Boolean(value) => ErrorDetail::Boolean(*value),
        SdkDiagnosticDetailValue::Count(value) | SdkDiagnosticDetailValue::ByteCount(value) => {
            i64::try_from(*value)
                .map_or_else(|_| ErrorDetail::Text(value.to_string()), ErrorDetail::Long)
        }
        SdkDiagnosticDetailValue::Capability(value) => ErrorDetail::Text(value.as_str().to_owned()),
        SdkDiagnosticDetailValue::ValueType(value) => ErrorDetail::Text(value.as_str().to_owned()),
        SdkDiagnosticDetailValue::Type(value) => {
            ErrorDetail::Text(value.label().as_str().to_owned())
        }
        SdkDiagnosticDetailValue::Field(value) => {
            ErrorDetail::Text(value.attribute().label().as_str().to_owned())
        }
        SdkDiagnosticDetailValue::Role(value) => {
            ErrorDetail::Text(value.label().as_str().to_owned())
        }
        SdkDiagnosticDetailValue::Fingerprint(value) => ErrorDetail::Text(value.digest().to_hex()),
        SdkDiagnosticDetailValue::ProviderOperation(value) => {
            ErrorDetail::Text(value.as_str().to_owned())
        }
        SdkDiagnosticDetailValue::CommitOutcome(value) => {
            ErrorDetail::Text(value.as_str().to_owned())
        }
        SdkDiagnosticDetailValue::Signed(value) => ErrorDetail::Long(*value),
        SdkDiagnosticDetailValue::QueryCategory(value) => {
            ErrorDetail::QueryCategory(query_category(*value))
        }
        SdkDiagnosticDetailValue::QueryIdentity(value) => {
            ErrorDetail::QueryIdentity(value.as_str().to_owned())
        }
        SdkDiagnosticDetailValue::QueryIdentityList(values) => ErrorDetail::QueryIdentityList(
            values
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect(),
        ),
        _ => ErrorDetail::Text("diagnostic".into()),
    }
}

fn sdk_query_category(error: &SdkExecutionDiagnostic) -> Option<SdkQueryDiagnosticCategory> {
    error.details().iter().find_map(|(name, value)| {
        (name.as_str() == "query_category").then(|| {
            let SdkDiagnosticDetailValue::QueryCategory(category) = value else {
                return None;
            };
            Some(*category)
        })?
    })
}

const fn query_category(value: SdkQueryDiagnosticCategory) -> QueryDiagnosticCategory {
    match value {
        SdkQueryDiagnosticCategory::InvalidPlan => QueryDiagnosticCategory::InvalidPlan,
        SdkQueryDiagnosticCategory::Cardinality => QueryDiagnosticCategory::Cardinality,
        SdkQueryDiagnosticCategory::UnsupportedCapability => {
            QueryDiagnosticCategory::UnsupportedCapability
        }
        SdkQueryDiagnosticCategory::StaleSchema => QueryDiagnosticCategory::StaleSchema,
        SdkQueryDiagnosticCategory::ResourceLimit => QueryDiagnosticCategory::ResourceLimit,
        SdkQueryDiagnosticCategory::Cancelled => QueryDiagnosticCategory::Cancelled,
        SdkQueryDiagnosticCategory::Provider => QueryDiagnosticCategory::Provider,
        SdkQueryDiagnosticCategory::ResultDecode => QueryDiagnosticCategory::ResultDecode,
        _ => QueryDiagnosticCategory::ResultDecode,
    }
}

const fn query_path_kind(value: SdkQueryDiagnosticPathKind) -> QueryDiagnosticPathKind {
    match value {
        SdkQueryDiagnosticPathKind::Request => QueryDiagnosticPathKind::Request,
        SdkQueryDiagnosticPathKind::Plan => QueryDiagnosticPathKind::Plan,
        SdkQueryDiagnosticPathKind::Operation => QueryDiagnosticPathKind::Operation,
        SdkQueryDiagnosticPathKind::Predicate => QueryDiagnosticPathKind::Predicate,
        SdkQueryDiagnosticPathKind::Output => QueryDiagnosticPathKind::Output,
        SdkQueryDiagnosticPathKind::ProviderEvidence => QueryDiagnosticPathKind::ProviderEvidence,
        SdkQueryDiagnosticPathKind::Result => QueryDiagnosticPathKind::Result,
        _ => QueryDiagnosticPathKind::Result,
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
            (ErrorCategory::Integrity, "integrity"),
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
