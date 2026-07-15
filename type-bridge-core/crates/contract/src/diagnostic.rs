//! Stable structured diagnostics shared by contract parsers and codecs.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error as _;

/// Stable high-level contract failure categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    /// Contract bytes or values are malformed or internally inconsistent.
    InvalidContract,
    /// A required open capability is not advertised.
    UnsupportedCapability,
    /// A canonical structural ceiling was exceeded.
    ResourceLimit,
    /// An integrity algorithm, digest, or canonicalization contract failed.
    Integrity,
}

impl DiagnosticCategory {
    /// Return the stable language-neutral category spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidContract => "invalid_contract",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::ResourceLimit => "resource_limit",
            Self::Integrity => "integrity",
        }
    }
}

impl fmt::Display for DiagnosticCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error returned when a diagnostic code is not canonical snake case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticCodeError;

impl fmt::Display for DiagnosticCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("diagnostic code must be 1-128 lowercase snake-case bytes")
    }
}

impl Error for DiagnosticCodeError {}

/// A validated stable machine-readable diagnostic code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticCode(String);

impl DiagnosticCode {
    /// Validate and construct one code.
    pub fn new(value: impl Into<String>) -> Result<Self, DiagnosticCodeError> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = value.len() <= 128
            && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if valid { Ok(Self(value)) } else { Err(DiagnosticCodeError) }
    }

    /// Return the canonical code spelling.
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer { serializer.serialize_str(self.as_str()) }
}

impl<'de> Deserialize<'de> for DiagnosticCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// One typed segment in a contract diagnostic path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DiagnosticPathSegment {
    /// An object field.
    Field(String),
    /// A zero-based collection index.
    Index(u64),
    /// A typed identifier rendered for diagnostics.
    Identifier(String),
}

/// A typed path into one canonical contract value.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticPath(Vec<DiagnosticPathSegment>);

impl DiagnosticPath {
    /// Construct an empty root path.
    pub const fn new() -> Self { Self(Vec::new()) }
    /// Construct a path from typed segments.
    pub fn from_segments(segments: impl IntoIterator<Item = DiagnosticPathSegment>) -> Self {
        Self(segments.into_iter().collect())
    }
    /// Return the ordered segments.
    pub fn segments(&self) -> &[DiagnosticPathSegment] { &self.0 }
    /// Append one segment.
    pub fn push(&mut self, segment: DiagnosticPathSegment) { self.0.push(segment); }
}

/// A deterministic typed diagnostic detail value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticDetailValue {
    /// Textual context.
    Text(String),
    /// A signed integer encoded as a decimal string for binding safety.
    Long(i64),
    /// A boolean fact.
    Boolean(bool),
    /// An ordered list of text values.
    TextList(Vec<String>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum DetailWire {
    Text(String),
    Long(String),
    Boolean(bool),
    TextList(Vec<String>),
}

impl Serialize for DiagnosticDetailValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        let wire = match self {
            Self::Text(value) => DetailWire::Text(value.clone()),
            Self::Long(value) => DetailWire::Long(value.to_string()),
            Self::Boolean(value) => DetailWire::Boolean(*value),
            Self::TextList(value) => DetailWire::TextList(value.clone()),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DiagnosticDetailValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        match DetailWire::deserialize(deserializer)? {
            DetailWire::Text(value) => Ok(Self::Text(value)),
            DetailWire::Boolean(value) => Ok(Self::Boolean(value)),
            DetailWire::TextList(value) => Ok(Self::TextList(value)),
            DetailWire::Long(value) => {
                let parsed = value.parse::<i64>().map_err(D::Error::custom)?;
                if parsed.to_string() != value {
                    return Err(D::Error::custom("diagnostic long is not canonical"));
                }
                Ok(Self::Long(parsed))
            }
        }
    }
}

impl From<&str> for DiagnosticDetailValue {
    fn from(value: &str) -> Self { Self::Text(value.to_owned()) }
}
impl From<String> for DiagnosticDetailValue {
    fn from(value: String) -> Self { Self::Text(value) }
}
impl From<i64> for DiagnosticDetailValue {
    fn from(value: i64) -> Self { Self::Long(value) }
}
impl From<bool> for DiagnosticDetailValue {
    fn from(value: bool) -> Self { Self::Boolean(value) }
}
impl From<Vec<String>> for DiagnosticDetailValue {
    fn from(value: Vec<String>) -> Self { Self::TextList(value) }
}

/// One stable structured contract failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    category: DiagnosticCategory,
    code: DiagnosticCode,
    message: String,
    path: DiagnosticPath,
    details: BTreeMap<String, DiagnosticDetailValue>,
}

impl Diagnostic {
    /// Construct a diagnostic from a validated code.
    pub fn new(category: DiagnosticCategory, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self { category, code, message: message.into(), path: DiagnosticPath::new(), details: BTreeMap::new() }
    }

    /// Construct an implementation-owned diagnostic with a static valid code.
    pub(crate) fn stable(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Self {
        Self::new(category, DiagnosticCode::new(code).expect("static diagnostic code is valid"), message)
    }

    /// Attach a complete typed path.
    pub fn with_path(mut self, path: DiagnosticPath) -> Self { self.path = path; self }
    /// Append one path segment.
    pub fn at(mut self, segment: DiagnosticPathSegment) -> Self { self.path.push(segment); self }
    /// Attach one deterministic detail.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<DiagnosticDetailValue>) -> Self {
        self.details.insert(key.into(), value.into()); self
    }
    /// Return the stable category.
    pub const fn category(&self) -> DiagnosticCategory { self.category }
    /// Return the stable code.
    pub fn code(&self) -> &DiagnosticCode { &self.code }
    /// Return the human-readable message.
    pub fn message(&self) -> &str { &self.message }
    /// Return the typed path.
    pub fn path(&self) -> &DiagnosticPath { &self.path }
    /// Return deterministic details.
    pub fn details(&self) -> &BTreeMap<String, DiagnosticDetailValue> { &self.details }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.category, self.code, self.message)
    }
}

impl Error for Diagnostic {}
