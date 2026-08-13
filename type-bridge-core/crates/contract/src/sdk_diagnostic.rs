//! Versioned in-memory diagnostics for binding-neutral SDK execution.
//!
//! This module is deliberately separate from [`crate::diagnostic`]. The
//! existing diagnostic is part of canonical contract wires; an SDK execution
//! diagnostic is an in-memory engine value that bindings project through their
//! own versioned ABI. It therefore implements no serialization contract.
//!
//! Dynamic provider messages, credentials, endpoint or custom-root paths,
//! query values, and internal backtraces have no representation here. Stable
//! messages borrow implementation-owned static text. Codes and names are
//! bounded owned identifiers so canonical typed-query fields can cross this
//! seam without leaking storage or accepting free-form provider text. Dynamic
//! context remains limited to the closed typed detail vocabulary.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::capability::CapabilityId;
use crate::fingerprint::Fingerprint;
use crate::id::{RoleId, TypeId};
use crate::schema::OwnsFactId;
use crate::value::ValueTypeTag;

/// Maximum bytes in one stable execution-diagnostic code.
pub const MAX_SDK_DIAGNOSTIC_CODE_BYTES: usize = 128;
/// Maximum UTF-8 bytes in one stable execution-diagnostic message.
pub const MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES: usize = 512;
/// Maximum bytes in one stable path name or detail key.
pub const MAX_SDK_DIAGNOSTIC_NAME_BYTES: usize = 128;
/// Maximum typed path segments in one execution diagnostic.
pub const MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS: usize = 32;
/// Maximum typed details in one execution diagnostic.
pub const MAX_SDK_DIAGNOSTIC_DETAILS: usize = 32;
/// Maximum UTF-8 bytes in one query-owned diagnostic identity.
pub const MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_BYTES: usize = 512;
/// Maximum identities in one query diagnostic list detail.
pub const MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_LIST: usize = 32;

/// The version of the in-memory SDK execution-diagnostic contract.
///
/// The numeric value is projected explicitly by each binding. It is not a
/// Rust layout or a serialization discriminant.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkDiagnosticVersion(u16);

impl SdkDiagnosticVersion {
    /// The first SDK execution-diagnostic contract.
    pub const V1: Self = Self(1);
    /// The version produced by constructors in this module.
    pub const CURRENT: Self = Self::V1;

    /// Return the stable positive version number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Stable binding-neutral SDK execution failure categories.
///
/// This is a new, forward-extensible category vocabulary. It intentionally
/// does not add variants to the released [`crate::diagnostic::DiagnosticCategory`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkDiagnosticCategory {
    /// Caller input does not satisfy the generated SDK contract.
    InvalidInput,
    /// The selected runtime or provider cannot execute a required capability.
    UnsupportedCapability,
    /// A configured or implementation-defined resource ceiling was exceeded.
    ResourceLimit,
    /// Schema, projection, database, request, or result identity did not match.
    Integrity,
    /// The database provider could not complete an operation.
    Provider,
    /// Transaction lifecycle or commit certainty prevents a successful result.
    Transaction,
    /// The operation was cancelled before provider dispatch.
    Cancelled,
    /// TypeBridge failed internally without exposing implementation state.
    Internal,
}

impl SdkDiagnosticCategory {
    /// Return the stable language-neutral category spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::ResourceLimit => "resource_limit",
            Self::Integrity => "integrity",
            Self::Provider => "provider",
            Self::Transaction => "transaction",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for SdkDiagnosticCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an SDK diagnostic component or bounded collection was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SdkDiagnosticBuildError {
    /// A code was not canonical lowercase snake case or exceeded its ceiling.
    InvalidCode,
    /// A message was empty, oversized, multiline, path-like, or not trimmed.
    InvalidMessage,
    /// A path name or detail key was not canonical lowercase snake case.
    InvalidName,
    /// The typed path exceeded [`MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS`].
    PathLimitExceeded,
    /// The detail map exceeded [`MAX_SDK_DIAGNOSTIC_DETAILS`].
    DetailLimitExceeded,
    /// The same stable detail key was attached more than once.
    DuplicateDetail,
}

impl fmt::Display for SdkDiagnosticBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidCode => {
                "SDK diagnostic code must be bounded canonical lowercase snake case"
            }
            Self::InvalidMessage => {
                "SDK diagnostic message must be bounded, trimmed, single-line, and path-free static text"
            }
            Self::InvalidName => {
                "SDK diagnostic name must be bounded canonical lowercase snake case"
            }
            Self::PathLimitExceeded => "SDK diagnostic path exceeds its segment ceiling",
            Self::DetailLimitExceeded => "SDK diagnostic details exceed their entry ceiling",
            Self::DuplicateDetail => "SDK diagnostic detail key is duplicated",
        };
        formatter.write_str(message)
    }
}

