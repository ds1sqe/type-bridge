//! Python projection of the prepared V2 query facade.
//!
//! Exactly three things cross this boundary: canonical declared-schema
//! bytes (once, into an opaque authority handle), canonical plan bytes,
//! and small JSON payloads for invocations and typed outcomes. Local
//! execution and the remote envelope share one authority, so a prepared
//! plan runs identically through either path.
//!
//! Typed Python authoring delegates every transition to the shared Rust
//! builder and passes its canonical plan bytes directly into this surface.

use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyUnicodeEncodeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyByteArray, PyBytes, PyInt, PyString};
use pythonize::pythonize;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::{
    MAX_CANONICAL_BYTES, MAX_CANONICAL_STRING_BYTES, MAX_QUERY_INVOCATION_BYTES,
    MAX_REMOTE_ENVELOPE_BYTES,
};
use type_bridge_contract::query_remote::{
    RemoteLimits, checked_remote_deadline, checked_remote_limit, remote_deadline_limit,
    remote_limit_invalid,
};
use type_bridge_orm::query_v2_prepared::{
    PendingRemoteQuery, QueryAuthority, decode_remote_capabilities, execute_prepared_local,
    prepare_remote_query, query_v2_host_string_unicode_error,
};
use type_bridge_orm::session::backend::{BoundedAnswerLimits, QueryV2AnswerLimits};

use crate::orm_runtime::{PyRustDatabase, provider_block_on};

pyo3::create_exception!(
    type_bridge_core,
    QueryV2Error,
    PyValueError,
    "Structured canonical V2 query diagnostic."
);

pub(crate) fn value_error(diagnostic: &Diagnostic) -> PyErr {
    Python::attach(|py| {
        let py_error = QueryV2Error::new_err(format!(
            "{}: {}",
            diagnostic.code().as_str(),
            diagnostic.message(),
        ));
        let attach = || -> PyResult<()> {
            let value = py_error.value(py);
            value.setattr("category", diagnostic.category().as_str())?;
            value.setattr("code", diagnostic.code().as_str())?;
            value.setattr("message", diagnostic.message())?;
            value.setattr("path", pythonize(py, diagnostic.path().segments())?)?;
            value.setattr("details", pythonize(py, diagnostic.details())?)?;
            Ok(())
        };
        match attach() {
            Ok(()) => py_error,
            Err(attribute_error) => attribute_error,
        }
    })
}

const LOCAL_WORKER_FAILED: &str =
    "query_v2_local_worker_failed: local provider worker terminated unexpectedly";
// `SemanticProfileId` freezes this tighter identifier ceiling inside the
// contract validator; mirror it at the FFI ownership boundary so Python and
// Node never allocate beyond the value that validator can admit.
const MAX_SEMANTIC_PROFILE_ID_BYTES: usize = 255;

fn contain_local_worker_panic<T>(worker: impl FnOnce() -> T) -> PyResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker))
        .map_err(|_| PyRuntimeError::new_err(LOCAL_WORKER_FAILED))
}

/// Opaque prepared-query schema authority handle.
#[pyclass(name = "QueryV2Authority", frozen)]
pub struct PyQueryV2Authority {
    authority: Arc<QueryAuthority>,
}

impl PyQueryV2Authority {
    pub(crate) fn authority(&self) -> Arc<QueryAuthority> {
        Arc::clone(&self.authority)
    }
}

#[pymethods]
impl PyQueryV2Authority {
    /// Build one authority from canonical declared-schema bytes.
    #[new]
    fn new(
        py: Python<'_>,
        declared_schema: &Bound<'_, PyAny>,
        scope: &Bound<'_, PyAny>,
        profile: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        build_query_v2_authority(py, declared_schema, scope, profile)
    }

    /// Build a local-only authority for a database with no migration controls.
    #[staticmethod]
    fn query_only(
        py: Python<'_>,
        database: &PyRustDatabase,
        declared_schema: &Bound<'_, PyAny>,
        scope: &Bound<'_, PyAny>,
        profile: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        build_query_v2_query_only_authority(py, database, declared_schema, scope, profile)
    }
}

