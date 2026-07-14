//! Stable structured errors for canonical match requests and results.
//!
//! Error categories are part of the cross-language contract. Human-readable
//! messages may improve over time, while callers key behavior on the category,
//! stable code, structured path, and deterministic detail map.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::ids::{BindingId, FieldId, RoleEdgeId, RoleId};

/// Stable high-level failure categories preserved across language bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchErrorCategory {
    /// The request is structurally or semantically invalid.
    InvalidPlan,
    /// A terminal's required row cardinality was not satisfied.
    Cardinality,
    /// The provider cannot execute a feature required by the validated request.
    UnsupportedCapability,
    /// Relevant descriptors changed after canonical request validation.
    StaleSchema,
    /// A canonical or provider/session resource ceiling was exceeded.
    ResourceLimit,
    /// The provider failed before it could produce complete evidence.
    Provider,
    /// Provider evidence or hydrated output did not match the validated request.
    ResultDecode,
}

impl MatchErrorCategory {
    /// Return the stable language-neutral category spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPlan => "invalid_plan",
            Self::Cardinality => "cardinality",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::StaleSchema => "stale_schema",
            Self::ResourceLimit => "resource_limit",
            Self::Provider => "provider",
            Self::ResultDecode => "result_decode",
        }
    }
}

impl fmt::Display for MatchErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A stable machine-readable code within one [`MatchErrorCategory`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatchErrorCode(String);

impl MatchErrorCode {
    /// Construct a code inside the canonical request/result implementation.
    pub(crate) fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// Return the stable code spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MatchErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One typed segment in the location of a match error.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MatchErrorPathSegment {
    /// The request envelope.
    Request,
    /// The graph plan.
    Plan,
    /// The selected operation.
    Operation,
    /// The canonical predicate tree.
    Predicate,
    /// The declared output shape.
    Output,
    /// Provider solution evidence.
    ProviderEvidence,
    /// The returned result envelope.
    Result,
    /// A plan binding.
    Binding(BindingId),
    /// A descriptor-qualified field.
    Field(FieldId),
    /// A descriptor-qualified role.
    Role(RoleId),
    /// A canonical role edge.
    RoleEdge(RoleEdgeId),
    /// A positional output slot.
    OutputSlot(usize),
    /// A named output member.
    OutputName(String),
    /// An indexed collection or expression member.
    Index(usize),
}

impl fmt::Display for MatchErrorPathSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request => formatter.write_str("request"),
            Self::Plan => formatter.write_str("plan"),
            Self::Operation => formatter.write_str("operation"),
            Self::Predicate => formatter.write_str("predicate"),
            Self::Output => formatter.write_str("output"),
            Self::ProviderEvidence => formatter.write_str("provider_evidence"),
            Self::Result => formatter.write_str("result"),
            Self::Binding(binding) => write!(formatter, "binding[{binding}]"),
            Self::Field(field) => write!(formatter, "field[{}:{}]", field.owner, field.name),
            Self::Role(role) => write!(formatter, "role[{}:{}]", role.owner, role.name),
            Self::RoleEdge(edge) => write!(formatter, "role_edge[{edge}]"),
            Self::OutputSlot(slot) => write!(formatter, "slot[{slot}]"),
            Self::OutputName(name) => write!(formatter, "name[{name}]"),
            Self::Index(index) => write!(formatter, "index[{index}]"),
        }
    }
}

/// A structured path into a request, provider assignment, or result.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatchErrorPath(Vec<MatchErrorPathSegment>);

impl MatchErrorPath {
    /// Construct an empty path.
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Construct a path from typed segments.
    pub fn from_segments(segments: impl IntoIterator<Item = MatchErrorPathSegment>) -> Self {
        Self(segments.into_iter().collect())
    }

    /// Return the typed path segments.
    pub fn segments(&self) -> &[MatchErrorPathSegment] {
        &self.0
    }

    /// Return whether this error is not attached to a narrower location.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Append a segment inside the canonical validator or result decoder.
    pub(crate) fn push(&mut self, segment: MatchErrorPathSegment) {
        self.0.push(segment);
    }
}

impl fmt::Display for MatchErrorPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return formatter.write_str("<root>");
        }
        for (index, segment) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(".")?;
            }
            segment.fmt(formatter)?;
        }
        Ok(())
    }
}