impl Error for SdkDiagnosticBuildError {}

/// A validated stable machine-readable execution-diagnostic code.
///
/// Codes are bounded canonical identifiers. Lowering code must still admit
/// only engine-owned stable codes, never provider messages or query values.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkDiagnosticCode(String);

impl SdkDiagnosticCode {
    /// Validate one implementation-owned code.
    pub fn new(value: impl Into<String>) -> Result<Self, SdkDiagnosticBuildError> {
        let value = value.into();
        if is_canonical_snake_name(&value, MAX_SDK_DIAGNOSTIC_CODE_BYTES) {
            Ok(Self(value))
        } else {
            Err(SdkDiagnosticBuildError::InvalidCode)
        }
    }

    /// Return the stable code spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SdkDiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A validated stable human-readable execution-diagnostic message.
///
/// The message is implementation-owned static text, not a formatting target.
/// Runtime context belongs only in [`SdkDiagnosticDetailValue`]. Path
/// separators and control characters are rejected so an endpoint, custom-root
/// path, or backtrace cannot be copied into this field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkDiagnosticMessage(&'static str);

impl SdkDiagnosticMessage {
    /// Validate one implementation-owned message.
    pub fn new(value: &'static str) -> Result<Self, SdkDiagnosticBuildError> {
        let valid = !value.is_empty()
            && value.len() <= MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES
            && value.trim() == value
            && !value
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'));
        if valid {
            Ok(Self(value))
        } else {
            Err(SdkDiagnosticBuildError::InvalidMessage)
        }
    }

    /// Return the stable message text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for SdkDiagnosticMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A validated stable argument name or detail key.
///
/// Names are bounded canonical identifiers. Dynamic schema identities use the
/// typed path and detail variants instead of being flattened into names.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkDiagnosticName(String);

impl SdkDiagnosticName {
    /// Validate one implementation-owned name.
    pub fn new(value: impl Into<String>) -> Result<Self, SdkDiagnosticBuildError> {
        let value = value.into();
        if is_canonical_snake_name(&value, MAX_SDK_DIAGNOSTIC_NAME_BYTES) {
            Ok(Self(value))
        } else {
            Err(SdkDiagnosticBuildError::InvalidName)
        }
    }

    /// Return the stable name spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SdkDiagnosticName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A bounded query-owned identity retained by a structured SDK diagnostic.
///
/// This is not a free-text diagnostic channel. The constructor accepts only a
/// single-line, whitespace-free identifier shape and rejects path separators,
/// control characters, and unbounded text. Query lowering additionally admits
/// identities only for closed, case-specific detail keys.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkQueryDiagnosticIdentity(String);

impl SdkQueryDiagnosticIdentity {
    /// Validate one bounded query identity.
    pub fn new(value: impl Into<String>) -> Result<Self, SdkDiagnosticBuildError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_BYTES
            && !value.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || matches!(character, '/' | '\\' | '@')
            });
        if valid {
            Ok(Self(value))
        } else {
            Err(SdkDiagnosticBuildError::InvalidName)
        }
    }

    /// Return the exact validated identity spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SdkQueryDiagnosticIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The exact canonical typed-query failure category retained inside an SDK
/// diagnostic whose outer category is binding-neutral.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkQueryDiagnosticCategory {
    /// The immutable query plan is invalid.
    InvalidPlan,
    /// A terminal cardinality contract was not satisfied.
    Cardinality,
    /// The provider lacks a required query capability.
    UnsupportedCapability,
    /// Request-relevant schema changed after validation.
    StaleSchema,
    /// A query processing ceiling was crossed.
    ResourceLimit,
    /// Cooperative cancellation interrupted query processing.
    Cancelled,
    /// The provider failed before complete evidence was available.
    Provider,
    /// Provider evidence did not match the validated invocation.
    ResultDecode,
}

