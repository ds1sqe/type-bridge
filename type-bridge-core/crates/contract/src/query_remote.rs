//! Versioned fail-closed wire envelopes for remote query execution.
//!
//! One validated plan/result contract serves direct and server execution:
//! the request carries the exact canonical plan bytes plus the invocation
//! (operation, input rows, caller budgets), the exact executor advertisement,
//! an absolute bounded expiry, and a caller nonce; the response binds that
//! nonce and the whole request fingerprint so replayed or foreign evidence is
//! rejected before any host object is constructed. Envelope formats are
//! versioned independently of the plan format.

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::codec::{
    from_canonical_json_with_limits, to_canonical_json, to_canonical_json_with_limits,
};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDigest, FingerprintDomain,
};
use crate::id::TypeId;
use crate::limits::{
    MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES, REMOTE_ENVELOPE_CODEC_LIMITS,
    REMOTE_REQUEST_CODEC_LIMITS,
};
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
/// The authenticated outer reply format used for both successes and failures.
pub const QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1: &str = "typebridge.query-remote-signed-reply/v1";
/// Domain separating Ed25519 reply signatures from every other signed value.
pub const QUERY_REMOTE_REPLY_SIGNATURE_DOMAIN: &str = "typebridge.query.remote-reply-signature/v1";
/// Domain separating deterministic reply-signing key identifiers.
pub const QUERY_REMOTE_REPLY_KEY_ID_DOMAIN: &str = "typebridge.query.remote-reply-key-id/v1";

const NONCE_MIN_BYTES: usize = 16;
const NONCE_MAX_BYTES: usize = 128;
const EXECUTOR_COMPONENT_MIN_BYTES: usize = 16;
const EXECUTOR_COMPONENT_MAX_BYTES: usize = 128;
/// Default lifetime for a remote request with no explicit caller deadline.
///
/// This matches the standalone executor's mandatory execution timeout and
/// keeps one omitted field from occupying replay capacity for hours.
pub const DEFAULT_REMOTE_DEADLINE_MS: u64 = 30 * 1_000;
/// Longest caller deadline admitted by the first remote format: five minutes.
pub const MAX_REMOTE_DEADLINE_MS: u64 = 5 * 60 * 1_000;
/// Maximum positive client/server wall-clock skew admitted by remote preflight.
///
/// The request lifetime is still bounded to [`MAX_REMOTE_DEADLINE_MS`]
/// between its fingerprint-bound preparation and expiry timestamps. This allowance only
/// prevents a client clock up to one minute ahead from being rejected as a
/// forged future request.
pub const MAX_REMOTE_CLOCK_SKEW_MS: u64 = 60 * 1_000;

/// Caller execution budgets carried with one remote invocation.
///
/// Budgets tighten provider and session ceilings; they never raise them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteLimits {
    /// Optional wall-clock deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Maximum successful typed-response bytes the caller accepts.
    ///
    /// Authenticated structured failures are control-plane evidence bounded by
    /// [`MAX_REMOTE_ENVELOPE_BYTES`], not by this success-data budget. This lets
    /// an executor report that even the smallest success cannot fit when the
    /// caller deliberately supplies a zero or otherwise tiny budget.
    pub max_bytes: u64,
    /// Maximum answer items the caller accepts.
    pub max_items: u64,
    /// Maximum aggregate list members the caller accepts across documents.
    pub max_collection_members: u64,
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
    // Pre-release wire ledger: before any 2.0.0 artifact shipped, /v1 gained
    // an exact capability-advertisement fingerprint and absolute bounded
    // preparation/expiry timestamps. Relative deadline_ms remains the caller
    // API, but it is resolved once during preparation; executors never restart
    // or replay the full relative window. Before any 2.0.0 artifact shipped,
    // the omitted-deadline lifetime became 30 seconds, the explicit ceiling
    // became five minutes, and decode began requiring the absolute timestamps
    // to equal that fingerprint-bound declared/default lifetime exactly.
    advertisement: String,
    expires_at_unix_ms: u64,
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
    prepared_at_unix_ms: u64,
    rows: Vec<Vec<Option<CanonicalValue>>>,
}

