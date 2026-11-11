//! Shared Python plan authority, diagnostics and bounded host values.

use pyo3::types::{PyBool, PyFloat};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::plan_scalar::{
    PlanScalarInput, plan_scalar, plan_scalar_host_type_error, plan_scalar_integer_range_error,
    plan_scalar_unicode_error,
};

use std::sync::Arc;

use pyo3::exceptions::{PyTypeError, PyUnicodeEncodeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyByteArray, PyBytes, PyDict, PyInt, PyString};
use pythonize::pythonize;
use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::{Fingerprint, MAX_FINGERPRINT_IDENTIFIER_BYTES};
use type_bridge_contract::limits::{MAX_CANONICAL_BYTES, MAX_CANONICAL_STRING_BYTES};
use type_bridge_contract::schema::encode_declared_schema;
use type_bridge_orm::plan_authority::QueryAuthority;
use type_bridge_orm::plan_scalar::host_string_unicode_error;
use type_bridge_schema::{
    MAX_SCHEMA_AUTHORITY_BYTES, SchemaAuthorityError, SchemaAuthorityErrorCode,
    decode_schema_authority, schema_authority_capability_vocabulary,
};

pyo3::create_exception!(
    type_bridge_core,
    PlanError,
    PyValueError,
    "Structured canonical plan diagnostic."
);

pub(crate) fn value_error(diagnostic: &Diagnostic) -> PyErr {
    Python::attach(|py| {
        let py_error = PlanError::new_err(format!(
            "{}: {}",
            diagnostic.code().as_str(),
            diagnostic.message(),
        ));
        // One attribute per key of the diagnostic's serde form, the same JSON
        // Node and the C ABI emit.
        let attach = || -> PyResult<()> {
            let value = py_error.value(py);
            let form = pythonize(py, diagnostic)?.cast_into::<PyDict>()?;
            for (key, item) in form.iter() {
                value.setattr(key.cast_into::<PyString>()?, item)?;
            }
            Ok(())
        };
        match attach() {
            Ok(()) => py_error,
            Err(attribute_error) => attribute_error,
        }
    })
}

fn binding_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static Python binding diagnostic code"),
        message,
    )
}

/// Opaque prepared-query schema authority handle.
#[pyclass(name = "PlanAuthority", frozen)]
pub struct PyPlanAuthority {
    pub(crate) authority: Arc<QueryAuthority>,
}

impl PyPlanAuthority {
    pub(crate) fn authority(&self) -> Arc<QueryAuthority> {
        Arc::clone(&self.authority)
    }
}

#[pymethods]
impl PyPlanAuthority {
    /// Derive an authority over the same schema whose plans are authored,
    /// decoded, and validated under `query_limits`.
    fn with_query_limits(&self, query_limits: &crate::match_runtime::PyQueryLimits) -> Self {
        Self {
            authority: Arc::new(self.authority.rebound_with_limits(query_limits.inner())),
        }
    }

    /// Build one authority from canonical declared-schema bytes.
    #[new]
    fn new(
        py: Python<'_>,
        declared_schema: &Bound<'_, PyAny>,
        scope: &Bound<'_, PyAny>,
        profile: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        build_plan_authority(py, declared_schema, scope, profile)
    }
}

pub(crate) enum PythonBytes<'py> {
    Bytes(Bound<'py, PyBytes>),
    ByteArray(Bound<'py, PyByteArray>),
}

impl<'py> PythonBytes<'py> {
    pub(crate) fn extract(value: &Bound<'py, PyAny>, argument: &'static str) -> PyResult<Self> {
        if let Ok(bytes) = value.cast::<PyBytes>() {
            Ok(Self::Bytes(bytes.to_owned()))
        } else if let Ok(bytes) = value.cast::<PyByteArray>() {
            Ok(Self::ByteArray(bytes.to_owned()))
        } else {
            Err(PyTypeError::new_err(format!(
                "argument '{argument}' must be bytes or bytearray"
            )))
        }
    }

    pub(crate) fn bounded_snapshot(&self, snapshot_limit: usize) -> Vec<u8> {
        match self {
            Self::Bytes(bytes) => {
                let bytes = bytes.as_bytes();
                bytes[..bytes.len().min(snapshot_limit)].to_vec()
            }
            Self::ByteArray(bytes) => {
                // SAFETY: the GIL remains held and the borrowed slice is used
                // only for this immediate bounded copy. It is never retained
                // across `allow_threads` or another call into Python.
                let bytes = unsafe { bytes.as_bytes() };
                bytes[..bytes.len().min(snapshot_limit)].to_vec()
            }
        }
    }
}

pub(crate) struct PythonString<'py>(Bound<'py, PyString>);