impl SdkQueryDiagnosticCategory {
    /// Return the canonical typed-query category spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPlan => "invalid_plan",
            Self::Cardinality => "cardinality",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::StaleSchema => "stale_schema",
            Self::ResourceLimit => "resource_limit",
            Self::Cancelled => "cancelled",
            Self::Provider => "provider",
            Self::ResultDecode => "result_decode",
        }
    }
}

/// One closed structural location within a typed query request or result.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkQueryDiagnosticPathKind {
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

impl SdkQueryDiagnosticPathKind {
    /// Return the stable structural spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Plan => "plan",
            Self::Operation => "operation",
            Self::Predicate => "predicate",
            Self::Output => "output",
            Self::ProviderEvidence => "provider_evidence",
            Self::Result => "result",
        }
    }
}

fn is_canonical_snake_name(value: impl AsRef<str>, maximum_bytes: usize) -> bool {
    let value = value.as_ref();
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > maximum_bytes
        || !bytes[0].is_ascii_lowercase()
        || !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit()
    {
        return false;
    }

    let mut previous_was_underscore = false;
    for byte in bytes {
        if *byte == b'_' {
            if previous_was_underscore {
                return false;
            }
            previous_was_underscore = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_was_underscore = false;
        } else {
            return false;
        }
    }
    true
}

/// One typed location within an SDK operation input or projected model.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkDiagnosticPathSegment {
    /// A stable SDK operation argument.
    Argument(SdkDiagnosticName),
    /// A zero-based item position in a bounded input collection.
    Index(u64),
    /// A canonical schema type identity.
    Type(TypeId),
    /// A canonical direct ownership-fact identity.
    Field(OwnsFactId),
    /// A canonical relation-qualified role identity.
    Role(RoleId),
    /// A structural typed-query request or result location.
    Query(SdkQueryDiagnosticPathKind),
    /// A plan-local typed-query binding ordinal.
    QueryBinding(u16),
    /// A descriptor-qualified, binding-facing typed-query field identity.
    QueryField {
        /// Kind-qualified registry descriptor identity.
        owner: SdkQueryDiagnosticIdentity,
        /// Binding-facing field name.
        name: SdkQueryDiagnosticIdentity,
    },
    /// A descriptor-qualified typed-query role identity.
    QueryRole {
        /// Kind-qualified registry descriptor identity.
        owner: SdkQueryDiagnosticIdentity,
        /// Binding-facing role name.
        name: SdkQueryDiagnosticIdentity,
    },
    /// A plan-local typed-query role-edge ordinal.
    QueryRoleEdge(u16),
    /// A zero-based positional typed-query output slot.
    QueryOutputSlot(u64),
    /// A declared generated typed-query output member.
    QueryOutputName(SdkQueryDiagnosticIdentity),
    /// An object field retained from one authenticated contract diagnostic.
    ContractField(SdkQueryDiagnosticIdentity),
    /// A typed identifier retained from one authenticated contract diagnostic.
    ContractIdentity(SdkQueryDiagnosticIdentity),
}

/// A provider operation class safe to expose across SDK boundaries.
///
/// Provider-specific error text, codes, endpoints, and query text are not
/// retained. Commit is intentionally absent: failed commits use
/// [`SdkCommitFailureOutcome`] so their durability certainty cannot be lost.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkProviderOperation {
    /// Establish or verify a direct database connection.
    Connect,
    /// Read or install schema authority.
    Schema,
    /// Open a read transaction.
    OpenReadTransaction,
    /// Open a write transaction.
    OpenWriteTransaction,
    /// Execute a read operation.
    Read,
    /// Execute a write operation before commit.
    Write,
    /// Roll back a write transaction.
    Rollback,
    /// Close a provider-owned resource.
    Close,
}

impl SdkProviderOperation {
    /// Return the stable language-neutral operation spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Schema => "schema",
            Self::OpenReadTransaction => "open_read_transaction",
            Self::OpenWriteTransaction => "open_write_transaction",
            Self::Read => "read",
            Self::Write => "write",
            Self::Rollback => "rollback",
            Self::Close => "close",
        }
    }
}

/// Durability certainty for a failed transaction commit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SdkCommitFailureOutcome {
    /// The provider proves that the transaction did not commit.
    DefinitelyAborted,
    /// The provider cannot determine whether the transaction committed.
    Unknown,
}