impl RemoteQueryRequest {
    /// Bind one plan invocation and caller budgets into a request envelope.
    pub fn new(
        plan: &QueryPlan,
        invocation: &QueryInvocation,
        advertisement: &RemoteCapabilities,
        limits: RemoteLimits,
        nonce: impl Into<String>,
        prepared_at_unix_ms: u64,
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
        validate_remote_limits(limits)?;
        let expires_at_unix_ms = prepared_at_unix_ms
            .checked_add(limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS))
            .ok_or_else(remote_time_invalid)?;
        validate_remote_time_shape(prepared_at_unix_ms, expires_at_unix_ms, limits)?;
        let advertisement = advertisement.fingerprint()?.digest_hex();
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
            advertisement,
            expires_at_unix_ms,
            format: QUERY_REMOTE_REQUEST_FORMAT_V1.to_owned(),
            limits,
            nonce,
            operation: RemoteOperation::from_operation(invocation.operation()),
            plan: plan_value,
            prepared_at_unix_ms,
            rows: invocation
                .inputs()
                .iter()
                .map(|row| row.values().to_vec())
                .collect(),
        })
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_REQUEST_CODEC_LIMITS)
    }

    /// Decode one request envelope, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let request = from_canonical_json_with_limits::<Self>(bytes, REMOTE_REQUEST_CODEC_LIMITS)?;
        if request.format != QUERY_REMOTE_REQUEST_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote request wire format is unsupported",
            ));
        }
        check_nonce(&request.nonce)?;
        validate_remote_limits(request.limits)?;
        FingerprintDigest::from_hex(&request.advertisement).map_err(|_| {
            envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_advertisement_invalid",
                "remote request carries a malformed advertisement fingerprint",
            )
        })?;
        validate_remote_time_shape(
            request.prepared_at_unix_ms,
            request.expires_at_unix_ms,
            request.limits,
        )?;
        if request.encode()? != bytes {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_request_wire_mismatch",
                "remote request bytes normalize after trusted reconstruction",
            ));
        }
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

    /// Return whether this request binds the executor's exact advertisement.
    pub fn binds_advertisement(
        &self,
        advertisement: &RemoteCapabilities,
    ) -> Result<bool, Diagnostic> {
        let expected = advertisement.fingerprint()?;
        let actual = FingerprintDigest::from_hex(&self.advertisement).map_err(|_| {
            envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_advertisement_invalid",
                "remote request carries a malformed advertisement fingerprint",
            )
        })?;
        Ok(actual == expected.as_fingerprint().digest())
    }

    /// Validate this request's absolute lifetime at one executor clock sample.
    ///
    /// Clients at most [`MAX_REMOTE_CLOCK_SKEW_MS`] ahead are accepted. The
    /// expiry itself is exclusive: a request at or beyond it is rejected.
    /// The returned duration reaches the absolute expiry and is the replay
    /// retention horizon. It can exceed the declared execution duration by
    /// the admitted positive clock skew; use [`Self::remaining_execution_ms`]
    /// to bound execution itself.
    pub fn remaining_lifetime_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        validate_remote_time_shape(
            self.prepared_at_unix_ms,
            self.expires_at_unix_ms,
            self.limits,
        )?;
        if self.prepared_at_unix_ms > now_unix_ms.saturating_add(MAX_REMOTE_CLOCK_SKEW_MS) {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_time_future",
                "remote request preparation time exceeds the allowed clock skew",
            ));
        }
        self.expires_at_unix_ms
            .checked_sub(now_unix_ms)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| {
                envelope_failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_request_expired",
                    "remote request absolute expiry has elapsed",
                )
            })
    }

    /// Return the remaining execution duration without granting clock skew.
    ///
    /// Positive skew can place absolute expiry later than `now + declared
    /// duration`. Execution therefore uses the smaller of the absolute
    /// remaining horizon and the fingerprint-bound declared lifetime, while
    /// replay retention continues through the full absolute horizon.
    pub fn remaining_execution_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        let absolute_remaining = self.remaining_lifetime_ms(now_unix_ms)?;
        let declared_lifetime = self
            .expires_at_unix_ms
            .checked_sub(self.prepared_at_unix_ms)
            .ok_or_else(remote_time_invalid)?;
        Ok(absolute_remaining.min(declared_lifetime))
    }

    /// Return the request's exclusive absolute expiry timestamp.
    #[must_use]
    pub const fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
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

/// Exact Ed25519 public key trusted to authenticate replies from one executor.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct RemoteSigningPublicKey([u8; 32]);

impl RemoteSigningPublicKey {
    /// Construct a public key from its exact Ed25519 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact Ed25519 public-key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the key as fixed-width lowercase hexadecimal wire text.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteSigningPublicKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RemoteSigningPublicKey")
            .field(&self.to_hex())
            .finish()
    }
}

impl Serialize for RemoteSigningPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RemoteSigningPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(|()| serde::de::Error::custom("invalid remote signing key"))
    }
}

/// Deterministic identity of one exact remote reply-signing public key.
///
/// The identifier is domain-separated from every other digest and is carried
/// alongside the key in capability advertisements and signed outer replies.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct RemoteSigningKeyId([u8; 32]);

impl RemoteSigningKeyId {
    /// Derive the identifier for one exact Ed25519 public key.
    #[must_use]
    pub fn for_public_key(key: RemoteSigningPublicKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(QUERY_REMOTE_REPLY_KEY_ID_DOMAIN.as_bytes());
        hasher.update([0]);
        hasher.update(key.as_bytes());
        Self(hasher.finalize().into())
    }

    /// Borrow the exact key-identifier bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the identifier as fixed-width lowercase hexadecimal wire text.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteSigningKeyId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RemoteSigningKeyId")
            .field(&self.to_hex())
            .finish()
    }
}

impl Serialize for RemoteSigningKeyId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RemoteSigningKeyId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value)
            .map_err(|()| serde::de::Error::custom("invalid remote signing key identifier"))
    }
}

/// Exact Ed25519 signature carried by the authenticated outer reply.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RemoteReplySignature([u8; 64]);

impl RemoteReplySignature {
    /// Construct a signature from its exact Ed25519 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact Ed25519 signature bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteReplySignature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemoteReplySignature(..)")
    }
}

/// Domain-separated digest signed for one canonical unsigned outer reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteReplySigningDigest([u8; 32]);

impl RemoteReplySigningDigest {
    /// Borrow the digest bytes passed to the Ed25519 implementation.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Binding-neutral signing operation used by the contract wire encoder.
pub trait RemoteReplySigner {
    /// Return the public key paired with this signer.
    fn public_key(&self) -> RemoteSigningPublicKey;

    /// Sign one domain-separated canonical reply digest.
    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature;
}

/// Binding-neutral signature verifier used before any reply payload is decoded.
pub trait RemoteReplyVerifier {
    /// Verify one digest against the exact trusted public key.
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool;
}

/// Expected authenticated success shape used for allocation-free budget scans.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteOutcomeShape {
    /// Selected rows with this exact output width.
    Rows {
        /// Exact number of values in every selected row.
        width: usize,
    },
    /// Fetched documents with this exact output width.
    Documents {
        /// Exact number of fields in every fetched document.
        width: usize,
    },
    /// A scalar count.
    Count,
    /// A scalar existence verdict.
    Exists,
}

