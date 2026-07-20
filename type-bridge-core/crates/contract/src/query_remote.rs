//! Versioned fail-closed wire envelopes for remote query execution.
//!
//! One validated plan/result contract serves direct and server execution:
//! the request carries the exact canonical plan bytes plus the invocation
//! (operation, input rows, caller budgets) and a caller nonce; the response
//! binds that nonce and the plan fingerprint so replayed or foreign
//! evidence is rejected before any host object is constructed. Envelope
//! formats are versioned independently of the plan format.

use serde::{Deserialize, Serialize};

use crate::codec::{from_canonical_json, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDigest, FingerprintDomain,
};
use crate::id::TypeId;
use crate::query_plan::{
    InputRow, QueryInvocation, QueryOperation, QueryPlan, QueryPlanFingerprint, decode_query_plan,
};
use crate::value::CanonicalValue;

/// The exact wire discriminator for first-format remote requests.
pub const QUERY_REMOTE_REQUEST_FORMAT_V1: &str = "typebridge.query-remote-request/v1";
/// The exact wire discriminator for first-format remote responses.
pub const QUERY_REMOTE_RESPONSE_FORMAT_V1: &str = "typebridge.query-remote-response/v1";
/// The exact wire discriminator for first-format remote failures.
pub const QUERY_REMOTE_FAILURE_FORMAT_V1: &str = "typebridge.query-remote-failure/v1";

const NONCE_MIN_BYTES: usize = 16;
const NONCE_MAX_BYTES: usize = 128;

/// Caller execution budgets carried with one remote invocation.
///
/// Budgets tighten provider and session ceilings; they never raise them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteLimits {
    /// Optional wall-clock deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Maximum response bytes the caller accepts.
    pub max_bytes: u64,
    /// Maximum answer items the caller accepts.
    pub max_items: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RemoteOperation {
    Rows,
    Count,
    Exists,
}

impl RemoteOperation {
    const fn from_operation(operation: QueryOperation) -> Self {
        match operation {
            QueryOperation::Rows => Self::Rows,
            QueryOperation::Count => Self::Count,
            QueryOperation::Exists => Self::Exists,
        }
    }

    const fn operation(self) -> QueryOperation {
        match self {
            Self::Rows => QueryOperation::Rows,
            Self::Count => QueryOperation::Count,
            Self::Exists => QueryOperation::Exists,
        }
    }
}

/// One complete remote invocation of a reusable validated plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryRequest {
    format: String,
    limits: RemoteLimits,
    nonce: String,
    operation: RemoteOperation,
    // Pre-release wire ledger: the /v1 request embedded the plan as one
    // JSON string until 2.0.0 shipped, which capped remote plans at the
    // 1 MiB per-string ceiling instead of the 16 MiB document limit the
    // plan contract states. The plan now embeds as the canonical JSON
    // object itself, so local and remote share one plan size limit.
    plan: serde_json::Value,
    rows: Vec<Vec<Option<CanonicalValue>>>,
}

impl RemoteQueryRequest {
    /// Bind one plan invocation and caller budgets into a request envelope.
    pub fn new(
        plan: &QueryPlan,
        invocation: &QueryInvocation,
        limits: RemoteLimits,
        nonce: impl Into<String>,
    ) -> Result<Self, Diagnostic> {
        if !invocation.binds(plan)? {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_invocation_plan_mismatch",
                "invocation does not bind the exact plan fingerprint",
            ));
        }
        let nonce = nonce.into();
        check_nonce(&nonce)?;
        // Canonical bytes prove the plan encodes within contract limits;
        // the parsed tree of those exact bytes is what the envelope embeds.
        let plan_value = serde_json::from_slice::<serde_json::Value>(&plan.canonical_bytes()?)
            .map_err(|_| {
                envelope_failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_plan_unencodable",
                    "the plan cannot be embedded as canonical JSON",
                )
            })?;
        Ok(Self {
            format: QUERY_REMOTE_REQUEST_FORMAT_V1.to_owned(),
            limits,
            nonce,
            operation: RemoteOperation::from_operation(invocation.operation()),
            plan: plan_value,
            rows: invocation
                .inputs()
                .iter()
                .map(|row| row.values().to_vec())
                .collect(),
        })
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Decode one request envelope, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let request = from_canonical_json::<Self>(bytes)?;
        if request.format != QUERY_REMOTE_REQUEST_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote request wire format is unsupported",
            ));
        }
        check_nonce(&request.nonce)?;
        Ok(request)
    }

    /// Rebuild the trusted plan from the embedded canonical document.
    ///
    /// The embedded tree re-encodes to exact canonical bytes and runs the
    /// full plan wire decoder, so every structural plan check applies to
    /// remote plans exactly as it does to local ones.
    pub fn plan(&self) -> Result<QueryPlan, Diagnostic> {
        decode_query_plan(&to_canonical_json(&self.plan)?)
    }

    /// Rebuild the validated invocation against the carried plan.
    pub fn invocation(&self, plan: &QueryPlan) -> Result<QueryInvocation, Diagnostic> {
        QueryInvocation::new(
            plan,
            self.operation.operation(),
            self.rows
                .iter()
                .map(|row| InputRow::new(row.clone()))
                .collect(),
        )
    }

    /// Return the caller budgets.
    #[must_use]
    pub const fn limits(&self) -> RemoteLimits {
        self.limits
    }

    /// Return the caller nonce echoed by the response.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
}