impl SdkCommitFailureOutcome {
    /// Return the stable language-neutral outcome spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefinitelyAborted => "definitely_aborted",
            Self::Unknown => "unknown",
        }
    }
}

/// One bounded typed SDK execution-diagnostic detail value.
///
/// There is deliberately no free-text or byte-string variant. Every dynamic
/// value is either a bounded canonical identity or a non-sensitive scalar
/// classification.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SdkDiagnosticDetailValue {
    /// A boolean contract fact.
    Boolean(bool),
    /// A non-negative item or occurrence count.
    Count(u64),
    /// A non-negative byte count.
    ByteCount(u64),
    /// A canonical capability identity.
    Capability(CapabilityId),
    /// A canonical scalar-domain identity.
    ValueType(ValueTypeTag),
    /// A canonical schema type identity.
    Type(TypeId),
    /// A canonical direct ownership-fact identity.
    Field(OwnsFactId),
    /// A canonical relation-qualified role identity.
    Role(RoleId),
    /// A complete bounded, domain-separated fingerprint.
    Fingerprint(Fingerprint),
    /// A redacted provider operation classification.
    ProviderOperation(SdkProviderOperation),
    /// A classified failed-commit outcome.
    CommitOutcome(SdkCommitFailureOutcome),
    /// A signed bound or scalar diagnostic value.
    Signed(i64),
    /// The exact canonical typed-query category.
    QueryCategory(SdkQueryDiagnosticCategory),
    /// One bounded typed-query identity admitted for a closed detail key.
    QueryIdentity(SdkQueryDiagnosticIdentity),
    /// A bounded ordered list of typed-query identities.
    QueryIdentityList(Vec<SdkQueryDiagnosticIdentity>),
}

/// Raw presence of one binding-supplied projection-evidence slot.
///
/// This closed value records presence only. It does not parse evidence bytes,
/// infer package provenance, or classify arbitrary evidence collections.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SdkProjectionEvidenceSlotPresence {
    /// The binding received no bytes for the required slot.
    Absent,
    /// The binding received one or more bytes for the slot.
    Present,
}

/// One stable, binding-neutral SDK execution failure.
///
/// This value is versioned but intentionally has no wire representation. A
/// binding must copy its accessor results into that binding's owned diagnostic
/// handle and must not expose this Rust layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkExecutionDiagnostic {
    version: SdkDiagnosticVersion,
    category: SdkDiagnosticCategory,
    code: SdkDiagnosticCode,
    message: SdkDiagnosticMessage,
    path: Vec<SdkDiagnosticPathSegment>,
    details: BTreeMap<SdkDiagnosticName, SdkDiagnosticDetailValue>,
}

impl SdkExecutionDiagnostic {
    fn stable(
        category: SdkDiagnosticCategory,
        code: SdkDiagnosticCode,
        message: SdkDiagnosticMessage,
    ) -> Self {
        Self {
            version: SdkDiagnosticVersion::CURRENT,
            category,
            code,
            message,
            path: Vec::new(),
            details: BTreeMap::new(),
        }
    }

    /// Construct a stable invalid-input diagnostic.
    #[must_use]
    pub fn invalid_input(code: SdkDiagnosticCode, message: SdkDiagnosticMessage) -> Self {
        Self::stable(SdkDiagnosticCategory::InvalidInput, code, message)
    }

    /// Construct a stable unsupported-capability diagnostic.
    #[must_use]
    pub fn unsupported_capability(code: SdkDiagnosticCode, message: SdkDiagnosticMessage) -> Self {
        Self::stable(SdkDiagnosticCategory::UnsupportedCapability, code, message)
    }

    /// Construct a stable resource-limit diagnostic.
    #[must_use]
    pub fn resource_limit(code: SdkDiagnosticCode, message: SdkDiagnosticMessage) -> Self {
        Self::stable(SdkDiagnosticCategory::ResourceLimit, code, message)
    }

    /// Construct a stable integrity diagnostic.
    #[must_use]
    pub fn integrity(code: SdkDiagnosticCode, message: SdkDiagnosticMessage) -> Self {
        Self::stable(SdkDiagnosticCategory::Integrity, code, message)
    }