/// Caller limits checked before a typed remote outcome is allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteReplyDecodeLimits {
    /// Expected success outcome shape.
    pub shape: RemoteOutcomeShape,
    /// Maximum authenticated successful outer-reply bytes accepted by the caller.
    ///
    /// Request-bound failure envelopes remain subject to the protocol hard
    /// ceiling so their typed diagnostic can be surfaced at any success budget.
    pub max_bytes: u64,
    /// Maximum rows, documents, count value, or positive existence item accepted by the caller.
    pub max_items: u64,
    /// Maximum aggregate list members accepted across fetched documents.
    pub max_collection_members: u64,
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

    /// Encode one authenticated outer reply using the exact trusted advertisement.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        encode_signed_reply(&self.encode_payload()?, advertisement, signer)
    }

    /// Return the exact authenticated wire length without invoking a signer.
    pub fn signed_encoded_len(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
    ) -> Result<usize, Diagnostic> {
        Ok(signed_reply_encoded_len(
            &self.encode_payload()?,
            advertisement,
            key,
        ))
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    fn decode_bound(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let response =
            from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if response.format != QUERY_REMOTE_RESPONSE_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote response wire format is unsupported",
            ));
        }
        if response.encode_payload()? != bytes {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_response_wire_mismatch",
                "remote response bytes normalize after trusted reconstruction",
            ));
        }
        Ok(response)
    }

    /// Return the typed outcome.
    #[must_use]
    pub const fn outcome(&self) -> &RemoteOutcome {
        &self.outcome
    }

    /// Consume the envelope and return its typed outcome.
    #[must_use]
    pub fn into_outcome(self) -> RemoteOutcome {
        self.outcome
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

    /// Verify this failure's binding to the request the caller sent.
    ///
    /// Both the nonce and whole-request digest are mandatory on a reply to a
    /// valid request. Pre-decode transport failures use a separate channel;
    /// an unbound envelope is never treated as request-correlated evidence.
    pub fn verify_binding(
        &self,
        expected_nonce: &str,
        expected_request: &RemoteRequestFingerprint,
    ) -> Result<(), Diagnostic> {
        verify_failure_binding(
            self.nonce.as_deref(),
            self.request.as_deref(),
            expected_nonce,
            expected_request,
        )
    }

    /// Encode one authenticated outer reply using the exact trusted advertisement.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        encode_signed_reply(&self.encode_payload()?, advertisement, signer)
    }

    /// Encode an authenticated failure, replacing an unencodable diagnostic
    /// with a fixed bounded internal failure while preserving safe bindings.
    ///
    /// Server transports use this path so production failures can never
    /// collapse to empty or unsigned bytes merely because an upstream error
    /// message exceeded a codec ceiling.
    #[must_use]
    pub fn encode_signed_or_fallback(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Vec<u8> {
        match self.encode_signed(advertisement, signer) {
            Ok(encoded) => encoded,
            Err(_) => encode_minimal_signed_failure(self, advertisement, signer),
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode an already authenticated failure payload.
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let failure = from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
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
/// Fingerprint domain for exact executor capability advertisements.
pub const QUERY_REMOTE_CAPABILITIES_FINGERPRINT_DOMAIN: &str =
    "typebridge.query.remote-capabilities";
/// Canonicalization identifier for capability-advertisement fingerprints.
pub const QUERY_REMOTE_CAPABILITIES_CANONICALIZATION: &str =
    "typebridge.query-remote-capabilities/v1";

/// One logical executor identity and one concrete process/shared-store epoch.
///
/// Standalone executors generate a fresh pair at startup. A multi-instance
/// deployment may share a pair only together with a globally atomic replay
/// store; otherwise advertisements differ and cross-instance requests fail
/// closed at preflight.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutorBinding {
    epoch: String,
    identity: String,
}

impl RemoteExecutorBinding {
    /// Construct one validated executor identity/epoch pair.
    pub fn new(identity: impl Into<String>, epoch: impl Into<String>) -> Result<Self, Diagnostic> {
        let binding = Self {
            epoch: epoch.into(),
            identity: identity.into(),
        };
        validate_executor_component(&binding.identity)?;
        validate_executor_component(&binding.epoch)?;
        Ok(binding)
    }

    /// Return the logical executor identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Return the concrete process/shared-store epoch.
    #[must_use]
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
}

/// The canonical fingerprint of one exact capability advertisement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCapabilitiesFingerprint(Fingerprint);

impl RemoteCapabilitiesFingerprint {
    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    /// Return the fixed-width lowercase advertisement digest.
    #[must_use]
    pub fn digest_hex(&self) -> String {
        self.0.digest().to_hex()
    }
}

/// One executor capability advertisement for pre-flight negotiation.
///
/// A client checks its plan's required capabilities against this set and
/// refuses to send unsupported plans; the executor re-checks on receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCapabilities {
    capabilities: crate::capability::CapabilitySet,
    // Pre-release wire ledger: /v1 gained an executor identity and epoch
    // before 2.0.0 shipped. Requests fingerprint these exact bytes so a
    // restart or a different standalone instance cannot accept captured
    // requests prepared for an earlier executor incarnation.
    executor: RemoteExecutorBinding,
    format: String,
    // This key is an explicit caller trust input. Its inclusion in the exact
    // advertisement fingerprint binds every prepared request to one signer.
    reply_key: RemoteSigningPublicKey,
    // The domain-separated identity is redundant by design: decoding verifies
    // it against `reply_key`, while signed replies bind both exact values.
    reply_key_id: RemoteSigningKeyId,
}

impl RemoteCapabilities {
    /// Advertise one executor capability set.
    #[must_use]
    pub fn new(
        capabilities: crate::capability::CapabilitySet,
        executor: RemoteExecutorBinding,
        reply_key: RemoteSigningPublicKey,
    ) -> Self {
        let reply_key_id = RemoteSigningKeyId::for_public_key(reply_key);
        Self {
            capabilities,
            executor,
            format: QUERY_REMOTE_CAPABILITIES_FORMAT_V1.to_owned(),
            reply_key,
            reply_key_id,
        }
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode one advertisement, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let advertisement =
            from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if advertisement.format != QUERY_REMOTE_CAPABILITIES_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote capability wire format is unsupported",
            ));
        }
        validate_executor_component(advertisement.executor.identity())?;
        validate_executor_component(advertisement.executor.epoch())?;
        if advertisement.reply_key_id != RemoteSigningKeyId::for_public_key(advertisement.reply_key)
        {
            return Err(remote_signature_invalid());
        }
        if advertisement.encode()? != bytes {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_capabilities_wire_mismatch",
                "remote capability bytes normalize after trusted reconstruction",
            ));
        }
        Ok(advertisement)
    }

    /// Return the advertised capability set.
    #[must_use]
    pub const fn capabilities(&self) -> &crate::capability::CapabilitySet {
        &self.capabilities
    }

    /// Return this advertisement's executor identity and epoch.
    #[must_use]
    pub const fn executor(&self) -> &RemoteExecutorBinding {
        &self.executor
    }

    /// Return the exact public key trusted to authenticate executor replies.
    #[must_use]
    pub const fn reply_key(&self) -> RemoteSigningPublicKey {
        self.reply_key
    }

    /// Return the deterministic identity of the advertised reply key.
    #[must_use]
    pub const fn reply_key_id(&self) -> RemoteSigningKeyId {
        self.reply_key_id
    }

    /// Fingerprint the exact canonical advertisement, including its epoch.
    pub fn fingerprint(&self) -> Result<RemoteCapabilitiesFingerprint, Diagnostic> {
        Ok(RemoteCapabilitiesFingerprint(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_CAPABILITIES_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_CAPABILITIES_CANONICALIZATION)?,
            None,
            &self.encode()?,
        )))
    }
}