impl<'py> PythonString<'py> {
    pub(crate) fn extract(value: &Bound<'py, PyAny>, argument: &'static str) -> PyResult<Self> {
        value
            .cast::<PyString>()
            .map(|value| Self(value.to_owned()))
            .map_err(|_| PyTypeError::new_err(format!("argument '{argument}' must be str")))
    }

    /// Copy at most the accepted byte ceiling, or one deterministic byte
    /// beyond it so the Rust-owned semantic engine emits its canonical limit
    /// diagnostic before parsing.
    pub(crate) fn bounded_snapshot(&self, max_bytes: usize) -> PyResult<String> {
        self.bounded_snapshot_with_unicode_error(max_bytes, host_string_unicode_error)
    }

    pub(crate) fn bounded_snapshot_with_unicode_error(
        &self,
        max_bytes: usize,
        invalid_unicode: impl FnOnce() -> Diagnostic,
    ) -> PyResult<String> {
        // `PyUnicode_AsUTF8AndSize` may materialize CPython's cached UTF-8
        // representation. Reject obviously oversized Unicode values first so
        // even that host-owned cache is bounded by four bytes per admitted
        // code point.
        let code_points = unsafe { ffi::PyUnicode_GetLength(self.0.as_ptr()) };
        if code_points < 0 {
            return Err(PyErr::fetch(self.0.py()));
        }
        if usize::try_from(code_points).unwrap_or(usize::MAX) > max_bytes {
            return Ok(" ".repeat(max_bytes.saturating_add(1)));
        }

        let value = self.0.to_str().map_err(|error| {
            if error.is_instance_of::<PyUnicodeEncodeError>(self.0.py()) {
                value_error(&invalid_unicode())
            } else {
                error
            }
        })?;
        if value.len() > max_bytes {
            Ok(" ".repeat(max_bytes.saturating_add(1)))
        } else {
            Ok(value.to_owned())
        }
    }
}

fn build_plan_authority(
    py: Python<'_>,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyPlanAuthority> {
    let declared_schema = PythonBytes::extract(declared_schema, "declared_schema")?;
    let scope = PythonString::extract(scope, "scope")?;
    let profile = PythonString::extract(profile, "profile")?;
    let declared_schema = declared_schema.bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    let scope = scope.bounded_snapshot(MAX_CANONICAL_STRING_BYTES)?;
    let profile = profile.bounded_snapshot(MAX_FINGERPRINT_IDENTIFIER_BYTES)?;
    let authority = py
        .detach(move || QueryAuthority::from_declared_bytes(&declared_schema, &scope, &profile))
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyPlanAuthority {
        authority: Arc::new(authority),
    })
}