/// A typed value attached to a structured match-error detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MatchErrorDetailValue {
    /// A textual identifier or diagnostic value.
    Text(String),
    /// A non-negative count, bound, or byte size.
    Unsigned(u64),
    /// A signed numeric value.
    Signed(i64),
    /// A boolean diagnostic fact.
    Boolean(bool),
    /// An ordered list of textual identifiers.
    TextList(Vec<String>),
}

impl From<String> for MatchErrorDetailValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for MatchErrorDetailValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<u64> for MatchErrorDetailValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for MatchErrorDetailValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<Vec<String>> for MatchErrorDetailValue {
    fn from(value: Vec<String>) -> Self {
        Self::TextList(value)
    }
}

/// One stable structured match-request or result failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchError {
    category: MatchErrorCategory,
    code: MatchErrorCode,
    message: String,
    path: MatchErrorPath,
    details: BTreeMap<String, MatchErrorDetailValue>,
}

impl MatchError {
    /// Construct an error inside the canonical match implementation.
    pub(crate) fn new(
        category: MatchErrorCategory,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            code: MatchErrorCode::new(code),
            message: message.into(),
            path: MatchErrorPath::new(),
            details: BTreeMap::new(),
        }
    }

    /// Attach a complete typed path.
    pub(crate) fn with_path(mut self, path: MatchErrorPath) -> Self {
        self.path = path;
        self
    }

    /// Append one typed path segment.
    pub(crate) fn at(mut self, segment: MatchErrorPathSegment) -> Self {
        self.path.push(segment);
        self
    }

    /// Attach one deterministic structured detail.
    pub(crate) fn with_detail(
        mut self,
        key: impl Into<String>,
        value: impl Into<MatchErrorDetailValue>,
    ) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Return the stable high-level category.
    pub const fn category(&self) -> MatchErrorCategory {
        self.category
    }

    /// Return the stable machine-readable code.
    pub fn code(&self) -> &MatchErrorCode {
        &self.code
    }

    /// Return the human-readable diagnostic.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Return the structured error location.
    pub fn path(&self) -> &MatchErrorPath {
        &self.path
    }

    /// Return deterministic structured error details.
    pub fn details(&self) -> &BTreeMap<String, MatchErrorDetailValue> {
        &self.details
    }
}

impl fmt::Display for MatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} [{}] at {}: {}",
            self.category, self.code, self.path, self.message
        )
    }
}

impl Error for MatchError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_have_stable_language_neutral_spellings() {
        let categories = [
            (MatchErrorCategory::InvalidPlan, "invalid_plan"),
            (MatchErrorCategory::Cardinality, "cardinality"),
            (
                MatchErrorCategory::UnsupportedCapability,
                "unsupported_capability",
            ),
            (MatchErrorCategory::StaleSchema, "stale_schema"),
            (MatchErrorCategory::ResourceLimit, "resource_limit"),
            (MatchErrorCategory::Provider, "provider"),
            (MatchErrorCategory::ResultDecode, "result_decode"),
        ];

        for (category, spelling) in categories {
            assert_eq!(category.as_str(), spelling);
            assert_eq!(
                serde_json::to_string(&category).unwrap(),
                format!(r#""{spelling}""#)
            );
        }
    }

    #[test]
    fn error_keeps_typed_path_and_deterministic_details() {
        let error = MatchError::new(
            MatchErrorCategory::InvalidPlan,
            "duplicate_selection",
            "one binding cannot occupy two output slots",
        )
        .with_path(MatchErrorPath::from_segments([
            MatchErrorPathSegment::Request,
            MatchErrorPathSegment::Output,
        ]))
        .at(MatchErrorPathSegment::OutputSlot(1))
        .with_detail("binding", "2")
        .with_detail("first_slot", 0_u64);

        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(error.code().as_str(), "duplicate_selection");
        assert_eq!(error.path().to_string(), "request.output.slot[1]");
        assert_eq!(
            error.to_string(),
            "invalid_plan [duplicate_selection] at request.output.slot[1]: one binding cannot occupy two output slots"
        );
        assert_eq!(
            error
                .details()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["binding", "first_slot"]
        );

        let encoded = serde_json::to_value(&error).unwrap();
        assert_eq!(encoded["category"], "invalid_plan");
        assert_eq!(encoded["code"], "duplicate_selection");
        assert_eq!(encoded["path"][2]["kind"], "output_slot");
    }
}