/// One decoded remote reply: a typed response or a request-bound failure.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteReply {
    /// A typed successful response, fully bound to the request.
    Response(RemoteQueryResponse),
    /// A structured failure whose request bindings were verified.
    Failure(RemoteQueryFailure),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedRemoteReplyPeek<'wire> {
    #[serde(borrow)]
    advertisement: &'wire str,
    #[serde(borrow)]
    format: &'wire str,
    #[serde(borrow)]
    key: &'wire str,
    #[serde(borrow)]
    key_id: &'wire str,
    #[serde(borrow)]
    payload: &'wire serde_json::value::RawValue,
    #[serde(borrow)]
    signature: &'wire str,
}

/// Borrowed correlation fields parsed before any reply outcome is materialized.
///
/// Canonical replies borrow all four strings directly from the input. Escaped
/// spellings are non-canonical and fail this precheck without allocating;
/// ignored outcome values are traversed without constructing a
/// `serde_json::Value` or a [`RemoteOutcome`].
#[derive(Deserialize)]
struct RemoteReplyBindingPeek<'wire> {
    #[serde(borrow)]
    format: &'wire str,
    #[serde(borrow)]
    nonce: Option<&'wire str>,
    #[serde(borrow)]
    plan: Option<&'wire str>,
    #[serde(borrow)]
    request: Option<&'wire str>,
}

/// Decode one reply envelope of either kind and verify its request binding.
///
/// Success and failure envelopes share one entry point so every caller —
/// including the Python and Node bindings — decodes both outcomes with the
/// same nonce and whole-request correlation checks.
#[expect(
    clippy::too_many_arguments,
    reason = "the trust-boundary API keeps every expected binding and verifier explicit"
)]
pub fn decode_remote_reply(
    bytes: &[u8],
    expected_nonce: &str,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprint,
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    limits: RemoteReplyDecodeLimits,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteReply, Diagnostic> {
    // Every reply is capped before JSON parsing or signature work. The caller
    // budget applies only to successful data: applying it before authenticating
    // the reply kind would make a valid request-bound failure undecodable at a
    // tiny budget, including the failure explaining that no success can fit.
    preflight_remote_reply_size(bytes, u64::MAX)?;
    let payload = verify_signed_reply(bytes, expected_advertisement, trusted_key, verifier)?;
    let peek = peek_remote_reply_binding(payload)?;
    if peek.format == QUERY_REMOTE_RESPONSE_FORMAT_V1 {
        verify_response_binding(
            peek.nonce,
            peek.plan,
            peek.request,
            expected_nonce,
            expected_plan,
            expected_request,
        )?;
        preflight_remote_reply_size(bytes, limits.max_bytes)?;
        preflight_remote_response_shape(payload, limits)?;
        return Ok(RemoteReply::Response(RemoteQueryResponse::decode_bound(
            payload,
        )?));
    }
    if peek.format == QUERY_REMOTE_FAILURE_FORMAT_V1 {
        verify_failure_binding(peek.nonce, peek.request, expected_nonce, expected_request)?;
        let failure = RemoteQueryFailure::decode_payload(payload)?;
        failure.verify_binding(expected_nonce, expected_request)?;
        return Ok(RemoteReply::Failure(failure));
    }
    Err(envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_format_unsupported",
        "remote reply wire format is unsupported",
    ))
}