    /// Construct the fixed generated-package projection-evidence diagnostic.
    ///
    /// Callers may append the exact rejected evidence index and validated
    /// evidence identity, plus closed typed count or provenance details. The
    /// root argument is fixed here so every binding exposes the same path.
    #[must_use]
    pub fn projection_evidence_mismatch() -> Self {
        Self::stable(
            SdkDiagnosticCategory::Integrity,
            static_code("projection_evidence_mismatch"),
            static_message(
                "Generated projection evidence does not match the verified schema package",
            ),
        )
        .try_at(SdkDiagnosticPathSegment::Argument(static_name(
            "projection_evidence",
        )))
        .expect("the fixed projection-evidence diagnostic path is bounded")
    }

    /// Construct the fixed diagnostic for one required projection-evidence
    /// identity that is absent from its expected position.
    ///
    /// The caller supplies a validated contract identity and whether the
    /// rejected package was foreign. The occurrence counts are fixed by the
    /// missing-evidence case: exactly one occurrence was expected and none was
    /// present. Other mismatch shapes must use
    /// [`Self::projection_evidence_mismatch`] and attach their own exact typed
    /// metadata rather than relabeling them as missing evidence.
    #[must_use]
    pub fn projection_evidence_missing(
        index: u64,
        identity: SdkQueryDiagnosticIdentity,
        foreign_package: bool,
    ) -> Self {
        Self::projection_evidence_mismatch()
            .try_at(SdkDiagnosticPathSegment::Index(index))
            .and_then(|diagnostic| {
                diagnostic.try_at(SdkDiagnosticPathSegment::ContractIdentity(identity))
            })
            .expect("the fixed missing-evidence diagnostic path is bounded")
            .with_static_detail(
                static_name("actual_occurrence_count"),
                SdkDiagnosticDetailValue::Count(0),
            )
            .with_static_detail(
                static_name("expected_occurrence_count"),
                SdkDiagnosticDetailValue::Count(1),
            )
            .with_static_detail(
                static_name("foreign_package"),
                SdkDiagnosticDetailValue::Boolean(foreign_package),
            )
    }

    /// Classify a rejected package by the raw presence of its detached
    /// semantic-schema-fingerprint evidence.
    ///
    /// The detached semantic fingerprint is the first item in the current
    /// generated-package admission evidence, so an absent slot has canonical
    /// index zero and identity `semantic_schema_fingerprint`. Absence alone
    /// never proves a foreign package. A present slot deliberately retains the
    /// generic mismatch because malformed, noncanonical, stale, forged, or
    /// otherwise conflicting bytes cannot honestly be narrowed by presence.
    #[must_use]
    pub fn classify_detached_semantic_schema_fingerprint_rejection(
        presence: SdkProjectionEvidenceSlotPresence,
    ) -> Self {
        match presence {
            SdkProjectionEvidenceSlotPresence::Absent => Self::projection_evidence_missing(
                0,
                SdkQueryDiagnosticIdentity::new("semantic_schema_fingerprint")
                    .expect("the fixed projection-evidence identity is canonical"),
                false,
            ),
            SdkProjectionEvidenceSlotPresence::Present => Self::projection_evidence_mismatch(),
        }
    }

    /// Construct the fixed generated-token package-brand diagnostic.
    ///
    /// Operation owners append the exact model, field, role, or query path at
    /// which a token from another installed package was rejected.
    #[must_use]
    pub fn generated_token_package_mismatch() -> Self {
        Self::stable(
            SdkDiagnosticCategory::Integrity,
            static_code("generated_token_package_mismatch"),
            static_message("The generated token belongs to a different installed schema package"),
        )
    }

    /// Construct a redacted provider diagnostic.
    ///
    /// The constructor accepts only a closed operation class. In particular,
    /// it cannot receive provider error text, credentials, endpoints, paths,
    /// query text, or query values.
    #[must_use]
    pub fn provider_failure(operation: SdkProviderOperation) -> Self {
        Self::stable(
            SdkDiagnosticCategory::Provider,
            static_code("provider_operation_failed"),
            static_message("The database provider could not complete the requested operation"),
        )
        .with_static_detail(
            static_name("operation"),
            SdkDiagnosticDetailValue::ProviderOperation(operation),
        )
    }