/// One prepared request with an atomic one-shot reply decoder.
#[pyclass(name = "PendingQueryV2Remote", frozen)]
pub struct PyPendingQueryV2Remote {
    pending: PendingRemoteQuery,
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
        self.bounded_snapshot_with_unicode_error(max_bytes, query_v2_host_string_unicode_error)
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

#[pymethods]
impl PyPendingQueryV2Remote {
    /// Return the exact canonical bytes to send to `/v2/query`.
    fn request_bytes(&self) -> Vec<u8> {
        self.pending.request_bytes().to_vec()
    }

    /// Consume the only reply accepted and verify its request binding.
    ///
    /// The winner snapshots at most one byte beyond the protocol hard ceiling;
    /// the caller's success-byte budget applies only after authentication and
    /// reply-kind classification, so it cannot truncate failure evidence.
    /// Parsing, evidence validation, and outcome serialization then run without
    /// the GIL so a maximal envelope cannot stall unrelated Python threads.
    fn decode_reply(&self, py: Python<'_>, response: &Bound<'_, PyAny>) -> PyResult<String> {
        let claimed = self
            .pending
            .claim_reply()
            .map_err(|diagnostic| value_error(&diagnostic))?;
        let response = PythonBytes::extract(response, "response")?;
        let response = response.bounded_snapshot(claimed.response_snapshot_limit());
        py.detach(move || claimed.decode(&response))
            .map_err(|diagnostic| value_error(&diagnostic))
    }
}

fn build_query_v2_authority(
    py: Python<'_>,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyQueryV2Authority> {
    let declared_schema = PythonBytes::extract(declared_schema, "declared_schema")?;
    let scope = PythonString::extract(scope, "scope")?;
    let profile = PythonString::extract(profile, "profile")?;
    let declared_schema = declared_schema.bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    let scope = scope.bounded_snapshot(MAX_CANONICAL_STRING_BYTES)?;
    let profile = profile.bounded_snapshot(MAX_SEMANTIC_PROFILE_ID_BYTES)?;
    let authority = py
        .detach(move || QueryAuthority::from_declared_bytes(&declared_schema, &scope, &profile))
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyQueryV2Authority {
        authority: Arc::new(authority),
    })
}

fn build_query_v2_query_only_authority(
    py: Python<'_>,
    database: &PyRustDatabase,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyQueryV2Authority> {
    let declared_schema = PythonBytes::extract(declared_schema, "declared_schema")?;
    let scope = PythonString::extract(scope, "scope")?;
    let profile = PythonString::extract(profile, "profile")?;
    let declared_schema = declared_schema.bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    let scope = scope.bounded_snapshot(MAX_CANONICAL_STRING_BYTES)?;
    let profile = profile.bounded_snapshot(MAX_SEMANTIC_PROFILE_ID_BYTES)?;
    let (database, _) = database.handles();
    let authority = py
        .detach(move || {
            QueryAuthority::from_declared_bytes_query_only(
                &declared_schema,
                &scope,
                &profile,
                &database,
            )
        })
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyQueryV2Authority {
        authority: Arc::new(authority),
    })
}

/// Build one authority from canonical declared-schema bytes.
#[pyfunction]
pub fn query_v2_authority(
    py: Python<'_>,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyQueryV2Authority> {
    build_query_v2_authority(py, declared_schema, scope, profile)
}

/// Build a local-only authority for a database with no migration controls.
#[pyfunction]
pub fn query_v2_query_only_authority(
    py: Python<'_>,
    database: &PyRustDatabase,
    declared_schema: &Bound<'_, PyAny>,
    scope: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
) -> PyResult<PyQueryV2Authority> {
    build_query_v2_query_only_authority(py, database, declared_schema, scope, profile)
}

/// Build the local execution budget from a checked optional deadline.
pub(crate) fn python_limit(value: &Bound<'_, PyAny>) -> Result<i128, Diagnostic> {
    let integer = value
        .cast_exact::<PyInt>()
        .map_err(|_| remote_limit_invalid())?;
    integer
        .extract::<u64>()
        .map(i128::from)
        .map_err(|_| remote_limit_invalid())
}

pub(crate) fn python_optional_limit(
    value: Option<&Bound<'_, PyAny>>,
) -> Result<Option<i128>, Diagnostic> {
    value.map(python_limit).transpose()
}