/// Authenticate and decode an uncorrelated remote failure.
///
/// This is reserved for transport failures that occur before a request can be
/// decoded and fingerprinted. Request-correlated clients must use
/// [`decode_remote_reply`] so nonce, plan, and whole-request bindings are
/// mandatory.
pub fn decode_signed_remote_failure(
    bytes: &[u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    max_bytes: u64,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteQueryFailure, Diagnostic> {
    preflight_remote_reply_size(bytes, max_bytes)?;
    let payload = verify_signed_reply(bytes, expected_advertisement, trusted_key, verifier)?;
    RemoteQueryFailure::decode_payload(payload)
}

fn preflight_remote_reply_size(bytes: &[u8], caller_max_bytes: u64) -> Result<(), Diagnostic> {
    let wire_max_bytes = u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX);
    let effective_max_bytes = caller_max_bytes.min(wire_max_bytes);
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= effective_max_bytes {
        return Ok(());
    }
    if caller_max_bytes < wire_max_bytes {
        return Err(remote_response_oversized());
    }
    Err(remote_envelope_too_large())
}

fn verify_signed_reply<'wire>(
    bytes: &'wire [u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    verifier: &impl RemoteReplyVerifier,
) -> Result<&'wire [u8], Diagnostic> {
    let outer = serde_json::from_slice::<SignedRemoteReplyPeek<'wire>>(bytes).map_err(|_| {
        envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_reply_malformed",
            "remote reply is not a signed JSON envelope",
        )
    })?;
    let expected_advertisement = expected_advertisement.digest_hex();
    let expected_key = trusted_key.to_hex();
    let expected_key_id = RemoteSigningKeyId::for_public_key(trusted_key).to_hex();
    let signature =
        RemoteReplySignature::from_hex(outer.signature).map_err(|()| remote_signature_invalid())?;
    if outer.advertisement != expected_advertisement
        || outer.key != expected_key
        || outer.key_id != expected_key_id
        || outer.signature != signature.to_hex()
    {
        return Err(remote_signature_invalid());
    }
    let digest = remote_reply_signing_digest(
        outer.advertisement,
        outer.format,
        outer.key,
        outer.key_id,
        outer.payload.get().as_bytes(),
    );
    if !verifier.verify(trusted_key, &digest, &signature) {
        return Err(remote_signature_invalid());
    }
    if outer.format != QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1 {
        return Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_format_unsupported",
            "signed remote reply wire format is unsupported",
        ));
    }
    let canonical = canonical_signed_reply(
        outer.advertisement,
        outer.format,
        outer.key,
        outer.key_id,
        outer.payload.get().as_bytes(),
        outer.signature,
    );
    if canonical != bytes {
        return Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "non_canonical_json",
            "input is valid JSON but not the canonical encoding",
        ));
    }
    Ok(outer.payload.get().as_bytes())
}

fn encode_signed_reply(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Result<Vec<u8>, Diagnostic> {
    let encoded = encode_signed_reply_unchecked(payload, advertisement, signer);
    if encoded.len() > MAX_REMOTE_ENVELOPE_BYTES {
        return Err(envelope_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_envelope_too_large",
            "remote reply exceeds the envelope byte ceiling",
        ));
    }
    Ok(encoded)
}

fn encode_signed_reply_unchecked(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    let advertisement = advertisement.digest_hex();
    let key = signer.public_key().to_hex();
    let key_id = RemoteSigningKeyId::for_public_key(signer.public_key()).to_hex();
    let digest = remote_reply_signing_digest(
        &advertisement,
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key,
        &key_id,
        payload,
    );
    let signature = signer.sign(&digest).to_hex();
    canonical_signed_reply(
        &advertisement,
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key,
        &key_id,
        payload,
        &signature,
    )
}

fn encode_minimal_signed_failure(
    original: &RemoteQueryFailure,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"category\":\"integrity\",\"code\":\"query_remote_internal_failure\",\"format\":\"typebridge.query-remote-failure/v1\",\"message\":\"executor could not encode the original remote failure\",\"nonce\":";
    let nonce = original
        .nonce
        .as_deref()
        .filter(|nonce| check_nonce(nonce).is_ok());
    let request = original.request.as_deref().filter(|request| {
        request.len() == 64
            && request
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    let mut payload = Vec::with_capacity(PREFIX.len() + 256);
    payload.extend_from_slice(PREFIX);
    append_optional_safe_ascii(&mut payload, nonce);
    payload.extend_from_slice(b",\"request\":");
    append_optional_safe_ascii(&mut payload, request);
    payload.push(b'}');
    encode_signed_reply_unchecked(&payload, advertisement, signer)
}

fn append_optional_safe_ascii(encoded: &mut Vec<u8>, value: Option<&str>) {
    if let Some(value) = value {
        encoded.push(b'"');
        encoded.extend_from_slice(value.as_bytes());
        encoded.push(b'"');
    } else {
        encoded.extend_from_slice(b"null");
    }
}

fn signed_reply_encoded_len(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    key: RemoteSigningPublicKey,
) -> usize {
    canonical_signed_reply(
        &advertisement.digest_hex(),
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key.to_hex(),
        &RemoteSigningKeyId::for_public_key(key).to_hex(),
        payload,
        &encode_hex(&[0_u8; 64]),
    )
    .len()
}

fn remote_reply_signing_digest(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
    payload: &[u8],
) -> RemoteReplySigningDigest {
    let prefix = canonical_signed_reply_prefix(advertisement, format, key, key_id);
    let mut hasher = Sha256::new();
    hasher.update(QUERY_REMOTE_REPLY_SIGNATURE_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(&prefix);
    hasher.update(payload);
    hasher.update(b"}");
    RemoteReplySigningDigest(hasher.finalize().into())
}

fn canonical_signed_reply_prefix(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
) -> Vec<u8> {
    format!(
        "{{\"advertisement\":\"{advertisement}\",\"format\":\"{format}\",\"key\":\"{key}\",\"key_id\":\"{key_id}\",\"payload\":"
    )
    .into_bytes()
}

fn canonical_signed_reply(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
    payload: &[u8],
    signature: &str,
) -> Vec<u8> {
    let prefix = canonical_signed_reply_prefix(advertisement, format, key, key_id);
    let suffix = format!(",\"signature\":\"{signature}\"}}");
    let mut encoded = Vec::with_capacity(prefix.len() + payload.len() + suffix.len());
    encoded.extend_from_slice(&prefix);
    encoded.extend_from_slice(payload);
    encoded.extend_from_slice(suffix.as_bytes());
    encoded
}

/// Stable rejection for an unauthenticated or foreign remote reply.
#[must_use]
pub fn remote_signature_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::Integrity,
        "query_remote_signature_invalid",
        "remote reply signature or trusted executor binding is invalid",
    )
}

fn remote_response_oversized() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_response_oversized",
        "response envelope exceeds the caller byte budget",
    )
}