    /// Construct a classified, redacted commit-failure diagnostic.
    ///
    /// The exact provider message is deliberately discarded. An unknown
    /// outcome requires state reconciliation before any retry.
    #[must_use]
    pub fn commit_failure(outcome: SdkCommitFailureOutcome) -> Self {
        let (code, message) = match outcome {
            SdkCommitFailureOutcome::DefinitelyAborted => (
                "commit_definitely_aborted",
                "The provider proves that the transaction did not commit",
            ),
            SdkCommitFailureOutcome::Unknown => (
                "commit_outcome_unknown",
                "The transaction commit outcome is unknown and state must be reconciled before retry",
            ),
        };
        Self::stable(
            SdkDiagnosticCategory::Transaction,
            static_code(code),
            static_message(message),
        )
        .with_static_detail(
            static_name("commit_outcome"),
            SdkDiagnosticDetailValue::CommitOutcome(outcome),
        )
    }

    /// Construct a stable transaction-lifecycle diagnostic.
    ///
    /// Dynamic transaction or provider text cannot enter this constructor;
    /// callers supply only implementation-owned stable code and message text.
    #[must_use]
    pub fn transaction_failure(code: SdkDiagnosticCode, message: SdkDiagnosticMessage) -> Self {
        Self::stable(SdkDiagnosticCategory::Transaction, code, message)
    }

    /// Construct a typed-query diagnostic while retaining its exact canonical
    /// query category inside the binding-neutral SDK envelope.
    ///
    /// The message is selected from a closed redacted vocabulary. Dynamic
    /// provider text, query operands, and transport context cannot enter this
    /// constructor.
    #[must_use]
    pub fn query_failure(category: SdkQueryDiagnosticCategory, code: SdkDiagnosticCode) -> Self {
        let (outer, message) = match category {
            SdkQueryDiagnosticCategory::InvalidPlan => (
                SdkDiagnosticCategory::InvalidInput,
                "The typed query plan does not satisfy the generated query contract",
            ),
            SdkQueryDiagnosticCategory::Cardinality => (
                SdkDiagnosticCategory::InvalidInput,
                "The typed query result does not satisfy the requested cardinality",
            ),
            SdkQueryDiagnosticCategory::UnsupportedCapability => (
                SdkDiagnosticCategory::UnsupportedCapability,
                "The provider cannot execute a capability required by this typed query",
            ),
            SdkQueryDiagnosticCategory::StaleSchema => (
                SdkDiagnosticCategory::Integrity,
                "The typed query schema authority changed after request validation",
            ),
            SdkQueryDiagnosticCategory::ResourceLimit => (
                SdkDiagnosticCategory::ResourceLimit,
                "The typed query exceeded a binding neutral execution limit",
            ),
            SdkQueryDiagnosticCategory::Cancelled => (
                SdkDiagnosticCategory::Cancelled,
                "The typed query was cancelled during execution",
            ),
            SdkQueryDiagnosticCategory::Provider => (
                SdkDiagnosticCategory::Provider,
                "The database provider could not complete the typed query",
            ),
            SdkQueryDiagnosticCategory::ResultDecode => (
                SdkDiagnosticCategory::Integrity,
                "Typed query evidence does not match the validated request invocation",
            ),
        };
        Self::stable(outer, code, static_message(message)).with_static_detail(
            static_name("query_category"),
            SdkDiagnosticDetailValue::QueryCategory(category),
        )
    }

    /// Construct the fixed diagnostic for pre-dispatch cancellation.
    #[must_use]
    pub fn cancelled_before_dispatch() -> Self {
        Self::stable(
            SdkDiagnosticCategory::Cancelled,
            static_code("cancelled_before_dispatch"),
            static_message("The operation was cancelled before provider dispatch"),
        )
    }

    /// Construct a redacted internal failure with no implementation details.
    #[must_use]
    pub fn internal_failure() -> Self {
        Self::stable(
            SdkDiagnosticCategory::Internal,
            static_code("internal_failure"),
            static_message("The operation failed inside the TypeBridge runtime"),
        )
    }

    /// Append one typed path segment, enforcing the path ceiling.
    pub fn try_at(
        mut self,
        segment: SdkDiagnosticPathSegment,
    ) -> Result<Self, SdkDiagnosticBuildError> {
        if self.path.len() == MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS {
            return Err(SdkDiagnosticBuildError::PathLimitExceeded);
        }
        self.path.push(segment);
        Ok(self)
    }

