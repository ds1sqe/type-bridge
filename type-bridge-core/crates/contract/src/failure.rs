//! Value-only provider failure vocabulary; classification belongs to the runtime.

use serde::{Deserialize, Serialize};

/// Validated TypeDB server code (`[A-Z]{3}[0-9]+`, at most 32 bytes).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Code(String);
impl Code {
    /// Validate a code without retaining invalid input.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let bytes = value.as_bytes();
        (bytes.len() >= 4
            && bytes.len() <= 32
            && bytes[..3].iter().all(u8::is_ascii_uppercase)
            && bytes[3..].iter().all(u8::is_ascii_digit))
        .then_some(Self(value))
    }
    /// Validated code text, without brackets.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for Code {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(decoder)?)
            .ok_or_else(|| serde::de::Error::custom("invalid provider code"))
    }
}
/// Value-free transport diagnosis shared by HTTP discovery and driver adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransportKind {
    /// The operation or HTTP/2 keep-alive timed out.
    Timeout,
    /// The endpoint refused the connection or is unavailable.
    Unavailable,
    /// TLS negotiation or certificate validation failed.
    Tls,
    /// Credentials were rejected.
    Authentication,
    /// The connection or transaction is closed.
    Closed,
    /// No more specific structural transport diagnosis is available.
    Other,
}
impl TransportKind {
    /// Fixed description; never includes provider or connection values.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unavailable => "connection refused or unavailable",
            Self::Tls => "TLS/certificate failure",
            Self::Authentication => "authentication failure",
            Self::Closed => "closed connection",
            Self::Other => "other transport",
        }
    }
    /// Recognize only this runtime-owned fixed vocabulary on legacy strings.
    #[must_use]
    pub fn from_description(value: &str) -> Option<Self> {
        match value {
            "timeout" => Some(Self::Timeout),
            "connection refused or unavailable" => Some(Self::Unavailable),
            "TLS/certificate failure" => Some(Self::Tls),
            "authentication failure" => Some(Self::Authentication),
            "closed connection" => Some(Self::Closed),
            "other transport" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Where the provider rejected an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Phase {
    /// Query execution, before commit.
    Query,
    /// Commit validation.
    Commit,
}
/// Conflict kind reported by fixed provider wording, without winner inference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LockKind {
    /// Concurrent exclusive writes.
    Exclusive,
    /// A concurrent commit deletes data this transaction requires.
    RequireDeletedKey,
    /// This transaction deletes data a concurrent commit requires.
    DeletingRequiredKey,
    /// The provider wording is unrecognized.
    Unknown,
}
/// Annotation identified by a pinned provider code, never by parsing values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnnotationKind {
    /// Abstract instance constraint.
    Abstract,
    /// Regular-expression constraint.
    Regex,
    /// Range constraint.
    Range,
    /// Enumerated values constraint.
    Values,
    /// Distinctness constraint.
    Distinct,
    /// Other annotation constraint.
    Unknown,
}
/// Value-free reason for a proven rejection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Reason {
    /// Commit-time isolation conflict.
    Conflict {
        /// Reported lock kind.
        lock: LockKind,
    },
    /// Cardinality constraint.
    Cardinality,
    /// Unique/key constraint.
    Unique,
    /// Another annotation constraint.
    Constraint {
        /// Annotation kind.
        annotation: AnnotationKind,
    },
    /// Other provider rejection.
    Other {
        /// Validated provider code.
        code: Code,
    },
}
/// Provider outcome without values, transaction identities, or raw prose.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Failure {
    /// The transaction did not store any changes.
    Rejected {
        /// Failure phase.
        phase: Phase,
        /// Value-free rejection reason.
        reason: Reason,
        /// Staged operation index, when structurally available.
        op_index: Option<u32>,
    },
    /// A commit may have applied; repeating it can duplicate changes.
    Indeterminate {
        /// Provider code, if structurally available and valid.
        code: Option<Code>,
    },
}
impl Failure {
    /// Whether repeating the whole transaction cannot duplicate this commit.
    #[must_use]
    pub const fn retry_safe(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
    /// Provider code implied by the typed reason, if known.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Indeterminate { code } => code.as_ref().map(Code::as_str),
            Self::Rejected { reason, .. } => Some(match reason {
                Reason::Conflict { .. } => "STC2",
                Reason::Cardinality => "CNT5",
                Reason::Unique => "CNT9",
                Reason::Constraint { annotation } => match annotation {
                    AnnotationKind::Abstract => "CNT4",
                    AnnotationKind::Regex => "CNT6",
                    AnnotationKind::Range => "CNT7",
                    AnnotationKind::Values => "CNT8",
                    AnnotationKind::Distinct => "CNT10",
                    AnnotationKind::Unknown => return None,
                },
                Reason::Other { code } => code.as_str(),
            }),
        }
    }
    /// Attach the executing staged item without parsing provider text.
    #[must_use]
    pub fn with_op_index(mut self, index: u32) -> Self {
        if let Self::Rejected { op_index, .. } = &mut self {
            *op_index = Some(index);
        }
        self
    }
}

impl Failure {
    /// Flat value-free projection shared by HTTP and language exception attributes.
    #[must_use]
    pub fn host_json(&self) -> serde_json::Value {
        let (phase, reason, lock, annotation, index) = match self {
            Self::Rejected {
                phase,
                reason,
                op_index,
            } => {
                let (name, lock, annotation) = match reason {
                    Reason::Conflict { lock } => (
                        "conflict",
                        Some(match lock {
                            LockKind::Exclusive => "exclusive",
                            LockKind::RequireDeletedKey => "require_deleted_key",
                            LockKind::DeletingRequiredKey => "deleting_required_key",
                            LockKind::Unknown => "unknown",
                        }),
                        None,
                    ),
                    Reason::Cardinality => ("cardinality", None, None),
                    Reason::Unique => ("unique", None, None),
                    Reason::Constraint { annotation } => (
                        "constraint",
                        None,
                        Some(match annotation {
                            AnnotationKind::Abstract => "abstract",
                            AnnotationKind::Regex => "regex",
                            AnnotationKind::Range => "range",
                            AnnotationKind::Values => "values",
                            AnnotationKind::Distinct => "distinct",
                            AnnotationKind::Unknown => "unknown",
                        }),
                    ),
                    Reason::Other { .. } => ("other", None, None),
                };
                (
                    match phase {
                        Phase::Query => "query",
                        Phase::Commit => "commit",
                    },
                    Some(name),
                    lock,
                    annotation,
                    *op_index,
                )
            }
            Self::Indeterminate { .. } => ("commit", None, None, None, None),
        };
        serde_json::json!({"phase":phase,"reason":reason,"lock_kind":lock,"annotation":annotation,
            "op_index":index,"retry_safe":self.retry_safe(),"code":self.code()})
    }
}