fn local_limits(deadline_ms: Option<&Bound<'_, PyAny>>) -> PyResult<QueryV2AnswerLimits> {
    let deadline = checked_remote_deadline(
        python_optional_limit(deadline_ms).map_err(|diagnostic| value_error(&diagnostic))?,
    )
    .map_err(|diagnostic| value_error(&diagnostic))?
    .map(|ms| {
        std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(ms))
            .ok_or_else(remote_deadline_limit)
    })
    .transpose()
    .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            deadline,
            ..BoundedAnswerLimits::default()
        },
        ..QueryV2AnswerLimits::default()
    })
}

/// Execute one prepared plan locally; returns typed outcome JSON.
///
/// The GIL is released for the whole provider round trip, and the
/// optional deadline bounds it — a stalled provider cannot pin other
/// Python threads for longer than the caller allows.
#[pyfunction]
#[pyo3(signature = (database, authority, plan, invocation_json, deadline_ms=None))]
pub fn query_v2_execute_local(
    py: Python<'_>,
    database: &PyRustDatabase,
    authority: &PyQueryV2Authority,
    plan: &Bound<'_, PyAny>,
    invocation_json: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<String> {
    let plan = PythonBytes::extract(plan, "plan")?;
    let invocation_json = PythonString::extract(invocation_json, "invocation_json")?;
    let limits = local_limits(deadline_ms)?;
    let plan = plan.bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    let invocation_json = invocation_json.bounded_snapshot(MAX_QUERY_INVOCATION_BYTES)?;
    let (db, runtime) = database.handles();
    let authority = Arc::clone(&authority.authority);
    contain_local_worker_panic(|| {
        provider_block_on(
            py,
            runtime.as_ref(),
            execute_prepared_local(&db, &authority, &plan, &invocation_json, limits),
        )
    })?
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Build the exact remote limit set from checked caller arguments.
///
/// Limits accept any Python integer and convert through the shared
/// contract range check, so a negative or oversized budget fails with
/// the same stable diagnostic the Node binding reports.
fn remote_limits(
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<RemoteLimits> {
    let build = || -> Result<RemoteLimits, Diagnostic> {
        Ok(RemoteLimits {
            deadline_ms: checked_remote_deadline(python_optional_limit(deadline_ms)?)?,
            max_bytes: checked_remote_limit(python_limit(max_bytes)?)?,
            max_items: checked_remote_limit(python_limit(max_items)?)?,
            max_collection_members: checked_remote_limit(python_limit(max_collection_members)?)?,
        })
    };
    build().map_err(|diagnostic| value_error(&diagnostic))
}

/// Decode one capability advertisement into its sorted capability ids.
#[pyfunction]
pub fn query_v2_remote_capabilities(
    py: Python<'_>,
    advertisement: &Bound<'_, PyAny>,
) -> PyResult<Vec<String>> {
    let advertisement = PythonBytes::extract(advertisement, "advertisement")?
        .bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    py.detach(move || decode_remote_capabilities(&advertisement))
        .map_err(|diagnostic| value_error(&diagnostic))
}

/// Prepare one remote invocation and its one-shot reply decoder.
///
/// `advertisement` carries the executor's exact `/v2/capabilities`
/// bytes; a plan or multi-row invocation the executor cannot execute is
/// refused here, before any request bytes exist.
/// `max_bytes` limits successful signed response bytes; authenticated failure
/// envelopes remain decodable under the protocol hard ceiling.
#[pyfunction]
#[pyo3(signature = (authority, plan, invocation_json, advertisement, max_items, max_bytes, max_collection_members, deadline_ms=None))]
#[expect(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_prepare_remote(
    py: Python<'_>,
    authority: &PyQueryV2Authority,
    plan: &Bound<'_, PyAny>,
    invocation_json: &Bound<'_, PyAny>,
    advertisement: &Bound<'_, PyAny>,
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyPendingQueryV2Remote> {
    let plan = PythonBytes::extract(plan, "plan")?;
    let invocation_json = PythonString::extract(invocation_json, "invocation_json")?;
    let advertisement = PythonBytes::extract(advertisement, "advertisement")?;
    let limits = remote_limits(max_items, max_bytes, max_collection_members, deadline_ms)?;
    let plan = plan.bounded_snapshot(MAX_CANONICAL_BYTES.saturating_add(1));
    let invocation_json = invocation_json.bounded_snapshot(MAX_QUERY_INVOCATION_BYTES)?;
    let advertisement = advertisement.bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    let authority = Arc::clone(&authority.authority);
    let pending = py
        .detach(move || {
            prepare_remote_query(&authority, &plan, &invocation_json, &advertisement, limits)
        })
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyPendingQueryV2Remote { pending })
}

/// Register the prepared V2 query surface on the native module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("QueryV2Error", m.py().get_type::<QueryV2Error>())?;
    m.add_class::<PyQueryV2Authority>()?;
    m.add_class::<PyPendingQueryV2Remote>()?;
    m.add_function(wrap_pyfunction!(query_v2_authority, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_query_only_authority, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_execute_local, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_remote_capabilities, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_prepare_remote, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use pyo3::ffi;
    use pyo3::prelude::*;
    use pyo3::types::{PyByteArray, PyBytes, PyString};
    use pythonize::depythonize;

    use super::{
        LOCAL_WORKER_FAILED, MAX_SEMANTIC_PROFILE_ID_BYTES, PyQueryV2Authority, PythonBytes,
        PythonString, QueryV2Error, contain_local_worker_panic, python_limit, value_error,
    };
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPathSegment,
    };
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::limits::{
        MAX_CANONICAL_STRING_BYTES, MAX_QUERY_INVOCATION_BYTES, MAX_REMOTE_ENVELOPE_BYTES,
    };
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
        encode_declared_schema,
    };
    use type_bridge_orm::query_v2_builder::QueryPlanBuilder;

    #[test]
    fn structured_query_error_preserves_typed_path_and_detail_values() {
        let diagnostic = Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new("query_v2_fixture").expect("diagnostic code"),
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
            assert!(error.is_instance_of::<QueryV2Error>(py));
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
                "query_v2_fixture",
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
            let authority = PyQueryV2Authority::new(
                py,
                PyBytes::new(py, &bytes).as_any(),
                PyString::new(py, "python-authority-constructor").as_any(),
                PyString::new(py, "typedb-3.12.1/v1").as_any(),
            )
            .expect("public class constructor");
            QueryPlanBuilder::new(authority.authority())
                .binding("person")
                .expect("constructed authority drives the shared builder");
        });
    }

    #[test]
    fn python_limits_require_exact_integers_like_the_node_bigint_surface() {
        Python::initialize();
        Python::attach(|py| {
            let integer = py.eval(ffi::c_str!("1"), None, None).expect("integer");
            assert_eq!(python_limit(&integer), Ok(1));

            let boolean = py.eval(ffi::c_str!("True"), None, None).expect("boolean");
            let diagnostic = python_limit(&boolean).expect_err("bool is not an exact integer");
            assert_eq!(diagnostic.code().as_str(), "query_remote_limit_invalid");
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
                "a".repeat(MAX_SEMANTIC_PROFILE_ID_BYTES - "/v1".len())
            );
            let exact_value = PyString::new(py, &exact_profile);
            let profile = PythonString::extract(exact_value.as_any(), "profile")
                .expect("profile string")
                .bounded_snapshot(MAX_SEMANTIC_PROFILE_ID_BYTES)
                .expect("UTF-8 profile");
            assert_eq!(profile, exact_profile);
            SemanticProfileId::new(profile).expect("exact profile ceiling is valid");

            let profile = PythonString::extract(value.as_any(), "profile")
                .expect("profile string")
                .bounded_snapshot(MAX_SEMANTIC_PROFILE_ID_BYTES)
                .expect("UTF-8 profile");
            assert_eq!(profile.len(), MAX_SEMANTIC_PROFILE_ID_BYTES + 1);
            assert_eq!(
                SemanticProfileId::new(profile)
                    .expect_err("oversized profile marker")
                    .code()
                    .as_str(),
                "invalid_fingerprint_identifier",
            );
        });
    }

    #[test]
    fn local_worker_panics_become_stable_python_runtime_errors() {
        Python::initialize();
        let error = contain_local_worker_panic(|| -> () {
            panic!("provider panic detail must not cross FFI");
        })
        .expect_err("panic is contained");
        Python::attach(|py| {
            assert!(error.is_instance_of::<pyo3::exceptions::PyRuntimeError>(py));
            assert_eq!(
                error
                    .value(py)
                    .str()
                    .expect("exception string")
                    .to_str()
                    .expect("UTF-8 exception"),
                LOCAL_WORKER_FAILED,
            );
        });
    }
}