/// One typed value of a remote result row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteValue {
    /// An entity or relation reference.
    Thing {
        /// The provider instance identity.
        iid: String,
        /// The validated runtime type.
        type_id: TypeId,
    },
    /// An attribute instance with its parsed canonical value.
    Attribute {
        /// The validated runtime attribute type.
        type_id: TypeId,
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// A pure typed value.
    Value {
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// An explicit absence in an optional column.
    Absent,
}

/// One typed field value of a remote fetched document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteFieldValue {
    /// One exact typed scalar.
    Scalar {
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// An explicit absence in an optional scalar field.
    Absent,
    /// A typed list of attribute values.
    List {
        /// The exact typed list elements.
        values: Vec<CanonicalValue>,
    },
}

/// The typed terminal outcome of one remote invocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteOutcome {
    /// Evidence-validated projected rows in provider order.
    Rows {
        /// Positional row values per validated output column.
        rows: Vec<Vec<RemoteValue>>,
    },
    /// Evidence-validated fetched documents in provider order.
    Documents {
        /// Positional field values per validated document column.
        documents: Vec<Vec<RemoteFieldValue>>,
    },
    /// The exact number of returned answers.
    Count {
        /// The counted answers.
        value: u64,
    },
    /// Whether at least one answer exists.
    Exists {
        /// The existence verdict.
        value: bool,
    },
}

/// Fingerprint domain for whole remote request envelopes.
pub const QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN: &str = "typebridge.query.remote-request";
/// Canonicalization identifier for whole remote request envelopes.
pub const QUERY_REMOTE_REQUEST_CANONICALIZATION: &str = "typebridge.query-remote-request/v1";

/// The canonical fingerprint of one complete request envelope.
///
/// Covers every request field — plan bytes, operation, input rows, limits,
/// and nonce — so evidence carrying it is bound to exactly one invocation,
/// never merely to a plan/nonce pair whose rows may differ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRequestFingerprint(Fingerprint);

impl RemoteRequestFingerprint {
    /// Compute the fingerprint of exact request envelope bytes.
    pub fn compute(request_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_REQUEST_CANONICALIZATION)?,
            None,
            request_bytes,
        )))
    }

    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    fn digest_hex(&self) -> String {
        self.0.digest().to_hex()
    }
}

/// One successful remote execution bound to its request and plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryResponse {
    format: String,
    nonce: String,
    outcome: RemoteOutcome,
    plan: String,
    // Pre-release wire ledger: the /v1 response gained the whole-request
    // binding before any 2.0.0 artifact shipped; no released bytes change.
    request: String,
}