fn remote_envelope_too_large() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_envelope_too_large",
        "remote reply exceeds the envelope byte ceiling",
    )
}

#[derive(Deserialize)]
struct RemoteResponseOutcomePeek<'wire> {
    #[serde(borrow)]
    outcome: &'wire serde_json::value::RawValue,
}

#[derive(Clone, Copy)]
enum SequenceScanKind {
    Rows,
    Documents,
}

#[derive(Clone, Copy)]
enum ShapeLimitExceeded {
    Items,
    Width,
    Members,
    Outcome,
    Evidence,
}

#[derive(Default)]
struct ShapeScanState {
    items: u64,
    members: u64,
    exceeded: Option<ShapeLimitExceeded>,
}

fn preflight_remote_response_shape(
    payload: &[u8],
    limits: RemoteReplyDecodeLimits,
) -> Result<(), Diagnostic> {
    let response =
        serde_json::from_slice::<RemoteResponseOutcomePeek<'_>>(payload).map_err(|_| {
            envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_reply_malformed",
                "remote response payload is malformed",
            )
        })?;
    let mut state = ShapeScanState::default();
    let mut deserializer = serde_json::Deserializer::from_slice(response.outcome.get().as_bytes());
    let result = match limits.shape {
        RemoteOutcomeShape::Rows { width } => OutcomeShapeSeed {
            field: "rows",
            kind: SequenceScanKind::Rows,
            width,
            max_items: limits
                .max_items
                .min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX)),
            max_members: limits
                .max_collection_members
                .min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX)),
            state: &mut state,
        }
        .deserialize(&mut deserializer),
        RemoteOutcomeShape::Documents { width } => OutcomeShapeSeed {
            field: "documents",
            kind: SequenceScanKind::Documents,
            width,
            max_items: limits
                .max_items
                .min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX)),
            max_members: limits
                .max_collection_members
                .min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX)),
            state: &mut state,
        }
        .deserialize(&mut deserializer),
        RemoteOutcomeShape::Count => ScalarOutcomeSeed {
            kind: ScalarScanKind::Count,
            max_items: limits.max_items,
            state: &mut state,
        }
        .deserialize(&mut deserializer),
        RemoteOutcomeShape::Exists => ScalarOutcomeSeed {
            kind: ScalarScanKind::Exists,
            max_items: limits.max_items,
            state: &mut state,
        }
        .deserialize(&mut deserializer),
    };
    if let Some(exceeded) = state.exceeded {
        return Err(match exceeded {
            ShapeLimitExceeded::Items => envelope_failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_response_oversized",
                "response rows, documents, or scalar evidence exceed the caller item budget",
            ),
            ShapeLimitExceeded::Width => envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_evidence_mismatch",
                "response evidence does not conform to the validated output schema",
            ),
            ShapeLimitExceeded::Members => envelope_failure(
                DiagnosticCategory::ResourceLimit,
                "query_v2_document_member_limit",
                "document lists exceed the aggregate member ceiling",
            ),
            ShapeLimitExceeded::Outcome => envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_outcome_mismatch",
                "response outcome kind does not match the invoked operation",
            ),
            ShapeLimitExceeded::Evidence => envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_evidence_mismatch",
                "response evidence does not conform to the validated output schema",
            ),
        });
    }
    result.map_err(|_| {
        envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_reply_malformed",
            "remote response outcome is malformed",
        )
    })
}

#[derive(Clone, Copy)]
enum ScalarScanKind {
    Count,
    Exists,
}

impl ScalarScanKind {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Exists => "exists",
        }
    }
}

struct ScalarOutcomeSeed<'scan> {
    kind: ScalarScanKind,
    max_items: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for ScalarOutcomeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ScalarOutcomeVisitor { seed: self })
    }
}

struct ScalarOutcomeVisitor<'scan> {
    seed: ScalarOutcomeSeed<'scan>,
}

impl<'de> Visitor<'de> for ScalarOutcomeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an exact count or exists remote outcome object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut found_kind = false;
        let mut found_value = false;
        while let Some(key) = map.next_key::<&str>()? {
            if key == "kind" {
                if found_kind {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote scalar outcome kind is duplicated",
                    ));
                }
                found_kind = true;
                if map.next_value::<&str>()? != self.seed.kind.wire_name() {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote scalar outcome kind does not match the expected operation",
                    ));
                }
            } else if key == "value" {
                if found_value {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote scalar outcome value is duplicated",
                    ));
                }
                found_value = true;
                let value = match self.seed.kind {
                    ScalarScanKind::Count => map.next_value::<u64>().map(|value| {
                        if value > self.seed.max_items {
                            self.seed.state.exceeded = Some(ShapeLimitExceeded::Items);
                        }
                    }),
                    ScalarScanKind::Exists => map.next_value::<bool>().map(|value| {
                        if value && self.seed.max_items == 0 {
                            self.seed.state.exceeded = Some(ShapeLimitExceeded::Items);
                        }
                    }),
                };
                if value.is_err() {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Evidence);
                }
                value?;
            } else {
                self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                return Err(serde::de::Error::custom(
                    "remote scalar outcome carries an unexpected field",
                ));
            }
        }
        if found_kind && found_value {
            Ok(())
        } else {
            self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
            Err(serde::de::Error::custom(
                "remote scalar outcome does not carry its exact fields",
            ))
        }
    }
}

struct OutcomeShapeSeed<'scan> {
    field: &'static str,
    kind: SequenceScanKind,
    width: usize,
    max_items: u64,
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for OutcomeShapeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(OutcomeShapeVisitor { seed: self })
    }
}