pub(crate) fn schema_authority_diagnostic(error: &SchemaAuthorityError) -> Diagnostic {
    if let Some(contract) = error.contract() {
        return contract.clone();
    }
    let (category, code, message) = match error.code() {
        SchemaAuthorityErrorCode::Contract => (
            DiagnosticCategory::InvalidContract,
            "generated_schema_authority_invalid",
            "generated schema authority is not a valid canonical contract",
        ),
        SchemaAuthorityErrorCode::Schema => (
            DiagnosticCategory::InvalidContract,
            "generated_schema_authority_schema_invalid",
            "generated schema authority cannot reconstruct its declared schema",
        ),
        SchemaAuthorityErrorCode::UnsupportedVersion => (
            DiagnosticCategory::InvalidContract,
            "generated_schema_authority_unsupported_version",
            "generated schema authority uses an unsupported version",
        ),
        SchemaAuthorityErrorCode::UnsupportedCapability => (
            DiagnosticCategory::UnsupportedCapability,
            "generated_schema_authority_unsupported_capability",
            "generated schema authority requires an unsupported capability",
        ),
        SchemaAuthorityErrorCode::ResourceLimit => (
            DiagnosticCategory::ResourceLimit,
            "generated_schema_authority_resource_limit",
            "generated schema authority exceeds a structural limit",
        ),
        SchemaAuthorityErrorCode::IntegrityMismatch => (
            DiagnosticCategory::Integrity,
            "generated_schema_authority_integrity_mismatch",
            "generated schema authority contains stale or mismatched evidence",
        ),
    };
    binding_diagnostic(category, code, message)
}

fn build_plan_authority_from_schema_authority(
    schema_authority: &[u8],
    semantic_fingerprint: &[u8],
) -> Result<PyPlanAuthority, Diagnostic> {
    let verified =
        decode_schema_authority(schema_authority, &schema_authority_capability_vocabulary())
            .map_err(|error| schema_authority_diagnostic(&error))?;
    let expected: Fingerprint = from_canonical_json(semantic_fingerprint)?;
    if &expected
        != verified
            .resolved_schema()
            .semantic_fingerprint()
            .as_fingerprint()
    {
        return Err(binding_diagnostic(
            DiagnosticCategory::Integrity,
            "generated_schema_authority_semantic_mismatch",
            "generated schema authority does not belong to the installed runtime projection",
        ));
    }
    if to_canonical_json(verified.resolved_schema().semantic_fingerprint())? != semantic_fingerprint
    {
        return Err(binding_diagnostic(
            DiagnosticCategory::InvalidContract,
            "generated_schema_authority_semantic_noncanonical",
            "generated semantic fingerprint is not canonical",
        ));
    }
    let declared = encode_declared_schema(verified.declared_schema())?;
    let authority = QueryAuthority::from_declared_bytes(
        &declared,
        verified.managed_scope().id().as_str(),
        verified.semantic_profile().id().as_str(),
    )?;
    Ok(PyPlanAuthority {
        authority: Arc::new(authority),
    })
}

/// Build one authority from canonical declared-schema bytes.
#[pyfunction]
pub fn plan_authority(
    py: Python<'_>,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyPlanAuthority> {
    build_plan_authority(py, declared_schema, scope, profile)
}

/// Verify one generated package's complete schema-authority envelope.
#[pyfunction]
pub fn plan_authority_from_schema_authority(
    py: Python<'_>,
    schema_authority: &Bound<'_, PyAny>,
    semantic_fingerprint: &Bound<'_, PyAny>,
) -> PyResult<PyPlanAuthority> {
    let schema_authority = PythonBytes::extract(schema_authority, "schema_authority")?
        .bounded_snapshot(MAX_SCHEMA_AUTHORITY_BYTES.saturating_add(1));
    let semantic_fingerprint = PythonBytes::extract(semantic_fingerprint, "semantic_fingerprint")?
        .bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    py.detach(move || {
        build_plan_authority_from_schema_authority(&schema_authority, &semantic_fingerprint)
    })
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Register shared plan authority and diagnostics on the native module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("PlanError", m.py().get_type::<PlanError>())?;
    m.add_class::<PyPlanAuthority>()?;
    m.add_function(wrap_pyfunction!(plan_authority, m)?)?;
    m.add_function(wrap_pyfunction!(plan_authority_from_schema_authority, m)?)?;
    Ok(())
}