impl RemoteQueryResponse {
    /// Bind one outcome to the request nonce, plan, and whole request.
    pub fn new(
        nonce: impl Into<String>,
        plan: &QueryPlanFingerprint,
        request: &RemoteRequestFingerprint,
        outcome: RemoteOutcome,
    ) -> Result<Self, Diagnostic> {
        let nonce = nonce.into();
        check_nonce(&nonce)?;
        Ok(Self {
            format: QUERY_REMOTE_RESPONSE_FORMAT_V1.to_owned(),
            nonce,
            outcome,
            plan: plan.as_fingerprint().digest().to_hex(),
            request: request.digest_hex(),
        })
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Decode and authenticate one response envelope.
    ///
    /// The caller supplies the nonce and plan fingerprint it sent; replayed,
    /// foreign, or malformed evidence is rejected here, before any host
    /// object is constructed from the outcome.
    pub fn decode(
        bytes: &[u8],
        expected_nonce: &str,
        expected_plan: &QueryPlanFingerprint,
        expected_request: &RemoteRequestFingerprint,
    ) -> Result<Self, Diagnostic> {
        let response = from_canonical_json::<Self>(bytes)?;
        if response.format != QUERY_REMOTE_RESPONSE_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote response wire format is unsupported",
            ));
        }
        if response.nonce != expected_nonce {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_nonce_mismatch",
                "response evidence does not echo the request nonce",
            ));
        }
        let echoed = FingerprintDigest::from_hex(&response.plan)?;
        if echoed != expected_plan.as_fingerprint().digest() {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_plan_mismatch",
                "response evidence does not bind the invoked plan",
            ));
        }
        let echoed_request = FingerprintDigest::from_hex(&response.request)?;
        if echoed_request != expected_request.as_fingerprint().digest() {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_request_mismatch",
                "response evidence does not bind the exact request envelope",
            ));
        }
        Ok(response)
    }

    /// Return the typed outcome.
    #[must_use]
    pub const fn outcome(&self) -> &RemoteOutcome {
        &self.outcome
    }
}

/// One structured remote failure bound to its request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryFailure {
    category: DiagnosticCategory,
    code: String,
    format: String,
    message: String,
    nonce: Option<String>,
    // Pre-release wire ledger: the /v1 failure gained the optional
    // whole-request binding before any 2.0.0 artifact shipped.
    request: Option<String>,
}

impl RemoteQueryFailure {
    /// Bind one structured diagnostic to the request nonce.
    #[must_use]
    pub fn new(nonce: Option<String>, diagnostic: &Diagnostic) -> Self {
        Self {
            category: diagnostic.category(),
            code: diagnostic.code().as_str().to_owned(),
            format: QUERY_REMOTE_FAILURE_FORMAT_V1.to_owned(),
            message: diagnostic.message().to_owned(),
            nonce,
            request: None,
        }
    }

    /// Bind one structured diagnostic to the nonce and the exact request.
    #[must_use]
    pub fn bound(
        nonce: impl Into<String>,
        request: &RemoteRequestFingerprint,
        diagnostic: &Diagnostic,
    ) -> Self {
        Self {
            category: diagnostic.category(),
            code: diagnostic.code().as_str().to_owned(),
            format: QUERY_REMOTE_FAILURE_FORMAT_V1.to_owned(),
            message: diagnostic.message().to_owned(),
            nonce: Some(nonce.into()),
            request: Some(request.digest_hex()),
        }
    }

    /// Authenticate this failure against the request the caller sent.
    ///
    /// A present nonce must echo the sent nonce and a present request
    /// digest must bind the sent envelope; a failure claiming foreign
    /// evidence is rejected instead of surfacing its diagnostic.
    pub fn verify_binding(
        &self,
        expected_nonce: &str,
        expected_request: &RemoteRequestFingerprint,
    ) -> Result<(), Diagnostic> {
        if let Some(nonce) = &self.nonce
            && nonce != expected_nonce
        {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_nonce_mismatch",
                "failure evidence does not echo the request nonce",
            ));
        }
        if let Some(request) = &self.request {
            let echoed = FingerprintDigest::from_hex(request)?;
            if echoed != expected_request.as_fingerprint().digest() {
                return Err(envelope_failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_request_mismatch",
                    "failure evidence does not bind the exact request envelope",
                ));
            }
        }
        Ok(())
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Decode one failure envelope, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let failure = from_canonical_json::<Self>(bytes)?;
        if failure.format != QUERY_REMOTE_FAILURE_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote failure wire format is unsupported",
            ));
        }
        Ok(failure)
    }

    /// Rebuild the structured diagnostic.
    pub fn diagnostic(&self) -> Result<Diagnostic, Diagnostic> {
        let code = DiagnosticCode::new(self.code.clone()).map_err(|_| {
            envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_code_invalid",
                "remote failure carries a malformed diagnostic code",
            )
        })?;
        Ok(Diagnostic::new(self.category, code, self.message.clone()))
    }

    /// Return the echoed request nonce, when the request decoded far enough.
    #[must_use]
    pub fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }
}