struct OutcomeShapeVisitor<'scan> {
    seed: OutcomeShapeSeed<'scan>,
}

impl<'de> Visitor<'de> for OutcomeShapeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a remote outcome object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut found = false;
        let mut found_kind = false;
        while let Some(key) = map.next_key::<&str>()? {
            if key == self.seed.field {
                if found {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote outcome field is duplicated",
                    ));
                }
                found = true;
                map.next_value_seed(OutcomeSequenceSeed {
                    kind: self.seed.kind,
                    width: self.seed.width,
                    max_items: self.seed.max_items,
                    max_members: self.seed.max_members,
                    state: self.seed.state,
                })?;
            } else if key == "kind" {
                if found_kind {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote outcome kind is duplicated",
                    ));
                }
                found_kind = true;
                if map.next_value::<&str>()? != self.seed.field {
                    self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                    return Err(serde::de::Error::custom(
                        "remote outcome kind does not match the expected shape",
                    ));
                }
            } else {
                self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
                return Err(serde::de::Error::custom(
                    "remote outcome carries an unexpected field",
                ));
            }
        }
        if found && found_kind {
            Ok(())
        } else {
            self.seed.state.exceeded = Some(ShapeLimitExceeded::Outcome);
            Err(serde::de::Error::custom(
                "remote outcome does not carry the expected field",
            ))
        }
    }
}

struct OutcomeSequenceSeed<'scan> {
    kind: SequenceScanKind,
    width: usize,
    max_items: u64,
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for OutcomeSequenceSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(OutcomeSequenceVisitor { seed: self })
    }
}

struct OutcomeSequenceVisitor<'scan> {
    seed: OutcomeSequenceSeed<'scan>,
}

impl<'de> Visitor<'de> for OutcomeSequenceVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a remote row or document sequence")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        loop {
            let next = sequence.next_element_seed(OutputItemSeed {
                kind: self.seed.kind,
                width: self.seed.width,
                max_items: self.seed.max_items,
                max_members: self.seed.max_members,
                state: self.seed.state,
            })?;
            if next.is_none() {
                return Ok(());
            }
        }
    }
}

struct OutputItemSeed<'scan> {
    kind: SequenceScanKind,
    width: usize,
    max_items: u64,
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for OutputItemSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        self.state.items = self.state.items.saturating_add(1);
        if self.state.items > self.max_items {
            self.state.exceeded = Some(ShapeLimitExceeded::Items);
            return Err(serde::de::Error::custom("remote item budget exceeded"));
        }
        deserializer.deserialize_seq(OutputItemVisitor { seed: self })
    }
}

struct OutputItemVisitor<'scan> {
    seed: OutputItemSeed<'scan>,
}

impl<'de> Visitor<'de> for OutputItemVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("one positional remote output item")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut width = 0_usize;
        loop {
            let next = sequence.next_element_seed(OutputFieldSeed {
                kind: self.seed.kind,
                position: &mut width,
                max_width: self.seed.width,
                max_members: self.seed.max_members,
                state: self.seed.state,
            })?;
            if next.is_none() {
                if width == self.seed.width {
                    return Ok(());
                }
                self.seed.state.exceeded = Some(ShapeLimitExceeded::Width);
                return Err(serde::de::Error::custom(
                    "remote output width does not match the validated shape",
                ));
            }
        }
    }
}

struct OutputFieldSeed<'scan> {
    kind: SequenceScanKind,
    position: &'scan mut usize,
    max_width: usize,
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for OutputFieldSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        *self.position = self.position.saturating_add(1);
        if *self.position > self.max_width {
            self.state.exceeded = Some(ShapeLimitExceeded::Width);
            return Err(serde::de::Error::custom("remote output width exceeded"));
        }
        match self.kind {
            SequenceScanKind::Rows => IgnoredAny::deserialize(deserializer).map(|_| ()),
            SequenceScanKind::Documents => deserializer.deserialize_map(DocumentFieldVisitor {
                max_members: self.max_members,
                state: self.state,
            }),
        }
    }
}

struct DocumentFieldVisitor<'scan> {
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> Visitor<'de> for DocumentFieldVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("one remote document field object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut kind = None;
        let mut value = false;
        let mut values = false;
        while let Some(key) = map.next_key::<&str>()? {
            if key == "kind" {
                if kind.is_some() {
                    self.state.exceeded = Some(ShapeLimitExceeded::Evidence);
                    return Err(serde::de::Error::custom(
                        "remote document field kind is duplicated",
                    ));
                }
                kind = Some(map.next_value::<&str>()?);
            } else if key == "value" {
                if value {
                    self.state.exceeded = Some(ShapeLimitExceeded::Evidence);
                    return Err(serde::de::Error::custom(
                        "remote document scalar value is duplicated",
                    ));
                }
                value = true;
                map.next_value::<IgnoredAny>()?;
            } else if key == "values" {
                if values {
                    self.state.exceeded = Some(ShapeLimitExceeded::Evidence);
                    return Err(serde::de::Error::custom(
                        "remote document list values are duplicated",
                    ));
                }
                values = true;
                map.next_value_seed(MemberSequenceSeed {
                    max_members: self.max_members,
                    state: self.state,
                })?;
            } else {
                self.state.exceeded = Some(ShapeLimitExceeded::Evidence);
                return Err(serde::de::Error::custom(
                    "remote document field carries an unexpected member",
                ));
            }
        }
        let exact = match kind {
            Some("absent") => !value && !values,
            Some("scalar") => value && !values,
            Some("list") => !value && values,
            Some(_) | None => false,
        };
        if exact {
            Ok(())
        } else {
            self.state.exceeded = Some(ShapeLimitExceeded::Evidence);
            Err(serde::de::Error::custom(
                "remote document field does not match its declared kind",
            ))
        }
    }
}