pub(crate) fn python_scalar(
    value_type: ValueTypeTag,
    value: &Bound<'_, PyAny>,
) -> PyResult<CanonicalValue> {
    let input = match value_type {
        ValueTypeTag::String
        | ValueTypeTag::Date
        | ValueTypeTag::DateTime
        | ValueTypeTag::DateTimeTz
        | ValueTypeTag::Decimal
        | ValueTypeTag::Duration => {
            let value = PythonString::extract(value, "value")
                .map_err(|_| value_error(&plan_scalar_host_type_error()))?
                .bounded_snapshot_with_unicode_error(
                    MAX_CANONICAL_STRING_BYTES,
                    plan_scalar_unicode_error,
                )?;
            PlanScalarInput::Text(value)
        }
        ValueTypeTag::Long => {
            let integer = value
                .cast_exact::<PyInt>()
                .map_err(|_| value_error(&plan_scalar_host_type_error()))?;
            let integer = integer
                .extract::<i64>()
                .map_err(|_| value_error(&plan_scalar_integer_range_error()))?;
            PlanScalarInput::Long(integer)
        }
        ValueTypeTag::Double => {
            let number = value
                .cast_exact::<PyFloat>()
                .map_err(|_| value_error(&plan_scalar_host_type_error()))?;
            PlanScalarInput::Double(number.extract::<f64>()?)
        }
        ValueTypeTag::Boolean => {
            let boolean = value
                .cast_exact::<PyBool>()
                .map_err(|_| value_error(&plan_scalar_host_type_error()))?;
            PlanScalarInput::Boolean(boolean.extract::<bool>()?)
        }
    };
    plan_scalar(value_type, input).map_err(|error| value_error(&error))
}

#[cfg(test)]
mod tests {
    use pyo3::prelude::*;
    use pyo3::types::{PyByteArray, PyBytes, PyString};
    use pythonize::depythonize;

