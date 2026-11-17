//! Strict bounded structured diagnostics shared by remote plan transports.

use crate::codec::to_canonical_json;
use crate::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticDetailValue, DiagnosticPath,
    DiagnosticPathSegment,
};
use crate::limits::MAX_DIAGNOSTIC_BYTES;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A strict representation of one complete structured diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDiagnostic {
    pub(crate) category: DiagnosticCategory,
    pub(crate) code: DiagnosticCode,
    pub(crate) details: BTreeMap<String, RemoteDiagnosticDetail>,
    pub(crate) message: String,
    pub(crate) path: Vec<RemoteDiagnosticPathSegment>,
}

impl RemoteDiagnostic {
    /// Preserve all diagnostic fields in their typed representation.
    pub fn new(diagnostic: &Diagnostic) -> Result<Self, Diagnostic> {
        let details = diagnostic
            .details()
            .iter()
            .map(|(key, value)| {
                validate_detail_key(key)?;
                Ok((key.clone(), RemoteDiagnosticDetail::from(value)))
            })
            .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
        let value = Self {
            category: diagnostic.category(),
            code: diagnostic.code().clone(),
            details,
            message: diagnostic.message().to_owned(),
            path: diagnostic
                .path()
                .segments()
                .iter()
                .map(RemoteDiagnosticPathSegment::from)
                .collect(),
        };
        value.validate_size()?;
        Ok(value)
    }

    /// Reconstruct the exact structured diagnostic.
    pub fn diagnostic(&self) -> Result<Diagnostic, Diagnostic> {
        self.validate_size()?;
        let path = DiagnosticPath::from_segments(
            self.path
                .iter()
                .map(DiagnosticPathSegment::from)
                .collect::<Vec<_>>(),
        );
        let mut diagnostic =
            Diagnostic::new(self.category, self.code.clone(), self.message.clone()).with_path(path);
        for (key, value) in &self.details {
            validate_detail_key(key)?;
            diagnostic = diagnostic.with_detail(key.clone(), detail_value_from_wire(value)?);
        }
        Ok(diagnostic)
    }

    pub(crate) fn validate_size(&self) -> Result<(), Diagnostic> {
        validate_remote_diagnostic_wire(self)?;
        let bytes = to_canonical_json(self)?;
        if bytes.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_diagnostic_too_large",
                "V2 remote diagnostic exceeds the canonical byte ceiling",
            ));
        }
        Ok(())
    }
}

/// One strict typed diagnostic path segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteDiagnosticPathSegment {
    /// An object field.
    Field(String),
    /// A zero-based collection index.
    Index(u64),
    /// A typed semantic identifier.
    Identifier(String),
}

impl From<&DiagnosticPathSegment> for RemoteDiagnosticPathSegment {
    fn from(value: &DiagnosticPathSegment) -> Self {
        match value {
            DiagnosticPathSegment::Field(value) => Self::Field(value.clone()),
            DiagnosticPathSegment::Index(value) => Self::Index(*value),
            DiagnosticPathSegment::Identifier(value) => Self::Identifier(value.clone()),
        }
    }
}

impl From<&RemoteDiagnosticPathSegment> for DiagnosticPathSegment {
    fn from(value: &RemoteDiagnosticPathSegment) -> Self {
        match value {
            RemoteDiagnosticPathSegment::Field(value) => Self::Field(value.clone()),
            RemoteDiagnosticPathSegment::Index(value) => Self::Index(*value),
            RemoteDiagnosticPathSegment::Identifier(value) => Self::Identifier(value.clone()),
        }
    }
}

/// One strict typed diagnostic detail value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteDiagnosticDetail {
    /// Text context.
    Text(String),
    /// Signed long, encoded canonically as decimal text for binding safety.
    Long(String),
    /// Boolean context.
    Boolean(bool),
    /// Ordered text values.
    TextList(Vec<String>),
}

impl From<&DiagnosticDetailValue> for RemoteDiagnosticDetail {
    fn from(value: &DiagnosticDetailValue) -> Self {
        match value {
            DiagnosticDetailValue::Text(value) => Self::Text(value.clone()),
            DiagnosticDetailValue::Long(value) => Self::Long(value.to_string()),
            DiagnosticDetailValue::Boolean(value) => Self::Boolean(*value),
            DiagnosticDetailValue::TextList(value) => Self::TextList(value.clone()),
        }
    }
}

fn validate_detail_key(key: &str) -> Result<(), Diagnostic> {
    DiagnosticCode::new(key.to_owned())
        .map(|_| ())
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_diagnostic_detail_key_invalid",
                "V2 diagnostic detail keys must be canonical snake-case identifiers",
            )
        })
}

fn detail_value_from_wire(
    value: &RemoteDiagnosticDetail,
) -> Result<DiagnosticDetailValue, Diagnostic> {
    match value {
        RemoteDiagnosticDetail::Text(value) => Ok(DiagnosticDetailValue::Text(value.clone())),
        RemoteDiagnosticDetail::Long(value) => {
            let parsed = value.parse::<i64>().map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_diagnostic_long_invalid",
                    "V2 diagnostic long detail is outside the signed 64-bit range",
                )
            })?;
            if parsed.to_string() != *value {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_diagnostic_long_invalid",
                    "V2 diagnostic long detail is not canonical decimal text",
                ));
            }
            Ok(DiagnosticDetailValue::Long(parsed))
        }
        RemoteDiagnosticDetail::Boolean(value) => Ok(DiagnosticDetailValue::Boolean(*value)),
        RemoteDiagnosticDetail::TextList(value) => {
            Ok(DiagnosticDetailValue::TextList(value.clone()))
        }
    }
}

pub(crate) fn validate_remote_diagnostic_wire(value: &RemoteDiagnostic) -> Result<(), Diagnostic> {
    for key in value.details.keys() {
        validate_detail_key(key)?;
    }
    for detail in value.details.values() {
        detail_value_from_wire(detail)?;
    }
    Ok(())
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static remote diagnostic code"),
        message,
    )
}