struct MemberSequenceSeed<'scan> {
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for MemberSequenceSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(MemberSequenceVisitor { seed: self })
    }
}

struct MemberSequenceVisitor<'scan> {
    seed: MemberSequenceSeed<'scan>,
}

impl<'de> Visitor<'de> for MemberSequenceVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a remote document member sequence")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        loop {
            let next = sequence.next_element_seed(MemberSeed {
                max_members: self.seed.max_members,
                state: self.seed.state,
            })?;
            if next.is_none() {
                return Ok(());
            }
        }
    }
}

struct MemberSeed<'scan> {
    max_members: u64,
    state: &'scan mut ShapeScanState,
}

impl<'de> DeserializeSeed<'de> for MemberSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        self.state.members = self.state.members.saturating_add(1);
        if self.state.members > self.max_members {
            self.state.exceeded = Some(ShapeLimitExceeded::Members);
            return Err(serde::de::Error::custom(
                "remote document member budget exceeded",
            ));
        }
        IgnoredAny::deserialize(deserializer).map(|_| ())
    }
}

fn peek_remote_reply_binding(bytes: &[u8]) -> Result<RemoteReplyBindingPeek<'_>, Diagnostic> {
    if bytes.len() > MAX_REMOTE_ENVELOPE_BYTES {
        return Err(envelope_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_envelope_too_large",
            "remote reply exceeds the envelope byte ceiling",
        ));
    }
    serde_json::from_slice(bytes).map_err(|_| {
        envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_reply_malformed",
            "remote reply is not a JSON envelope with a format discriminator",
        )
    })
}

fn verify_response_binding(
    nonce: Option<&str>,
    plan: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprint,
) -> Result<(), Diagnostic> {
    if nonce != Some(expected_nonce) {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_nonce_mismatch",
            "response evidence does not echo the request nonce",
        ));
    }
    let Some(plan) = plan else {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_plan_mismatch",
            "response evidence does not bind the invoked plan",
        ));
    };
    let echoed = FingerprintDigest::from_hex(plan)?;
    if echoed != expected_plan.as_fingerprint().digest() {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_plan_mismatch",
            "response evidence does not bind the invoked plan",
        ));
    }
    let Some(request) = request else {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_request_mismatch",
            "response evidence does not bind the exact request envelope",
        ));
    };
    let echoed_request = FingerprintDigest::from_hex(request)?;
    if echoed_request != expected_request.as_fingerprint().digest() {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_request_mismatch",
            "response evidence does not bind the exact request envelope",
        ));
    }
    Ok(())
}

fn verify_failure_binding(
    nonce: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_request: &RemoteRequestFingerprint,
) -> Result<(), Diagnostic> {
    let nonce = nonce.ok_or_else(|| {
        envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_failure_unbound",
            "failure evidence is not bound to the request nonce",
        )
    })?;
    if nonce != expected_nonce {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_nonce_mismatch",
            "failure evidence does not echo the request nonce",
        ));
    }
    let request = request.ok_or_else(|| {
        envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_failure_unbound",
            "failure evidence is not bound to the request envelope",
        )
    })?;
    let echoed = FingerprintDigest::from_hex(request)?;
    if echoed != expected_request.as_fingerprint().digest() {
        return Err(envelope_failure(
            DiagnosticCategory::Integrity,
            "query_remote_request_mismatch",
            "failure evidence does not bind the exact request envelope",
        ));
    }
    Ok(())
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
    value
        .map(|value| {
            let value = checked_remote_limit(value)?;
            if value > MAX_REMOTE_DEADLINE_MS {
                return Err(remote_deadline_limit());
            }
            Ok(value)
        })
        .transpose()
}

fn validate_remote_limits(limits: RemoteLimits) -> Result<(), Diagnostic> {
    if limits
        .deadline_ms
        .is_some_and(|value| value > MAX_REMOTE_DEADLINE_MS)
    {
        return Err(remote_deadline_limit());
    }
    Ok(())
}

fn validate_remote_time_shape(
    prepared_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    limits: RemoteLimits,
) -> Result<(), Diagnostic> {
    let lifetime = expires_at_unix_ms
        .checked_sub(prepared_at_unix_ms)
        .ok_or_else(remote_time_invalid)?;
    let declared_lifetime = limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS);
    if lifetime != declared_lifetime {
        return Err(remote_time_invalid());
    }
    Ok(())
}

fn remote_time_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_time_invalid",
        "remote request timestamps do not form a bounded absolute lifetime",
    )
}

/// Stable rejection for a deadline outside the supported monotonic range.
#[must_use]
pub fn remote_deadline_limit() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_deadline_limit",
        "remote deadline exceeds the maximum supported duration",
    )
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

fn validate_executor_component(value: &str) -> Result<(), Diagnostic> {
    let valid = (EXECUTOR_COMPONENT_MIN_BYTES..=EXECUTOR_COMPONENT_MAX_BYTES)
        .contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_executor_invalid",
            "executor identity and epoch are 16-128 safe ASCII bytes",
        ))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut decoded = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_reply_binding_peek_borrows_correlation_strings() {
        let bytes = br#"{"format":"typebridge.query-remote-response/v1","nonce":"remote-nonce-0123456789abcdef","outcome":{"kind":"exists","value":true},"plan":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","request":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#;
        let peek: RemoteReplyBindingPeek<'_> = serde_json::from_slice(bytes).expect("binding peek");

        assert!(bytes.as_ptr_range().contains(&peek.format.as_ptr()));
        assert!(
            peek.nonce
                .is_some_and(|value| bytes.as_ptr_range().contains(&value.as_ptr()))
        );
        assert!(
            peek.plan
                .is_some_and(|value| bytes.as_ptr_range().contains(&value.as_ptr()))
        );
        assert!(
            peek.request
                .is_some_and(|value| bytes.as_ptr_range().contains(&value.as_ptr()))
        );
    }
}