/// The exact wire discriminator for first-format capability advertisements.
pub const QUERY_REMOTE_CAPABILITIES_FORMAT_V1: &str = "typebridge.query-remote-capabilities/v1";

/// One executor capability advertisement for pre-flight negotiation.
///
/// A client checks its plan's required capabilities against this set and
/// refuses to send unsupported plans; the executor re-checks on receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCapabilities {
    capabilities: crate::capability::CapabilitySet,
    format: String,
}

impl RemoteCapabilities {
    /// Advertise one executor capability set.
    #[must_use]
    pub fn new(capabilities: crate::capability::CapabilitySet) -> Self {
        Self {
            capabilities,
            format: QUERY_REMOTE_CAPABILITIES_FORMAT_V1.to_owned(),
        }
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Decode one advertisement, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let advertisement = from_canonical_json::<Self>(bytes)?;
        if advertisement.format != QUERY_REMOTE_CAPABILITIES_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote capability wire format is unsupported",
            ));
        }
        Ok(advertisement)
    }

    /// Return the advertised capability set.
    #[must_use]
    pub const fn capabilities(&self) -> &crate::capability::CapabilitySet {
        &self.capabilities
    }
}

/// One decoded remote reply: a typed response or an authenticated failure.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteReply {
    /// A typed successful response, fully bound to the request.
    Response(RemoteQueryResponse),
    /// A structured failure whose present bindings authenticated.
    Failure(RemoteQueryFailure),
}

/// Decode one reply envelope of either kind and authenticate its binding.
///
/// Success and failure envelopes share one entry point so every caller —
/// including the Python and Node bindings — decodes both outcomes with the
/// same nonce and whole-request authentication.
pub fn decode_remote_reply(
    bytes: &[u8],
    expected_nonce: &str,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprint,
) -> Result<RemoteReply, Diagnostic> {
    #[derive(Deserialize)]
    struct FormatPeek {
        format: String,
    }
    let peek: FormatPeek = serde_json::from_slice(bytes).map_err(|_| {
        envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_reply_malformed",
            "remote reply is not a JSON envelope with a format discriminator",
        )
    })?;
    if peek.format == QUERY_REMOTE_RESPONSE_FORMAT_V1 {
        return Ok(RemoteReply::Response(RemoteQueryResponse::decode(
            bytes,
            expected_nonce,
            expected_plan,
            expected_request,
        )?));
    }
    if peek.format == QUERY_REMOTE_FAILURE_FORMAT_V1 {
        let failure = RemoteQueryFailure::decode(bytes)?;
        failure.verify_binding(expected_nonce, expected_request)?;
        return Ok(RemoteReply::Failure(failure));
    }
    Err(envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_format_unsupported",
        "remote reply wire format is unsupported",
    ))
}

/// Convert one caller-supplied limit into the unsigned wire range.
///
/// Every binding funnels its limit arguments through this exact
/// conversion, so a negative or out-of-range budget fails with one
/// stable diagnostic in every language instead of silently saturating
/// to zero or dropping the budget entirely.
pub fn checked_remote_limit(value: i128) -> Result<u64, Diagnostic> {
    u64::try_from(value).map_err(|_| remote_limit_invalid())
}

/// The stable rejection every out-of-range limit argument maps to.
///
/// Exposed so bindings whose integer representation exceeds `i128`
/// (JavaScript `BigInt`) reject unrepresentable values with exactly
/// this diagnostic instead of inventing their own.
#[must_use]
pub fn remote_limit_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_limit_invalid",
        "remote limits are unsigned 64-bit integers",
    )
}

/// Convert one optional caller-supplied deadline into the wire range.
///
/// A negative deadline is rejected — never silently mapped to "no
/// deadline", which would remove the bound instead of enforcing it.
pub fn checked_remote_deadline(value: Option<i128>) -> Result<Option<u64>, Diagnostic> {
    value.map(checked_remote_limit).transpose()
}

fn check_nonce(nonce: &str) -> Result<(), Diagnostic> {
    let valid = (NONCE_MIN_BYTES..=NONCE_MAX_BYTES).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_nonce_invalid",
            "request nonces are 16-128 ASCII alphanumeric or dash bytes",
        ))
    }
}

fn envelope_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static remote envelope code"),
        message,
    )
}