    /// Attach one typed detail, enforcing uniqueness and the detail ceiling.
    pub fn try_with_detail(
        mut self,
        key: SdkDiagnosticName,
        value: SdkDiagnosticDetailValue,
    ) -> Result<Self, SdkDiagnosticBuildError> {
        if self.details.contains_key(&key) {
            return Err(SdkDiagnosticBuildError::DuplicateDetail);
        }
        if self.details.len() == MAX_SDK_DIAGNOSTIC_DETAILS {
            return Err(SdkDiagnosticBuildError::DetailLimitExceeded);
        }
        self.details.insert(key, value);
        Ok(self)
    }

    fn with_static_detail(
        mut self,
        key: SdkDiagnosticName,
        value: SdkDiagnosticDetailValue,
    ) -> Self {
        let replaced = self.details.insert(key, value);
        debug_assert!(replaced.is_none());
        self
    }

    /// Return the in-memory diagnostic contract version.
    #[must_use]
    pub const fn version(&self) -> SdkDiagnosticVersion {
        self.version
    }

    /// Return the stable category.
    #[must_use]
    pub const fn category(&self) -> SdkDiagnosticCategory {
        self.category
    }

    /// Return the validated stable code.
    #[must_use]
    pub const fn code(&self) -> &SdkDiagnosticCode {
        &self.code
    }

    /// Return the validated stable message.
    #[must_use]
    pub const fn message(&self) -> SdkDiagnosticMessage {
        self.message
    }

    /// Return the ordered typed path.
    #[must_use]
    pub fn path(&self) -> &[SdkDiagnosticPathSegment] {
        &self.path
    }

    /// Return deterministic typed details in lexical key order.
    #[must_use]
    pub const fn details(&self) -> &BTreeMap<SdkDiagnosticName, SdkDiagnosticDetailValue> {
        &self.details
    }
}

impl fmt::Display for SdkExecutionDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} [{}]: {}",
            self.category, self.code, self.message
        )
    }
}

impl Error for SdkExecutionDiagnostic {}

fn static_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static SDK diagnostic code is canonical")
}

fn static_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static SDK diagnostic message is valid")
}

fn static_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static SDK diagnostic name is canonical")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_package_diagnostics_have_fixed_binding_neutral_vocabulary() {
        let evidence = SdkExecutionDiagnostic::projection_evidence_mismatch();
        assert_eq!(evidence.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(evidence.code().as_str(), "projection_evidence_mismatch");
        assert_eq!(
            evidence.message().as_str(),
            "Generated projection evidence does not match the verified schema package",
        );
        assert!(matches!(
            evidence.path(),
            [SdkDiagnosticPathSegment::Argument(name)]
                if name.as_str() == "projection_evidence"
        ));
        assert!(evidence.details().is_empty());

        let token = SdkExecutionDiagnostic::generated_token_package_mismatch();
        assert_eq!(token.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(token.code().as_str(), "generated_token_package_mismatch");
        assert_eq!(
            token.message().as_str(),
            "The generated token belongs to a different installed schema package",
        );
        assert!(token.path().is_empty());
        assert!(token.details().is_empty());
    }

    #[test]
    fn detached_semantic_fingerprint_classifier_matches_v3_representative() {
        let missing =
            SdkExecutionDiagnostic::classify_detached_semantic_schema_fingerprint_rejection(
                SdkProjectionEvidenceSlotPresence::Absent,
            );
        assert_eq!(missing.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(missing.code().as_str(), "projection_evidence_mismatch");
        assert!(matches!(
            missing.path(),
            [
                SdkDiagnosticPathSegment::Argument(argument),
                SdkDiagnosticPathSegment::Index(0),
                SdkDiagnosticPathSegment::ContractIdentity(identity),
            ] if argument.as_str() == "projection_evidence"
                && identity.as_str() == "semantic_schema_fingerprint"
        ));
        assert_eq!(
            missing
                .details()
                .iter()
                .map(|(name, value)| (name.as_str(), value))
                .collect::<Vec<_>>(),
            vec![
                (
                    "actual_occurrence_count",
                    &SdkDiagnosticDetailValue::Count(0),
                ),
                (
                    "expected_occurrence_count",
                    &SdkDiagnosticDetailValue::Count(1),
                ),
                ("foreign_package", &SdkDiagnosticDetailValue::Boolean(false),),
            ],
        );

        let present =
            SdkExecutionDiagnostic::classify_detached_semantic_schema_fingerprint_rejection(
                SdkProjectionEvidenceSlotPresence::Present,
            );
        assert_eq!(
            present,
            SdkExecutionDiagnostic::projection_evidence_mismatch()
        );
    }
}