    use super::{
        MAX_FINGERPRINT_IDENTIFIER_BYTES, PlanError, PyPlanAuthority, PythonBytes, PythonString,
        build_plan_authority_from_schema_authority, value_error,
    };
    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPathSegment,
    };
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::limits::{
        MAX_CANONICAL_STRING_BYTES, MAX_QUERY_INVOCATION_BYTES, MAX_REMOTE_ENVELOPE_BYTES,
    };
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::query_plan::PlanBuilder;
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
        encode_declared_schema,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        build_schema_authority, encode_schema_authority, normalize_documents,
    };

    fn build_person_plan(authority: &type_bridge_orm::plan_authority::QueryAuthority) {
        let mut builder = PlanBuilder::new();
        let person = builder.exact(TypeId::new(TypeKind::Entity, "person").unwrap());
        builder.returning(vec![person]);
        builder
            .build(
                authority.declared(),
                authority
                    .context()
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
            )
            .expect("verified authority drives the native plan builder");
    }

    fn generated_authority_fixture(source: &str) -> (Vec<u8>, Vec<u8>) {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("python-generated-authority").expect("document"),
            source,
        )])
        .expect("schema parses");
        let declared = normalize_documents(&documents).expect("schema normalizes");
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|id| CapabilityId::new(*id).expect("capability"))
            .collect();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("python-generated-authority").expect("scope"),
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            available,
        );
        let authority =
            build_schema_authority(&declared, declared.required_capabilities(), &context)
                .expect("authority builds");
        let semantic =
            to_canonical_json(authority.resolved_schema().semantic_fingerprint()).unwrap();
        (encode_schema_authority(&authority), semantic)
    }

    #[test]
    fn generated_authority_constructor_verifies_envelope_and_projection_binding() {
        let (authority, semantic) =
            generated_authority_fixture("format: typebridge.schema/v2\nentities:\n  person: {}\n");
        let installed = build_plan_authority_from_schema_authority(&authority, &semantic)
            .expect("verified generated authority installs");
        build_person_plan(&installed.authority());

        let (_, foreign_semantic) =
            generated_authority_fixture("format: typebridge.schema/v2\nentities:\n  robot: {}\n");
        assert_eq!(
            build_plan_authority_from_schema_authority(&authority, &foreign_semantic)
                .err()
                .expect("foreign projection fingerprint must fail")
                .code()
                .as_str(),
            "generated_schema_authority_semantic_mismatch",
        );
        assert_eq!(
            build_plan_authority_from_schema_authority(b"{", &semantic)
                .err()
                .expect("malformed envelope must fail")
                .code()
                .as_str(),
            "malformed_canonical_json",
        );
        assert_eq!(
            build_plan_authority_from_schema_authority(
                &vec![b' '; type_bridge_schema::MAX_SCHEMA_AUTHORITY_BYTES + 1],
                &semantic,
            )
            .err()
            .expect("oversized envelope must fail")
            .code()
            .as_str(),
            "canonical_json_too_large",
        );
    }

    #[test]
    fn structured_query_error_preserves_typed_path_and_detail_values() {
        let diagnostic = Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new("query_fixture").expect("diagnostic code"),
            "fixture diagnostic",
        )
        .at(DiagnosticPathSegment::Field("patterns".to_owned()))
        .at(DiagnosticPathSegment::Index(2))
        .at(DiagnosticPathSegment::Identifier("person".to_owned()))
        .with_detail("boolean", true)
        .with_detail("long", i64::MIN)
        .with_detail("text", "context")
        .with_detail("text_list", vec!["a".to_owned(), "b".to_owned()]);

        Python::initialize();
        let error = value_error(&diagnostic);
        Python::attach(|py| {
            assert!(error.is_instance_of::<PlanError>(py));
            let value = error.value(py);
            assert_eq!(
                value
                    .getattr("category")
                    .expect("category")
                    .extract::<String>()
                    .expect("category string"),
                "invalid_contract",
            );
            assert_eq!(
                value
                    .getattr("code")
                    .expect("code")
                    .extract::<String>()
                    .expect("code string"),
                "query_fixture",
            );
            assert_eq!(
                value
                    .getattr("message")
                    .expect("message")
                    .extract::<String>()
                    .expect("message string"),
                "fixture diagnostic",
            );
            let path: serde_json::Value =
                depythonize(&value.getattr("path").expect("path")).expect("JSON path");
            assert_eq!(
                path,
                serde_json::json!([
                    {"kind": "field", "value": "patterns"},
                    {"kind": "index", "value": 2},
                    {"kind": "identifier", "value": "person"},
                ]),
            );
            let details: serde_json::Value =
                depythonize(&value.getattr("details").expect("details")).expect("JSON details");
            assert_eq!(
                details,
                serde_json::json!({
                    "boolean": {"kind": "boolean", "value": true},
                    "long": {"kind": "long", "value": "-9223372036854775808"},
                    "text": {"kind": "text", "value": "context"},
                    "text_list": {"kind": "text_list", "value": ["a", "b"]},
                }),
            );
        });
    }

    #[test]
    fn authority_class_constructor_uses_the_existing_canonical_authority_path() {
        let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
        let fact = SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person).expect("type fact")),
            SourceSpan::new(
                DocumentId::new("python-authority-constructor").expect("document"),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .expect("source"),
        );
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, Default::default(), [fact])
            .expect("declared schema");
        let bytes = encode_declared_schema(&declared).expect("declared bytes");

        Python::initialize();
        Python::attach(|py| {
            let authority = PyPlanAuthority::new(
                py,
                PyBytes::new(py, &bytes).as_any(),
                PyString::new(py, "python-authority-constructor").as_any(),
                PyString::new(py, "typedb-3.12.1/v1").as_any(),
            )
            .expect("public class constructor");
            build_person_plan(&authority.authority());
        });
    }

    #[test]
    fn reply_snapshots_are_bounded_for_bytes_and_bytearrays() {
        Python::initialize();
        Python::attach(|py| {
            let payload = vec![0x5a; 4_096];
            let bytes = PyBytes::new(py, &payload);
            assert_eq!(
                PythonBytes::extract(bytes.as_any(), "response")
                    .expect("bytes response")
                    .bounded_snapshot(257),
                payload[..257],
            );

            let bytearray = PyByteArray::new(py, &payload);
            assert_eq!(
                PythonBytes::extract(bytearray.as_any(), "response")
                    .expect("bytearray response")
                    .bounded_snapshot(257),
                payload[..257],
            );
        });
    }

    #[test]
    fn reply_snapshot_copies_only_the_hard_ceiling_oversize_marker() {
        Python::initialize();
        Python::attach(|py| {
            let payload = vec![0x5a; MAX_REMOTE_ENVELOPE_BYTES + 4_096];
            let bytes = PyBytes::new(py, &payload);
            let snapshot = PythonBytes::extract(bytes.as_any(), "response")
                .expect("bytes response")
                .bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
            assert_eq!(snapshot.len(), MAX_REMOTE_ENVELOPE_BYTES + 1);
            assert_eq!(snapshot, payload[..MAX_REMOTE_ENVELOPE_BYTES + 1]);
        });
    }

    #[test]
    fn invocation_snapshot_owns_only_the_ceiling_oversize_marker() {
        Python::initialize();
        Python::attach(|py| {
            let oversized = "x".repeat(MAX_QUERY_INVOCATION_BYTES + 4_096);
            let value = PyString::new(py, &oversized);
            let snapshot = PythonString::extract(value.as_any(), "invocation_json")
                .expect("string input")
                .bounded_snapshot(MAX_QUERY_INVOCATION_BYTES)
                .expect("UTF-8 input");
            assert_eq!(snapshot.len(), MAX_QUERY_INVOCATION_BYTES + 1);
            assert!(snapshot.bytes().all(|byte| byte == b' '));

            let exact = "x".repeat(MAX_QUERY_INVOCATION_BYTES);
            let value = PyString::new(py, &exact);
            let snapshot = PythonString::extract(value.as_any(), "invocation_json")
                .expect("string input")
                .bounded_snapshot(MAX_QUERY_INVOCATION_BYTES)
                .expect("UTF-8 input");
            assert_eq!(snapshot, exact);
        });
    }

    #[test]
    fn authority_strings_are_bounded_before_downstream_identity_validation() {
        Python::initialize();
        Python::attach(|py| {
            let exact = "x".repeat(MAX_CANONICAL_STRING_BYTES);
            let value = PyString::new(py, &exact);
            let snapshot = PythonString::extract(value.as_any(), "scope")
                .expect("scope string")
                .bounded_snapshot(MAX_CANONICAL_STRING_BYTES)
                .expect("UTF-8 scope");
            assert_eq!(snapshot, exact);

            let oversized = "x".repeat(MAX_CANONICAL_STRING_BYTES + 4_096);
            let value = PyString::new(py, &oversized);
            let scope = PythonString::extract(value.as_any(), "scope")
                .expect("scope string")
                .bounded_snapshot(MAX_CANONICAL_STRING_BYTES)
                .expect("UTF-8 scope");
            assert_eq!(scope.len(), MAX_CANONICAL_STRING_BYTES + 1);
            assert_eq!(
                ManagedScopeId::new(scope)
                    .expect_err("oversized scope marker")
                    .code()
                    .as_str(),
                "malformed_managed_scope_id",
            );

            let exact_profile = format!(
                "{}/v1",
                "a".repeat(MAX_FINGERPRINT_IDENTIFIER_BYTES - "/v1".len())
            );
            let exact_value = PyString::new(py, &exact_profile);
            let profile = PythonString::extract(exact_value.as_any(), "profile")
                .expect("profile string")
                .bounded_snapshot(MAX_FINGERPRINT_IDENTIFIER_BYTES)
                .expect("UTF-8 profile");
            assert_eq!(profile, exact_profile);
            SemanticProfileId::new(profile).expect("exact profile ceiling is valid");

            let profile = PythonString::extract(value.as_any(), "profile")
                .expect("profile string")
                .bounded_snapshot(MAX_FINGERPRINT_IDENTIFIER_BYTES)
                .expect("UTF-8 profile");
            assert_eq!(profile.len(), MAX_FINGERPRINT_IDENTIFIER_BYTES + 1);
            assert_eq!(
                SemanticProfileId::new(profile)
                    .expect_err("oversized profile marker")
                    .code()
                    .as_str(),
                "invalid_fingerprint_identifier",
            );
        });
    }
}
