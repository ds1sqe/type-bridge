//! Remote execution of validated query plans over the versioned envelope.
//!
//! Both halves of one plan/result contract live here. The client half
//! encodes a validated invocation into a [`RemoteQueryRequest`] and
//! evidence-validates the returned outcome against the same derived output
//! schema the local executor uses — oversized, replayed, foreign, or
//! mistyped evidence is rejected before any host object is constructed.
//! The server half decodes a request, re-validates the carried plan against
//! its own schema authority (staleness), checks advertised capabilities,
//! binds the request to the advertised executor epoch and absolute expiry,
//! tightens caller budgets under its ceilings, executes through the local
//! engine, and encodes the typed outcome. Transport stays out: callers move
//! the envelope bytes however they like.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::Serialize;

use type_bridge_contract::codec::to_canonical_json_with_limits;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::limits::{
    MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES, REMOTE_ENVELOPE_CODEC_LIMITS,
    StructuralLimits,
};
use type_bridge_contract::query_plan::{QueryInvocation, QueryOperation, QueryPlanFingerprint};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteCapabilitiesFingerprint, RemoteFieldValue, RemoteLimits,
    RemoteOutcome, RemoteOutcomeShape, RemoteQueryFailure, RemoteQueryRequest, RemoteQueryResponse,
    RemoteReply, RemoteReplyDecodeLimits, RemoteReplySignature, RemoteReplySigner,
    RemoteReplySigningDigest, RemoteReplyVerifier, RemoteRequestFingerprint,
    RemoteSigningPublicKey, RemoteValue, decode_remote_reply,
};
use type_bridge_contract::query_remote_v2::{
    RemoteLimitsV2, RemoteOutcomeV2, RemoteQueryFailureV2, RemoteQueryRequestV2,
    RemoteQueryResponseV2, RemoteReplyDecodeLimitsV2, RemoteReplyV2, RemoteRequestFingerprintV2,
    RemoteResultKindV2, decode_remote_reply_v2, validate_remote_outcome_v2,
};
use type_bridge_query::{
    DocumentColumnShape, MigrationAssertionValidationContext, OutputSchema, ValidatedQuery,
    validate_query_plan,
};

use crate::query_v2::{
    DocumentFieldValue, QueryResultDocument, QueryResultRow, QueryRowValue, QueryV2Outcome,
    QueryV2ValidatedItemObserver, execute_with_provider_observer, failure,
    preflight_invocation_transport,
};
use crate::session::backend::{BoundedAnswerLimits, QueryV2AnswerLimits};

/// Process- or deployment-epoch Ed25519 key used only for V2 reply authentication.
#[derive(Clone)]
pub struct RemoteReplySigningKey(Arc<SigningKey>);

impl RemoteReplySigningKey {
    /// Generate one fresh signer for a standalone executor lifetime.
    #[must_use]
    pub fn generate() -> Self {
        Self(Arc::new(SigningKey::generate(&mut OsRng)))
    }

    /// Construct a signer from an explicitly managed secret seed.
    ///
    /// This supports a shared execution epoch whose replay store and signer are
    /// provisioned together. The secret is never exposed by `Debug` or errors.
    #[must_use]
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self(Arc::new(SigningKey::from_bytes(&secret)))
    }

    /// Return the exact public key carried by the capability advertisement.
    #[must_use]
    pub fn public_key(&self) -> RemoteSigningPublicKey {
        RemoteSigningPublicKey::from_bytes(self.0.verifying_key().to_bytes())
    }
}

impl std::fmt::Debug for RemoteReplySigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteReplySigningKey")
            .field("public_key", &self.public_key())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl RemoteReplySigner for RemoteReplySigningKey {
    fn public_key(&self) -> RemoteSigningPublicKey {
        self.public_key()
    }

    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature {
        RemoteReplySignature::from_bytes(self.0.sign(digest.as_bytes()).to_bytes())
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// Stateless Ed25519 verifier for authenticated remote reply digests.
pub struct Ed25519RemoteReplyVerifier;

impl RemoteReplyVerifier for Ed25519RemoteReplyVerifier {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(key.as_bytes()) else {
            return false;
        };
        key.verify_strict(
            digest.as_bytes(),
            &Signature::from_bytes(signature.as_bytes()),
        )
        .is_ok()
    }
}

/// Explicit request format selected before any version-specific reconstruction.
///
/// Unknown and malformed envelopes remain on the historical V1 rejection path
/// so adding V2 does not change the diagnostic bytes returned for traffic that
/// the existing endpoint already rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteRequestFormat {
    /// Released V1 request/reply contract.
    V1,
    /// Additive V2 request/reply contract.
    V2,
}

const REMOTE_REQUEST_FORMAT_PREFIX_BYTES: usize = 256;
const REMOTE_ADVERTISEMENT_HEX_BYTES: usize = 64;
const REMOTE_REQUEST_FORMAT_V2_FIELD: &[u8] =
    b",\"format\":\"typebridge.query-remote-request/v2\",";

/// Dispatch a request by its canonical leading fields before decoding its plan.
///
/// Canonical request objects sort `advertisement`, `expires_at_unix_ms`, and
/// `format` before the potentially multi-megabyte plan. Classification reads
/// at most a fixed 256-byte prefix and performs no allocation,
/// so an attacker cannot make Tokio workers parse the full body merely to
/// select a decoder. Only the exact canonical V2 prefix selects V2; unknown,
/// reordered, or malformed format prefixes retain the historical V1 path.
#[must_use]
pub fn remote_request_format(bytes: &[u8]) -> RemoteRequestFormat {
    if bytes.len() > MAX_REMOTE_ENVELOPE_BYTES {
        return RemoteRequestFormat::V1;
    }
    let prefix = &bytes[..bytes.len().min(REMOTE_REQUEST_FORMAT_PREFIX_BYTES)];
    let Some(mut rest) = prefix.strip_prefix(br#"{"advertisement":""#) else {
        return RemoteRequestFormat::V1;
    };
    let Some((advertisement, suffix)) = rest.split_at_checked(REMOTE_ADVERTISEMENT_HEX_BYTES)
    else {
        return RemoteRequestFormat::V1;
    };
    if !advertisement
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return RemoteRequestFormat::V1;
    }
    rest = suffix;
    let Some(suffix) = rest.strip_prefix(br#"","expires_at_unix_ms":"#) else {
        return RemoteRequestFormat::V1;
    };
    rest = suffix;
    let digit_count = rest
        .iter()
        .take(20)
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count == 0
        || (digit_count > 1 && rest.first() == Some(&b'0'))
        || rest.get(digit_count) != Some(&b',')
    {
        return RemoteRequestFormat::V1;
    }
    rest = &rest[digit_count..];
    if rest.starts_with(REMOTE_REQUEST_FORMAT_V2_FIELD) {
        RemoteRequestFormat::V2
    } else {
        // This intentionally includes V1, unknown, reordered, and malformed
        // prefixes. V1's existing decoder remains their compatibility
        // authority.
        RemoteRequestFormat::V1
    }
}

struct RemoteResponseWireMeter {
    encoded_bytes: u64,
    items: u64,
    max_bytes: u64,
}

impl RemoteResponseWireMeter {
    fn new(
        nonce: &str,
        plan: &QueryPlanFingerprint,
        request: &RemoteRequestFingerprint,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
        outcome: RemoteOutcome,
        max_bytes: u64,
    ) -> Result<Self, Diagnostic> {
        let response = RemoteQueryResponse::new(nonce, plan, request, outcome)?;
        let encoded_bytes = u64::try_from(response.signed_encoded_len(advertisement, key)?)
            .map_err(|_| response_oversized())?;
        if encoded_bytes > max_bytes {
            return Err(response_oversized());
        }
        Ok(Self {
            encoded_bytes,
            items: 0,
            max_bytes,
        })
    }

    fn add_item<T: Serialize>(&mut self, item: &T) -> Result<(), Diagnostic> {
        let encoded = to_canonical_json_with_limits(item, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        let item_bytes = u64::try_from(encoded.len()).map_err(|_| response_oversized())?;
        let separator = u64::from(self.items != 0);
        let next = self
            .encoded_bytes
            .checked_add(separator)
            .and_then(|value| value.checked_add(item_bytes))
            .ok_or_else(response_oversized)?;
        if next > self.max_bytes {
            return Err(response_oversized());
        }
        self.encoded_bytes = next;
        self.items = self.items.saturating_add(1);
        Ok(())
    }
}

impl QueryV2ValidatedItemObserver for RemoteResponseWireMeter {
    fn observe_row(&mut self, row: &QueryResultRow) -> Result<(), Diagnostic> {
        self.add_item(&remote_row(row))
    }

    fn observe_document(&mut self, document: &QueryResultDocument) -> Result<(), Diagnostic> {
        self.add_item(&remote_document(document))
    }
}

struct RemoteResponseWireMeterV2 {
    encoded_bytes: u64,
    items: u64,
    max_bytes: u64,
}

impl RemoteResponseWireMeterV2 {
    #[expect(
        clippy::too_many_arguments,
        reason = "the exact signed-wire floor binds every V2 correlation field"
    )]
    fn new(
        nonce: &str,
        plan: &type_bridge_contract::query_plan::QueryPlan,
        request: &RemoteRequestFingerprintV2,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &RemoteReplySigningKey,
        result: RemoteResultKindV2,
        outcome: RemoteOutcomeV2,
        max_bytes: u64,
    ) -> Result<Self, Diagnostic> {
        let response = RemoteQueryResponseV2::new(nonce, plan, request, result, outcome)?;
        let encoded_bytes = u64::try_from(response.encode_signed(advertisement, signer)?.len())
            .map_err(|_| response_oversized_v2())?;
        if encoded_bytes > max_bytes {
            return Err(response_oversized_v2());
        }
        Ok(Self {
            encoded_bytes,
            items: 0,
            max_bytes,
        })
    }

    fn add_item<T: Serialize>(&mut self, item: &T) -> Result<(), Diagnostic> {
        let encoded = to_canonical_json_with_limits(item, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        let item_bytes = u64::try_from(encoded.len()).map_err(|_| response_oversized_v2())?;
        let separator = u64::from(self.items != 0);
        let next = self
            .encoded_bytes
            .checked_add(separator)
            .and_then(|value| value.checked_add(item_bytes))
            .ok_or_else(response_oversized_v2)?;
        if next > self.max_bytes {
            return Err(response_oversized_v2());
        }
        self.encoded_bytes = next;
        self.items = self.items.saturating_add(1);
        Ok(())
    }
}

impl QueryV2ValidatedItemObserver for RemoteResponseWireMeterV2 {
    fn observe_row(&mut self, row: &QueryResultRow) -> Result<(), Diagnostic> {
        self.add_item(&remote_row(row))
    }

    fn observe_document(&mut self, document: &QueryResultDocument) -> Result<(), Diagnostic> {
        self.add_item(&remote_document(document))
    }
}

/// Encode one validated invocation into request envelope bytes.
pub fn encode_remote_request(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
    limits: RemoteLimits,
    nonce: impl Into<String>,
) -> Result<Vec<u8>, Diagnostic> {
    encode_remote_request_at(
        validated,
        invocation,
        advertisement,
        limits,
        nonce,
        unix_time_ms()?,
    )
}

/// Encode one request at an explicit wall-clock sample.
///
/// This is primarily useful to test expiry/skew boundaries without sleeping;
/// normal callers use [`encode_remote_request`].
pub fn encode_remote_request_at(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
    limits: RemoteLimits,
    nonce: impl Into<String>,
    prepared_at_unix_ms: u64,
) -> Result<Vec<u8>, Diagnostic> {
    preflight_invocation_transport(validated.plan(), invocation)?;
    RemoteQueryRequest::new(
        validated.plan(),
        invocation,
        advertisement,
        limits,
        nonce,
        prepared_at_unix_ms,
    )?
    .encode()
}

/// Encode one validated low-level V2 invocation into request-envelope bytes.
///
/// Model-query terminals use the same envelope but are prepared only after the
/// production compatibility executor advertises their capabilities.
pub fn encode_remote_request_v2(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
    limits: RemoteLimitsV2,
    nonce: impl Into<String>,
) -> Result<Vec<u8>, Diagnostic> {
    encode_remote_request_v2_at(
        validated,
        invocation,
        advertisement,
        limits,
        nonce,
        unix_time_ms()?,
    )
}

/// Encode one V2 request at an explicit wall-clock sample.
pub fn encode_remote_request_v2_at(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
    limits: RemoteLimitsV2,
    nonce: impl Into<String>,
    prepared_at_unix_ms: u64,
) -> Result<Vec<u8>, Diagnostic> {
    preflight_invocation_transport(validated.plan(), invocation)?;
    let result = remote_result_kind_v2(validated, invocation)?;
    RemoteQueryRequestV2::new(
        validated.plan(),
        invocation,
        result,
        advertisement,
        limits,
        nonce,
        prepared_at_unix_ms,
    )?
    .encode()
}

fn remote_result_kind_v2(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
) -> Result<RemoteResultKindV2, Diagnostic> {
    if let Some(model_query) = validated
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    {
        return Ok(match model_query {
            type_bridge_contract::query_plan::ModelQueryV2::Rows { .. } => {
                RemoteResultKindV2::HydratedRows
            }
            type_bridge_contract::query_plan::ModelQueryV2::Page { .. } => {
                RemoteResultKindV2::HydratedPage
            }
            type_bridge_contract::query_plan::ModelQueryV2::DistinctCount { .. } => {
                RemoteResultKindV2::DistinctCount
            }
            type_bridge_contract::query_plan::ModelQueryV2::DistinctExists { .. } => {
                RemoteResultKindV2::DistinctExists
            }
        });
    }
    match (invocation.operation(), validated.output_schema()) {
        (QueryOperation::Rows, OutputSchema::Rows(_)) => Ok(RemoteResultKindV2::Rows),
        (QueryOperation::Rows, OutputSchema::Documents(_)) => Ok(RemoteResultKindV2::Documents),
        (QueryOperation::Count, _) => Ok(RemoteResultKindV2::Count),
        (QueryOperation::Exists, _) => Ok(RemoteResultKindV2::Exists),
    }
}

/// Refuse one invocation the executor's advertisement cannot execute.
///
/// The contract promises clients check required capabilities against the
/// exact advertisement and refuse unsupported plans before any I/O. Both
/// the plan's derived capabilities and the invocation's transport
/// capabilities (batches, explicit absence, and exact datetime-tz `given`
/// values) are checked; the executor re-checks the same sets at admission.
pub fn check_advertised_capabilities(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
) -> Result<(), Diagnostic> {
    preflight_invocation_transport(validated.plan(), invocation)?;
    let advertised = advertisement.capabilities();
    for capability in validated.plan().required_capabilities().iter() {
        if !advertised.contains(capability) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_capability_unsupported",
                "the plan requires a capability this executor does not advertise",
            ));
        }
    }
    for capability in invocation.transport_capabilities().iter() {
        if !advertised.contains(capability) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_capability_unsupported",
                "the invocation requires a transport capability this executor does not advertise",
            ));
        }
    }
    Ok(())
}

/// Refuse one V2 invocation the exact executor advertisement cannot execute.
pub fn check_advertised_capabilities_v2(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
) -> Result<(), Diagnostic> {
    preflight_invocation_transport(validated.plan(), invocation)?;
    // Constructing a bounded request is the contract-owned exhaustive
    // capability check. Use fixed safe values; no bytes leave this function.
    RemoteQueryRequestV2::new(
        validated.plan(),
        invocation,
        remote_result_kind_v2(validated, invocation)?,
        advertisement,
        RemoteLimitsV2 {
            deadline_ms: None,
            max_bytes: 0,
            max_items: 0,
            max_collection_members: 0,
            max_graph_nodes: 0,
            max_attribute_values: 0,
            max_role_players: 0,
        },
        "capability-check-0000000000000000",
        0,
    )
    .map(|_| ())
}

/// Decode and evidence-validate one response into a typed outcome.
///
/// The caller supplies the exact validated plan, the operation it invoked,
/// the nonce it sent, and its budgets. Every rejection here happens before
/// host object construction.
#[expect(
    clippy::too_many_arguments,
    reason = "the trust-boundary API keeps every expected binding and budget explicit"
)]
pub fn decode_remote_outcome(
    bytes: &[u8],
    validated: &ValidatedQuery,
    operation: QueryOperation,
    nonce: &str,
    request: &RemoteRequestFingerprint,
    advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    limits: RemoteLimits,
) -> Result<QueryV2Outcome, Diagnostic> {
    let fingerprint = QueryPlanFingerprint::compute(validated.plan())?;
    let shape = match (operation, validated.output_schema()) {
        (QueryOperation::Rows, OutputSchema::Rows(schema)) => RemoteOutcomeShape::Rows {
            width: schema.columns().len(),
        },
        (QueryOperation::Rows, OutputSchema::Documents(schema)) => RemoteOutcomeShape::Documents {
            width: schema.columns().len(),
        },
        (QueryOperation::Count, _) => RemoteOutcomeShape::Count,
        (QueryOperation::Exists, _) => RemoteOutcomeShape::Exists,
    };
    let response = match decode_remote_reply(
        bytes,
        nonce,
        &fingerprint,
        request,
        advertisement,
        trusted_key,
        RemoteReplyDecodeLimits {
            shape,
            max_bytes: limits.max_bytes,
            max_items: limits.max_items,
            max_collection_members: limits.max_collection_members,
        },
        &Ed25519RemoteReplyVerifier,
    )? {
        RemoteReply::Response(response) => response,
        // A request-bound failure surfaces its stable server diagnostic
        // instead of collapsing into a generic decode error.
        RemoteReply::Failure(failure) => return Err(failure.diagnostic()?),
    };
    match (operation, response.into_outcome()) {
        (QueryOperation::Rows, RemoteOutcome::Rows { rows }) => {
            let OutputSchema::Rows(schema) = validated.output_schema() else {
                return Err(outcome_mismatch());
            };
            if u64::try_from(rows.len()).unwrap_or(u64::MAX) > limits.max_items {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_response_oversized",
                    "response rows exceed the caller item budget",
                ));
            }
            let mut validated_rows = Vec::with_capacity(rows.len());
            for row in rows {
                if row.len() != schema.columns().len() {
                    return Err(evidence_mismatch());
                }
                let values = schema
                    .columns()
                    .iter()
                    .zip(row)
                    .map(|(column, value)| {
                        let domain = column.domain();
                        match value {
                            RemoteValue::Absent if column.optional() => Ok(QueryRowValue::Absent),
                            RemoteValue::Value { value }
                                if domain.type_ids().is_empty()
                                    && domain.value_type() == Some(value.value_type()) =>
                            {
                                Ok(QueryRowValue::Value { value })
                            }
                            RemoteValue::Attribute { type_id, value }
                                if domain.type_ids().contains(&type_id)
                                    && domain.value_type() == Some(value.value_type()) =>
                            {
                                Ok(QueryRowValue::Attribute { type_id, value })
                            }
                            RemoteValue::Thing { iid, type_id }
                                if domain.type_ids().contains(&type_id)
                                    && domain.value_type().is_none()
                                    && type_bridge_contract::id::is_canonical_thing_iid(&iid) =>
                            {
                                Ok(QueryRowValue::Thing { type_id, iid })
                            }
                            _ => Err(evidence_mismatch()),
                        }
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                validated_rows.push(QueryResultRow::from_values(values));
            }
            Ok(QueryV2Outcome::Rows(validated_rows))
        }
        (QueryOperation::Rows, RemoteOutcome::Documents { documents }) => {
            let OutputSchema::Documents(schema) = validated.output_schema() else {
                return Err(outcome_mismatch());
            };
            if u64::try_from(documents.len()).unwrap_or(u64::MAX) > limits.max_items {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_response_oversized",
                    "response documents exceed the caller item budget",
                ));
            }
            let mut validated_documents = Vec::with_capacity(documents.len());
            let mut collection_members = 0_u64;
            for document in documents {
                if document.len() != schema.columns().len() {
                    return Err(evidence_mismatch());
                }
                let values = schema
                    .columns()
                    .iter()
                    .zip(document)
                    .map(|(column, value)| match (column.shape(), value) {
                        (
                            DocumentColumnShape::Scalar { optional: true, .. },
                            RemoteFieldValue::Absent,
                        ) => Ok(DocumentFieldValue::Absent),
                        (
                            DocumentColumnShape::Scalar { value_type, .. },
                            RemoteFieldValue::Scalar { value },
                        ) if *value_type == value.value_type() => {
                            Ok(DocumentFieldValue::Scalar(value))
                        }
                        (
                            DocumentColumnShape::List { element_type, .. },
                            RemoteFieldValue::List { values },
                        ) if values
                            .iter()
                            .all(|value| value.value_type() == *element_type) => {
                                let field_members = u64::try_from(values.len()).map_err(|_| {
                                    failure(
                                        DiagnosticCategory::ResourceLimit,
                                        "query_v2_document_member_limit",
                                        "document list member count exceeds the supported counter range",
                                    )
                                })?;
                                collection_members = collection_members
                                    .checked_add(field_members)
                                    .ok_or_else(|| {
                                        failure(
                                            DiagnosticCategory::ResourceLimit,
                                            "query_v2_document_member_limit",
                                            "document list member counter overflowed",
                                        )
                                    })?;
                                if collection_members > limits.max_collection_members {
                                    return Err(failure(
                                        DiagnosticCategory::ResourceLimit,
                                        "query_v2_document_member_limit",
                                        "document lists exceed the aggregate member ceiling",
                                    ));
                                }
                                Ok(DocumentFieldValue::List(values))
                            }
                        _ => Err(evidence_mismatch()),
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                validated_documents.push(QueryResultDocument::from_values(values));
            }
            Ok(QueryV2Outcome::Documents(validated_documents))
        }
        (QueryOperation::Count, RemoteOutcome::Count { value }) => {
            if value > limits.max_items {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_response_oversized",
                    "response count exceeds the caller item budget",
                ));
            }
            Ok(QueryV2Outcome::Count(value))
        }
        (QueryOperation::Exists, RemoteOutcome::Exists { value }) => {
            if value && limits.max_items == 0 {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_response_oversized",
                    "a true existence response exceeds the caller item budget",
                ));
            }
            Ok(QueryV2Outcome::Exists(value))
        }
        _ => Err(outcome_mismatch()),
    }
}

/// Authenticate, correlate, and evidence-validate one additive V2 reply.
///
/// The contract decoder performs signature and binding checks before typed
/// outcome allocation. Returning the contract-owned outcome keeps model
/// construction behind the later registry-validation boundary.
pub fn decode_remote_outcome_v2(
    bytes: &[u8],
    request_envelope: &RemoteQueryRequestV2,
    validated: &ValidatedQuery,
    request: &RemoteRequestFingerprintV2,
    advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    limits: RemoteReplyDecodeLimitsV2,
) -> Result<RemoteOutcomeV2, Diagnostic> {
    let plan = validated.plan().fingerprint()?;
    match decode_remote_reply_v2(
        bytes,
        request_envelope,
        &plan,
        request,
        advertisement,
        trusted_key,
        limits,
        &Ed25519RemoteReplyVerifier,
    )? {
        RemoteReplyV2::Response(response) => Ok(response.into_outcome()),
        RemoteReplyV2::Failure(failure) => Err(failure.diagnostic()?),
    }
}

/// A remote request proven admissible without any provider resource.
///
/// Constructed only by [`preflight_remote_request`], which has no access
/// to a transaction or provider by signature: decode, capability,
/// schema/staleness, invocation, and budget rejection all happen before
/// any provider resource can exist.
pub struct AdmittedRemoteRequest {
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    byte_budget: u64,
    invocation: QueryInvocation,
    limits: QueryV2AnswerLimits,
    nonce: String,
    plan_fingerprint: QueryPlanFingerprint,
    provider_byte_budget: u64,
    replay_until: Instant,
    request_fingerprint: RemoteRequestFingerprint,
    validated: ValidatedQuery,
}

impl AdmittedRemoteRequest {
    /// Borrow the exact schema-validated plan for typed transport policy.
    #[must_use]
    pub fn plan(&self) -> &type_bridge_contract::query_plan::QueryPlan {
        self.validated.plan()
    }

    /// Borrow the exact operation and input rows for typed transport policy.
    #[must_use]
    pub const fn invocation(&self) -> &QueryInvocation {
        &self.invocation
    }

    /// Borrow the canonical plan fingerprint bound into a successful reply.
    #[must_use]
    pub const fn plan_fingerprint(&self) -> &QueryPlanFingerprint {
        &self.plan_fingerprint
    }

    /// Borrow the whole-request fingerprint bound into every admitted reply.
    #[must_use]
    pub const fn request_fingerprint(&self) -> &RemoteRequestFingerprint {
        &self.request_fingerprint
    }

    /// Return the caller-supplied nonce for failure envelopes produced
    /// after admission (for example a provider that cannot open).
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Return the effective absolute execution deadline after tightening.
    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.limits.answer.deadline
    }

    /// Return the allocation-free client decode limits for this admitted reply.
    #[must_use]
    pub fn reply_decode_limits(&self) -> RemoteReplyDecodeLimits {
        let shape = match (self.invocation.operation(), self.validated.output_schema()) {
            (QueryOperation::Rows, OutputSchema::Rows(schema)) => RemoteOutcomeShape::Rows {
                width: schema.columns().len(),
            },
            (QueryOperation::Rows, OutputSchema::Documents(schema)) => {
                RemoteOutcomeShape::Documents {
                    width: schema.columns().len(),
                }
            }
            (QueryOperation::Count, _) => RemoteOutcomeShape::Count,
            (QueryOperation::Exists, _) => RemoteOutcomeShape::Exists,
        };
        RemoteReplyDecodeLimits {
            shape,
            max_bytes: self.byte_budget,
            max_items: self.limits.answer.max_items,
            max_collection_members: self.limits.max_collection_members,
        }
    }

    /// Return the replay-retention horizon derived from absolute request expiry.
    #[must_use]
    pub const fn replay_until(&self) -> Instant {
        self.replay_until
    }

    /// Encode a post-admission failure bound to this exact request.
    #[must_use]
    pub fn bound_failure(
        &self,
        diagnostic: &Diagnostic,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        RemoteQueryFailure::bound(self.nonce.clone(), &self.request_fingerprint, diagnostic)
            .encode_signed_or_fallback(&self.advertisement_fingerprint, signer)
    }
}

/// A provider-independent rejection produced by request preflight.
#[derive(Debug)]
pub struct RemoteRejection {
    diagnostic: Box<Diagnostic>,
    nonce: Option<String>,
    request: Option<Box<RemoteRequestFingerprint>>,
}

impl RemoteRejection {
    fn new(nonce: Option<String>, diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
            nonce,
            request: None,
        }
    }

    fn bound(nonce: String, request: RemoteRequestFingerprint, diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
            nonce: Some(nonce),
            request: Some(Box::new(request)),
        }
    }

    /// Return the stable diagnostic code before consuming this rejection.
    #[must_use]
    pub fn diagnostic_code(&self) -> &str {
        self.diagnostic.code().as_str()
    }

    /// Encode the failure envelope carrying this rejection.
    #[must_use]
    pub fn into_failure_envelope(
        self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        match (self.nonce, self.request) {
            (Some(nonce), Some(request)) => {
                RemoteQueryFailure::bound(nonce, &request, &self.diagnostic)
            }
            (nonce, _) => RemoteQueryFailure::new(nonce, &self.diagnostic),
        }
        .encode_signed_or_fallback(advertisement, signer)
    }
}

/// A provider-independent V2 request admitted under the server's exact
/// schema, advertisement epoch, and monotonically tightened budgets.
pub struct AdmittedRemoteRequestV2 {
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    byte_budget: u64,
    invocation: QueryInvocation,
    limits: QueryV2AnswerLimits,
    nonce: String,
    plan_fingerprint: QueryPlanFingerprint,
    provider_byte_budget: u64,
    replay_until: Instant,
    reply_limits: RemoteReplyDecodeLimitsV2,
    request_envelope: RemoteQueryRequestV2,
    request_fingerprint: RemoteRequestFingerprintV2,
    result: RemoteResultKindV2,
    validated: ValidatedQuery,
}

impl AdmittedRemoteRequestV2 {
    /// Borrow the schema-validated V2 plan for transport policy.
    #[must_use]
    pub fn plan(&self) -> &type_bridge_contract::query_plan::QueryPlan {
        self.validated.plan()
    }

    /// Borrow the exact operation and input rows.
    #[must_use]
    pub const fn invocation(&self) -> &QueryInvocation {
        &self.invocation
    }

    /// Return the caller nonce.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Return the effective absolute execution deadline.
    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.limits.answer.deadline
    }

    /// Return the replay-retention horizon derived from absolute expiry.
    #[must_use]
    pub const fn replay_until(&self) -> Instant {
        self.replay_until
    }

    /// Return authenticated V2 reply decode budgets.
    #[must_use]
    pub const fn reply_decode_limits(&self) -> RemoteReplyDecodeLimitsV2 {
        self.reply_limits
    }

    /// Encode a complete structured post-admission failure.
    #[must_use]
    pub fn bound_failure(
        &self,
        diagnostic: &Diagnostic,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        encode_bound_remote_failure_v2(
            &self.nonce,
            &self.request_fingerprint,
            diagnostic,
            &self.advertisement_fingerprint,
            signer,
        )
    }
}

/// A provider-independent V2 rejection retaining safe request correlation.
#[derive(Debug)]
pub struct RemoteRejectionV2 {
    diagnostic: Box<Diagnostic>,
    nonce: Option<String>,
    request: Option<RemoteRequestFingerprintV2>,
}

impl RemoteRejectionV2 {
    fn new(nonce: Option<String>, diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
            nonce,
            request: None,
        }
    }

    fn bound(nonce: String, request: RemoteRequestFingerprintV2, diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
            nonce: Some(nonce),
            request: Some(request),
        }
    }

    /// Return the stable diagnostic code before consuming this rejection.
    #[must_use]
    pub fn diagnostic_code(&self) -> &str {
        self.diagnostic.code().as_str()
    }

    /// Encode the complete structured V2 failure.
    #[must_use]
    pub fn into_failure_envelope(
        self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        match (self.nonce, self.request) {
            (Some(nonce), Some(request)) => encode_bound_remote_failure_v2(
                &nonce,
                &request,
                &self.diagnostic,
                advertisement,
                signer,
            ),
            (nonce, _) => {
                encode_unbound_remote_failure_v2(nonce, &self.diagnostic, advertisement, signer)
            }
        }
    }
}

/// Encode a V2 failure that could not yet bind a complete request.
#[must_use]
pub fn encode_unbound_remote_failure_v2(
    nonce: Option<String>,
    diagnostic: &Diagnostic,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &RemoteReplySigningKey,
) -> Vec<u8> {
    let failure = RemoteQueryFailureV2::new(nonce, diagnostic).unwrap_or_else(|_| {
        RemoteQueryFailureV2::new(None, &remote_v2_internal_failure())
            .expect("fixed V2 internal diagnostic is canonical and bounded")
    });
    failure.encode_signed_or_fallback(advertisement, signer)
}

fn encode_bound_remote_failure_v2(
    nonce: &str,
    request: &RemoteRequestFingerprintV2,
    diagnostic: &Diagnostic,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &RemoteReplySigningKey,
) -> Vec<u8> {
    let failure = RemoteQueryFailureV2::bound(nonce, request, diagnostic).unwrap_or_else(|_| {
        RemoteQueryFailureV2::bound(nonce, request, &remote_v2_internal_failure())
            .expect("admitted V2 bindings and fixed diagnostic remain canonical and bounded")
    });
    failure.encode_signed_or_fallback(advertisement, signer)
}

fn remote_v2_internal_failure() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "query_remote_v2_internal_failure",
        "executor could not encode the original V2 remote failure",
    )
}

/// Run every provider-independent check over one request envelope.
///
/// The plan re-validates against this server's schema authority, so a
/// stale managed fingerprint fails closed; capabilities the server does
/// not advertise reject here; caller budgets tighten under the supplied
/// ceilings and never raise them. No transaction, provider, or other
/// host resource is constructed — rejected traffic costs only CPU.
pub fn preflight_remote_request(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
) -> Result<AdmittedRemoteRequest, RemoteRejection> {
    let clock =
        PreflightClockSample::now().map_err(|diagnostic| RemoteRejection::new(None, diagnostic))?;
    preflight_remote_request_with_clock(request_bytes, context, advertisement, ceilings, clock)
}

/// Run provider-independent preflight at an explicit executor wall clock.
///
/// Expiry and executor-epoch binding are checked before plan validation, policy
/// hooks, replay admission, or provider construction.
pub fn preflight_remote_request_at(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
    now_unix_ms: u64,
) -> Result<AdmittedRemoteRequest, RemoteRejection> {
    let clock = PreflightClockSample {
        monotonic_anchor: Instant::now(),
        now_unix_ms,
    };
    preflight_remote_request_with_clock(request_bytes, context, advertisement, ceilings, clock)
}

#[derive(Clone, Copy)]
struct PreflightClockSample {
    // Sample monotonic time first. Converting the wall-clock remainder from
    // this earlier anchor is conservative if wall-clock sampling stalls.
    monotonic_anchor: Instant,
    now_unix_ms: u64,
}

impl PreflightClockSample {
    fn now() -> Result<Self, Diagnostic> {
        let monotonic_anchor = Instant::now();
        let now_unix_ms = unix_time_ms()?;
        Ok(Self {
            monotonic_anchor,
            now_unix_ms,
        })
    }

    fn deadline_after(self, remaining_ms: u64) -> Option<Instant> {
        self.monotonic_anchor
            .checked_add(Duration::from_millis(remaining_ms))
    }
}

fn preflight_remote_request_with_clock(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
    clock: PreflightClockSample,
) -> Result<AdmittedRemoteRequest, RemoteRejection> {
    let request = RemoteQueryRequest::decode(request_bytes)
        .map_err(|diagnostic| RemoteRejection::new(None, diagnostic))?;
    let nonce = request.nonce().to_owned();
    // Bind every post-decode rejection and all later evidence to the exact
    // received envelope bytes, covering plan, operation, rows, limits, and
    // nonce at once.
    let request_fingerprint = RemoteRequestFingerprint::compute(request_bytes)
        .map_err(|diagnostic| RemoteRejection::new(Some(nonce.clone()), diagnostic))?;
    let fail = |diagnostic: Diagnostic| {
        RemoteRejection::bound(nonce.clone(), request_fingerprint.clone(), diagnostic)
    };

    let replay_remaining_ms = request
        .remaining_lifetime_ms(clock.now_unix_ms)
        .map_err(&fail)?;
    let execution_remaining_ms = request
        .remaining_execution_ms(clock.now_unix_ms)
        .map_err(&fail)?;
    if !request.binds_advertisement(advertisement).map_err(&fail)? {
        return Err(fail(failure(
            DiagnosticCategory::Integrity,
            "query_remote_executor_mismatch",
            "the request does not bind this executor advertisement epoch",
        )));
    }
    let advertisement_fingerprint = advertisement.fingerprint().map_err(&fail)?;
    let replay_until = clock.deadline_after(replay_remaining_ms).ok_or_else(|| {
        fail(failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_deadline_limit",
            "remote expiry exceeds the supported monotonic clock range",
        ))
    })?;
    let execution_until = clock
        .deadline_after(execution_remaining_ms)
        .ok_or_else(|| {
            fail(failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_deadline_limit",
                "remote execution deadline exceeds the supported monotonic clock range",
            ))
        })?;

    let plan = request.plan().map_err(&fail)?;
    let advertised = advertisement.capabilities();
    for capability in plan.required_capabilities().iter() {
        if !advertised.contains(capability) {
            return Err(fail(failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_capability_unsupported",
                "the plan requires a capability this executor does not advertise",
            )));
        }
    }
    let validated =
        validate_query_plan(&plan, context, StructuralLimits::CANONICAL).map_err(&fail)?;
    let invocation = request.invocation(&plan).map_err(&fail)?;
    // Multi-row batches need the native given transport; an executor that
    // cannot transport rows must reject here, not after opening a
    // transaction and asking the provider mid-execution.
    for capability in invocation.transport_capabilities().iter() {
        if !advertised.contains(capability) {
            return Err(fail(failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_capability_unsupported",
                "the invocation requires a transport capability this executor does not advertise",
            )));
        }
    }
    preflight_invocation_transport(&plan, &invocation).map_err(&fail)?;
    let execution_shape = match (invocation.operation(), validated.output_schema()) {
        (QueryOperation::Rows, OutputSchema::Rows(_)) => RemoteExecutionShape::Rows,
        (QueryOperation::Rows, OutputSchema::Documents(_)) => RemoteExecutionShape::Documents,
        (QueryOperation::Count, _) => RemoteExecutionShape::Count,
        (QueryOperation::Exists, _) => RemoteExecutionShape::Exists,
    };
    let provider_byte_budget = ceilings
        .answer
        .max_bytes
        .min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX));
    let limits = tighten_limits(request.limits(), ceilings, execution_until, execution_shape);
    let byte_budget = limits.answer.max_bytes;
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).map_err(&fail)?;
    let minimum_response_bytes = minimum_signed_success_response_len(
        &nonce,
        &plan_fingerprint,
        &request_fingerprint,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        execution_shape,
        limits.answer.max_items,
    )
    .map_err(&fail)?;
    if minimum_response_bytes > byte_budget {
        return Err(fail(response_oversized()));
    }

    Ok(AdmittedRemoteRequest {
        advertisement_fingerprint,
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint,
        provider_byte_budget,
        replay_until,
        request_fingerprint,
        validated,
    })
}

/// Run every provider-independent check over one additive V2 request.
#[expect(
    clippy::result_large_err,
    reason = "the typed rejection retains bounded correlation state needed to sign the exact V2 failure"
)]
pub fn preflight_remote_request_v2(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
) -> Result<AdmittedRemoteRequestV2, RemoteRejectionV2> {
    let clock = PreflightClockSample::now()
        .map_err(|diagnostic| RemoteRejectionV2::new(None, diagnostic))?;
    preflight_remote_request_v2_with_clock(request_bytes, context, advertisement, ceilings, clock)
}

/// Run additive V2 preflight at an explicit executor wall clock.
#[expect(
    clippy::result_large_err,
    reason = "the typed rejection retains bounded correlation state needed to sign the exact V2 failure"
)]
pub fn preflight_remote_request_v2_at(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
    now_unix_ms: u64,
) -> Result<AdmittedRemoteRequestV2, RemoteRejectionV2> {
    preflight_remote_request_v2_with_clock(
        request_bytes,
        context,
        advertisement,
        ceilings,
        PreflightClockSample {
            monotonic_anchor: Instant::now(),
            now_unix_ms,
        },
    )
}

#[expect(
    clippy::result_large_err,
    reason = "the typed rejection retains bounded correlation state needed to sign the exact V2 failure"
)]
fn preflight_remote_request_v2_with_clock(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
    clock: PreflightClockSample,
) -> Result<AdmittedRemoteRequestV2, RemoteRejectionV2> {
    let request_envelope = RemoteQueryRequestV2::decode(request_bytes)
        .map_err(|diagnostic| RemoteRejectionV2::new(None, diagnostic))?;
    let nonce = request_envelope.nonce().to_owned();
    let request_fingerprint = RemoteRequestFingerprintV2::compute(request_bytes)
        .map_err(|diagnostic| RemoteRejectionV2::new(Some(nonce.clone()), diagnostic))?;
    let fail = |diagnostic: Diagnostic| {
        RemoteRejectionV2::bound(nonce.clone(), request_fingerprint.clone(), diagnostic)
    };

    let replay_remaining_ms = request_envelope
        .remaining_lifetime_ms(clock.now_unix_ms)
        .map_err(&fail)?;
    let execution_remaining_ms = request_envelope
        .remaining_execution_ms(clock.now_unix_ms)
        .map_err(&fail)?;
    request_envelope
        .validate_advertisement(advertisement)
        .map_err(&fail)?;
    let advertisement_fingerprint = advertisement.fingerprint().map_err(&fail)?;
    let replay_until = clock.deadline_after(replay_remaining_ms).ok_or_else(|| {
        fail(failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_deadline_limit",
            "V2 remote expiry exceeds the supported monotonic clock range",
        ))
    })?;
    let execution_until = clock
        .deadline_after(execution_remaining_ms)
        .ok_or_else(|| {
            fail(failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_deadline_limit",
                "V2 execution deadline exceeds the supported monotonic clock range",
            ))
        })?;

    let plan = request_envelope.plan().map_err(&fail)?;
    let validated =
        validate_query_plan(&plan, context, StructuralLimits::CANONICAL).map_err(&fail)?;
    let invocation = request_envelope.invocation(&plan).map_err(&fail)?;
    preflight_invocation_transport(&plan, &invocation).map_err(&fail)?;
    let result = remote_result_kind_v2(&validated, &invocation).map_err(&fail)?;
    if result != request_envelope.result_kind() {
        return Err(fail(failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_result_contract_mismatch",
            "validated V2 terminal differs from its request-bound result family",
        )));
    }

    let shape = match result {
        RemoteResultKindV2::Rows
        | RemoteResultKindV2::HydratedRows
        | RemoteResultKindV2::HydratedPage => RemoteExecutionShape::Rows,
        RemoteResultKindV2::Documents => RemoteExecutionShape::Documents,
        RemoteResultKindV2::Count | RemoteResultKindV2::DistinctCount => {
            RemoteExecutionShape::Count
        }
        RemoteResultKindV2::Exists | RemoteResultKindV2::DistinctExists => {
            RemoteExecutionShape::Exists
        }
    };
    let provider_byte_budget = ceilings
        .answer
        .max_bytes
        .min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX));
    let (limits, reply_limits) =
        tighten_limits_v2(request_envelope.limits(), ceilings, execution_until, shape);
    let byte_budget = limits.answer.max_bytes;
    let plan_fingerprint = plan.fingerprint().map_err(&fail)?;
    let minimum_response_bytes = minimum_signed_success_response_len_v2(
        &nonce,
        &plan,
        &request_fingerprint,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        result,
        limits.answer.max_items,
    )
    .map_err(&fail)?;
    if minimum_response_bytes > byte_budget {
        return Err(fail(response_oversized_v2()));
    }

    Ok(AdmittedRemoteRequestV2 {
        advertisement_fingerprint,
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint,
        provider_byte_budget,
        replay_until,
        reply_limits,
        request_envelope,
        request_fingerprint,
        result,
        validated,
    })
}

/// One admitted request selected by explicit wire-format dispatch.
#[expect(
    clippy::large_enum_variant,
    reason = "the short-lived dispatch envelope preserves the allocation-free historical V1 path"
)]
pub enum AdmittedRemoteRequestEnvelope {
    /// Released V1 request.
    V1(AdmittedRemoteRequest),
    /// Additive V2 request.
    V2(AdmittedRemoteRequestV2),
}

impl AdmittedRemoteRequestEnvelope {
    /// Borrow the exact validated plan for policy evaluation.
    #[must_use]
    pub fn plan(&self) -> &type_bridge_contract::query_plan::QueryPlan {
        match self {
            Self::V1(admitted) => admitted.plan(),
            Self::V2(admitted) => admitted.plan(),
        }
    }

    /// Borrow the exact validated invocation for policy evaluation.
    #[must_use]
    pub fn invocation(&self) -> &QueryInvocation {
        match self {
            Self::V1(admitted) => admitted.invocation(),
            Self::V2(admitted) => admitted.invocation(),
        }
    }

    /// Return the request nonce used by the shared replay registry.
    #[must_use]
    pub fn nonce(&self) -> &str {
        match self {
            Self::V1(admitted) => admitted.nonce(),
            Self::V2(admitted) => admitted.nonce(),
        }
    }

    /// Return the monotonically tightened execution deadline.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        match self {
            Self::V1(admitted) => admitted.deadline(),
            Self::V2(admitted) => admitted.deadline(),
        }
    }

    /// Return the absolute replay-retention horizon.
    #[must_use]
    pub fn replay_until(&self) -> Instant {
        match self {
            Self::V1(admitted) => admitted.replay_until(),
            Self::V2(admitted) => admitted.replay_until(),
        }
    }

    /// Encode a request-version-correlated failure.
    #[must_use]
    pub fn bound_failure(
        &self,
        diagnostic: &Diagnostic,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        match self {
            Self::V1(admitted) => admitted.bound_failure(diagnostic, signer),
            Self::V2(admitted) => admitted.bound_failure(diagnostic, signer),
        }
    }

    /// Snapshot exact reply-correlation state before execution consumes admission.
    #[must_use]
    pub fn reply_expectation(&self) -> RemoteReplyExpectation {
        match self {
            Self::V1(admitted) => RemoteReplyExpectation::V1 {
                limits: admitted.reply_decode_limits(),
                nonce: admitted.nonce.clone(),
                plan: admitted.plan_fingerprint.clone(),
                request: admitted.request_fingerprint.clone(),
            },
            Self::V2(admitted) => RemoteReplyExpectation::V2 {
                limits: admitted.reply_decode_limits(),
                plan: admitted.plan_fingerprint.clone(),
                request: admitted.request_fingerprint.clone(),
                request_envelope: admitted.request_envelope.clone(),
            },
        }
    }
}

/// One provider-independent rejection selected by request format.
pub enum RemoteRejectionEnvelope {
    /// Historical V1 rejection.
    V1(RemoteRejection),
    /// Complete structured V2 rejection.
    V2(RemoteRejectionV2),
}

impl RemoteRejectionEnvelope {
    /// Return the stable diagnostic code.
    #[must_use]
    pub fn diagnostic_code(&self) -> &str {
        match self {
            Self::V1(rejection) => rejection.diagnostic_code(),
            Self::V2(rejection) => rejection.diagnostic_code(),
        }
    }

    /// Encode a rejection in the request-selected failure format.
    #[must_use]
    pub fn into_failure_envelope(
        self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        match self {
            Self::V1(rejection) => rejection.into_failure_envelope(advertisement, signer),
            Self::V2(rejection) => rejection.into_failure_envelope(advertisement, signer),
        }
    }
}

/// Dispatch before version-specific reconstruction, then run complete preflight.
#[expect(
    clippy::result_large_err,
    reason = "the versioned rejection retains the request-correlated failure needed by the server boundary"
)]
pub fn preflight_remote_request_versioned(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: QueryV2AnswerLimits,
) -> Result<AdmittedRemoteRequestEnvelope, RemoteRejectionEnvelope> {
    match remote_request_format(request_bytes) {
        RemoteRequestFormat::V1 => {
            preflight_remote_request(request_bytes, context, advertisement, ceilings)
                .map(AdmittedRemoteRequestEnvelope::V1)
                .map_err(RemoteRejectionEnvelope::V1)
        }
        RemoteRequestFormat::V2 => {
            preflight_remote_request_v2(request_bytes, context, advertisement, ceilings)
                .map(AdmittedRemoteRequestEnvelope::V2)
                .map_err(RemoteRejectionEnvelope::V2)
        }
    }
}

/// Exact bindings needed to audit a server-produced reply before release.
pub enum RemoteReplyExpectation {
    /// V1 correlation and shape contract.
    V1 {
        /// Authenticated ceilings used while decoding the reply.
        limits: RemoteReplyDecodeLimits,
        /// Request nonce that the reply must echo exactly.
        nonce: String,
        /// Fingerprint of the prepared plan selected by the request.
        plan: QueryPlanFingerprint,
        /// Fingerprint of the complete V1 request envelope.
        request: RemoteRequestFingerprint,
    },
    /// V2 correlation, shape, and structured-diagnostic contract.
    V2 {
        /// Authenticated ceilings used while decoding the V2 reply.
        limits: RemoteReplyDecodeLimitsV2,
        /// Fingerprint of the prepared plan selected by the request.
        plan: QueryPlanFingerprint,
        /// Fingerprint of the complete V2 request envelope.
        request: RemoteRequestFingerprintV2,
        /// Exact request retained for V2 correlation and result-shape checks.
        request_envelope: RemoteQueryRequestV2,
    },
}

/// Audited reply classification used by the transport policy hook.
pub struct AuditedRemoteReply {
    success: bool,
    code: String,
}

impl AuditedRemoteReply {
    /// Whether the reply is a successful response.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.success
    }

    /// Stable diagnostic code, or `ok` for a success.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl RemoteReplyExpectation {
    /// Authenticate and validate a reply in the exact request-selected format.
    pub fn audit(
        &self,
        bytes: &[u8],
        advertisement: &RemoteCapabilitiesFingerprint,
        trusted_key: RemoteSigningPublicKey,
    ) -> Result<AuditedRemoteReply, Diagnostic> {
        match self {
            Self::V1 {
                limits,
                nonce,
                plan,
                request,
            } => match decode_remote_reply(
                bytes,
                nonce,
                plan,
                request,
                advertisement,
                trusted_key,
                *limits,
                &Ed25519RemoteReplyVerifier,
            )? {
                RemoteReply::Response(_) => Ok(AuditedRemoteReply {
                    success: true,
                    code: "ok".to_owned(),
                }),
                RemoteReply::Failure(failure) => {
                    let diagnostic = failure.diagnostic()?;
                    Ok(AuditedRemoteReply {
                        success: false,
                        code: diagnostic.code().as_str().to_owned(),
                    })
                }
            },
            Self::V2 {
                limits,
                plan,
                request,
                request_envelope,
            } => match decode_remote_reply_v2(
                bytes,
                request_envelope,
                plan,
                request,
                advertisement,
                trusted_key,
                *limits,
                &Ed25519RemoteReplyVerifier,
            )? {
                RemoteReplyV2::Response(_) => Ok(AuditedRemoteReply {
                    success: true,
                    code: "ok".to_owned(),
                }),
                RemoteReplyV2::Failure(failure) => {
                    let diagnostic = failure.diagnostic()?;
                    Ok(AuditedRemoteReply {
                        success: false,
                        code: diagnostic.code().as_str().to_owned(),
                    })
                }
            },
        }
    }

    /// Encode an internal transport failure bound to this exact request.
    #[must_use]
    pub fn bound_internal_failure(
        &self,
        diagnostic: &Diagnostic,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &RemoteReplySigningKey,
    ) -> Vec<u8> {
        match self {
            Self::V1 { nonce, request, .. } => {
                RemoteQueryFailure::bound(nonce.clone(), request, diagnostic)
                    .encode_signed_or_fallback(advertisement, signer)
            }
            Self::V2 {
                request,
                request_envelope,
                ..
            } => encode_bound_remote_failure_v2(
                request_envelope.nonce(),
                request,
                diagnostic,
                advertisement,
                signer,
            ),
        }
    }
}

/// Execute one admitted request over a live transaction.
///
/// Always returns envelope bytes: a typed response on success, a
/// structured failure carrying the admitted nonce otherwise.
pub async fn execute_admitted_remote_request(
    admitted: AdmittedRemoteRequest,
    transaction: &mut crate::session::transaction::Transaction,
    signer: &RemoteReplySigningKey,
) -> Vec<u8> {
    let nonce = admitted.nonce.clone();
    let request_fingerprint = admitted.request_fingerprint.clone();
    let advertisement_fingerprint = admitted.advertisement_fingerprint.clone();
    match run_admitted_request(admitted, transaction, signer).await {
        Ok(response) => response,
        Err((_, diagnostic)) => RemoteQueryFailure::bound(nonce, &request_fingerprint, &diagnostic)
            .encode_signed_or_fallback(&advertisement_fingerprint, signer),
    }
}

async fn run_admitted_request(
    admitted: AdmittedRemoteRequest,
    transaction: &mut crate::session::transaction::Transaction,
    signer: &RemoteReplySigningKey,
) -> Result<Vec<u8>, (Option<String>, Diagnostic)> {
    let AdmittedRemoteRequest {
        advertisement_fingerprint,
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint,
        provider_byte_budget,
        replay_until: _,
        request_fingerprint,
        validated,
    } = admitted;
    let fail = |diagnostic: Diagnostic| (Some(nonce.clone()), diagnostic);

    let mut wire_meter = if invocation.operation() == QueryOperation::Rows {
        let empty = match validated.output_schema() {
            OutputSchema::Rows(_) => RemoteOutcome::Rows { rows: Vec::new() },
            OutputSchema::Documents(_) => RemoteOutcome::Documents {
                documents: Vec::new(),
            },
        };
        Some(
            RemoteResponseWireMeter::new(
                &nonce,
                &plan_fingerprint,
                &request_fingerprint,
                &advertisement_fingerprint,
                signer.public_key(),
                empty,
                byte_budget,
            )
            .map_err(&fail)?,
        )
    } else {
        None
    };

    let provider_transaction = transaction.provider_mut().map_err(|_| {
        fail(failure(
            DiagnosticCategory::Integrity,
            "query_remote_provider_unavailable",
            "the executor transaction has no live provider",
        ))
    })?;
    let mut provider = crate::migration_assertion::TransactionAssertionProvider {
        transaction: provider_transaction,
    };
    let mut execution_limits = limits;
    // Remote rows/documents are charged against their exact authenticated wire
    // representation by `wire_meter`; raw provider JSON is not that contract.
    execution_limits.answer.max_bytes = provider_byte_budget;
    let outcome = execute_with_provider_observer(
        &mut provider,
        &validated,
        &invocation,
        execution_limits,
        wire_meter
            .as_mut()
            .map(|meter| meter as &mut dyn QueryV2ValidatedItemObserver),
    )
    .await
    .map_err(|error| {
        fail(match error {
            crate::query_v2::QueryV2ExecutionError::Validation(diagnostic) => diagnostic,
            crate::query_v2::QueryV2ExecutionError::Provider(error) => {
                crate::query_v2::provider_diagnostic(
                    &error,
                    "query_remote_provider_failed",
                    "the executor provider call failed",
                )
            }
        })
    })?;

    let response = RemoteQueryResponse::new(
        nonce.clone(),
        &plan_fingerprint,
        &request_fingerprint,
        remote_outcome(&outcome),
    )
    .map_err(&fail)?;
    let bytes = response
        .encode_signed(&advertisement_fingerprint, signer)
        .map_err(&fail)?;
    if wire_meter
        .as_ref()
        .is_some_and(|meter| meter.encoded_bytes != u64::try_from(bytes.len()).unwrap_or(u64::MAX))
    {
        return Err(fail(failure(
            DiagnosticCategory::Integrity,
            "query_remote_response_meter_mismatch",
            "the authenticated response length differs from its streaming meter",
        )));
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > byte_budget {
        return Err(fail(failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_response_oversized",
            "the typed response exceeds the effective byte budget",
        )));
    }
    Ok(bytes)
}

/// Execute one admitted additive V2 request over exactly one live transaction.
pub async fn execute_admitted_remote_request_v2(
    admitted: AdmittedRemoteRequestV2,
    transaction: &mut crate::session::transaction::Transaction,
    signer: &RemoteReplySigningKey,
) -> Vec<u8> {
    let nonce = admitted.nonce.clone();
    let request_fingerprint = admitted.request_fingerprint.clone();
    let advertisement_fingerprint = admitted.advertisement_fingerprint.clone();
    match run_admitted_request_v2(admitted, transaction, signer).await {
        Ok(response) => response,
        Err(diagnostic) => encode_bound_remote_failure_v2(
            &nonce,
            &request_fingerprint,
            &diagnostic,
            &advertisement_fingerprint,
            signer,
        ),
    }
}

/// Execute one format-dispatched admitted request without downgrade.
pub async fn execute_admitted_remote_request_versioned(
    admitted: AdmittedRemoteRequestEnvelope,
    transaction: &mut crate::session::transaction::Transaction,
    signer: &RemoteReplySigningKey,
) -> Vec<u8> {
    match admitted {
        AdmittedRemoteRequestEnvelope::V1(admitted) => {
            execute_admitted_remote_request(admitted, transaction, signer).await
        }
        AdmittedRemoteRequestEnvelope::V2(admitted) => {
            execute_admitted_remote_request_v2(admitted, transaction, signer).await
        }
    }
}

async fn run_admitted_request_v2(
    admitted: AdmittedRemoteRequestV2,
    transaction: &mut crate::session::transaction::Transaction,
    signer: &RemoteReplySigningKey,
) -> Result<Vec<u8>, Diagnostic> {
    let AdmittedRemoteRequestV2 {
        advertisement_fingerprint,
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint: _,
        provider_byte_budget,
        replay_until: _,
        reply_limits,
        request_envelope: _,
        request_fingerprint,
        result,
        validated,
    } = admitted;
    let plan = validated.plan().clone();

    let empty = match result {
        RemoteResultKindV2::Rows => Some(RemoteOutcomeV2::Rows { rows: Vec::new() }),
        RemoteResultKindV2::Documents => Some(RemoteOutcomeV2::Documents {
            documents: Vec::new(),
        }),
        RemoteResultKindV2::Count
        | RemoteResultKindV2::Exists
        | RemoteResultKindV2::HydratedRows
        | RemoteResultKindV2::HydratedPage
        | RemoteResultKindV2::DistinctCount
        | RemoteResultKindV2::DistinctExists => None,
    };
    let mut wire_meter = empty
        .map(|outcome| {
            RemoteResponseWireMeterV2::new(
                &nonce,
                &plan,
                &request_fingerprint,
                &advertisement_fingerprint,
                signer,
                result,
                outcome,
                byte_budget,
            )
        })
        .transpose()?;

    let mut execution_limits = limits;
    // Provider answer bytes and authenticated response bytes are different
    // representations. Keep provider work under the server ceiling while the
    // exact signed response remains under the caller-bound `byte_budget`.
    execution_limits.answer.max_bytes = provider_byte_budget;
    let execution_diagnostic = |error| match error {
        crate::query_v2::QueryV2ExecutionError::Validation(diagnostic) => diagnostic,
        crate::query_v2::QueryV2ExecutionError::Provider(error) => {
            crate::query_v2::provider_diagnostic(
                &error,
                "query_remote_v2_provider_failed",
                "the V2 executor provider call failed",
            )
        }
    };
    let model_result = matches!(
        result,
        RemoteResultKindV2::HydratedRows
            | RemoteResultKindV2::HydratedPage
            | RemoteResultKindV2::DistinctCount
            | RemoteResultKindV2::DistinctExists
    );
    let outcome = if model_result {
        crate::query_v2::execute_validated_model_query(
            transaction,
            &validated,
            &invocation,
            execution_limits,
            reply_limits,
        )
        .await
        .map_err(execution_diagnostic)?
    } else {
        let provider_transaction = transaction.provider_mut().map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_provider_unavailable",
                "the V2 executor transaction has no live provider",
            )
        })?;
        let mut provider = crate::migration_assertion::TransactionAssertionProvider {
            transaction: provider_transaction,
        };
        let outcome = execute_with_provider_observer(
            &mut provider,
            &validated,
            &invocation,
            execution_limits,
            wire_meter
                .as_mut()
                .map(|meter| meter as &mut dyn QueryV2ValidatedItemObserver),
        )
        .await
        .map_err(execution_diagnostic)?;
        remote_outcome_v2(&outcome)
    };

    // The compatibility executor already uses this contract-owned gate while
    // constructing evidence. Re-run it at the remote trust boundary so no
    // future executor path can sign an outcome the caller's exact request
    // budgets would reject.
    validate_remote_outcome_v2(&outcome, result, reply_limits, &plan)?;
    let response = RemoteQueryResponseV2::new(nonce, &plan, &request_fingerprint, result, outcome)?;
    let signed_len = u64::try_from(
        response.signed_encoded_len(&advertisement_fingerprint, signer.public_key())?,
    )
    .map_err(|_| response_oversized_v2())?;
    if signed_len > byte_budget {
        return Err(response_oversized_v2());
    }
    let bytes = response.encode_signed(&advertisement_fingerprint, signer)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != signed_len {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_response_length_mismatch",
            "authenticated V2 response length changed between preflight and signing",
        ));
    }
    if wire_meter
        .as_ref()
        .is_some_and(|meter| meter.encoded_bytes != u64::try_from(bytes.len()).unwrap_or(u64::MAX))
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_response_meter_mismatch",
            "authenticated V2 response length differs from its streaming meter",
        ));
    }
    Ok(bytes)
}

/// Tighten caller budgets under executor ceilings; never raise them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteExecutionShape {
    Rows,
    Documents,
    Count,
    Exists,
}

/// Return the exact signed-wire floor for any successful response of this
/// execution shape. This is deliberately independent of the provider: a
/// request whose budget cannot carry even the smallest valid success envelope
/// is rejected during preflight, before replay admission or host construction.
fn minimum_signed_success_response_len(
    nonce: &str,
    plan: &QueryPlanFingerprint,
    request: &RemoteRequestFingerprint,
    advertisement: &RemoteCapabilitiesFingerprint,
    key: RemoteSigningPublicKey,
    shape: RemoteExecutionShape,
    max_items: u64,
) -> Result<u64, Diagnostic> {
    let outcome = match shape {
        RemoteExecutionShape::Rows => RemoteOutcome::Rows { rows: Vec::new() },
        RemoteExecutionShape::Documents => RemoteOutcome::Documents {
            documents: Vec::new(),
        },
        RemoteExecutionShape::Count => RemoteOutcome::Count { value: 0 },
        // `true` is the shorter canonical spelling. A zero-item limit cannot
        // admit a positive existence result, so only `false` is valid there.
        RemoteExecutionShape::Exists if max_items == 0 => RemoteOutcome::Exists { value: false },
        RemoteExecutionShape::Exists => RemoteOutcome::Exists { value: true },
    };
    let response = RemoteQueryResponse::new(nonce, plan, request, outcome)?;
    u64::try_from(response.signed_encoded_len(advertisement, key)?)
        .map_err(|_| response_oversized())
}

fn minimum_signed_success_response_len_v2(
    nonce: &str,
    plan: &type_bridge_contract::query_plan::QueryPlan,
    request: &RemoteRequestFingerprintV2,
    advertisement: &RemoteCapabilitiesFingerprint,
    key: RemoteSigningPublicKey,
    result: RemoteResultKindV2,
    max_items: u64,
) -> Result<u64, Diagnostic> {
    let outcome = match result {
        RemoteResultKindV2::Rows => RemoteOutcomeV2::Rows { rows: Vec::new() },
        RemoteResultKindV2::Documents => RemoteOutcomeV2::Documents {
            documents: Vec::new(),
        },
        RemoteResultKindV2::Count => RemoteOutcomeV2::Count { value: 0 },
        RemoteResultKindV2::Exists if max_items == 0 => RemoteOutcomeV2::Exists { value: false },
        RemoteResultKindV2::Exists => RemoteOutcomeV2::Exists { value: true },
        RemoteResultKindV2::HydratedRows
        | RemoteResultKindV2::HydratedPage
        | RemoteResultKindV2::DistinctCount
        | RemoteResultKindV2::DistinctExists => {
            return u64::try_from(RemoteQueryResponseV2::signed_framing_floor(
                nonce,
                plan,
                request,
                result,
                advertisement,
                key,
                max_items,
            )?)
            .map_err(|_| response_oversized_v2());
        }
    };
    let response = RemoteQueryResponseV2::new(nonce, plan, request, result, outcome)?;
    u64::try_from(response.signed_encoded_len(advertisement, key)?)
        .map_err(|_| response_oversized_v2())
}

fn tighten_limits(
    caller: RemoteLimits,
    ceilings: QueryV2AnswerLimits,
    request_expiry: Instant,
    shape: RemoteExecutionShape,
) -> QueryV2AnswerLimits {
    let max_items = ceilings.answer.max_items.min(caller.max_items);
    let max_items = if matches!(
        shape,
        RemoteExecutionShape::Rows | RemoteExecutionShape::Documents
    ) {
        max_items.min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX))
    } else {
        max_items
    };
    let max_collection_members = ceilings
        .max_collection_members
        .min(caller.max_collection_members);
    let max_collection_members = if shape == RemoteExecutionShape::Documents {
        max_collection_members.min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX))
    } else {
        max_collection_members
    };
    QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items,
            max_bytes: ceilings
                .answer
                .max_bytes
                .min(caller.max_bytes)
                .min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX)),
            deadline: Some(
                ceilings
                    .answer
                    .deadline
                    .map_or(request_expiry, |ceiling| ceiling.min(request_expiry)),
            ),
            cancellation: ceilings.answer.cancellation,
        },
        max_collection_members,
    }
}

fn tighten_limits_v2(
    caller: RemoteLimitsV2,
    ceilings: QueryV2AnswerLimits,
    request_expiry: Instant,
    shape: RemoteExecutionShape,
) -> (QueryV2AnswerLimits, RemoteReplyDecodeLimitsV2) {
    let hard_collection = u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX);
    let max_items = ceilings.answer.max_items.min(caller.max_items);
    let max_items = if matches!(
        shape,
        RemoteExecutionShape::Rows | RemoteExecutionShape::Documents
    ) {
        max_items.min(hard_collection)
    } else {
        max_items
    };
    let max_collection_members = ceilings
        .max_collection_members
        .min(caller.max_collection_members);
    let max_collection_members = if shape == RemoteExecutionShape::Documents {
        max_collection_members.min(hard_collection)
    } else {
        max_collection_members
    };
    let max_bytes = ceilings
        .answer
        .max_bytes
        .min(caller.max_bytes)
        .min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX));
    let limits = QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items,
            max_bytes,
            deadline: Some(
                ceilings
                    .answer
                    .deadline
                    .map_or(request_expiry, |ceiling| ceiling.min(request_expiry)),
            ),
            cancellation: ceilings.answer.cancellation,
        },
        max_collection_members,
    };
    let reply = RemoteReplyDecodeLimitsV2 {
        max_bytes,
        max_items,
        max_collection_members,
        max_graph_nodes: caller.max_graph_nodes.min(hard_collection),
        max_attribute_values: caller.max_attribute_values.min(hard_collection),
        max_role_players: caller.max_role_players.min(hard_collection),
    };
    (limits, reply)
}

fn unix_time_ms() -> Result<u64, Diagnostic> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_clock_invalid",
            "system clock cannot establish an absolute remote request time",
        )
    })?;
    u64::try_from(duration.as_millis()).map_err(|_| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_clock_invalid",
            "system clock exceeds the supported remote timestamp range",
        )
    })
}

pub(crate) fn remote_outcome(outcome: &QueryV2Outcome) -> RemoteOutcome {
    match outcome {
        QueryV2Outcome::Rows(rows) => RemoteOutcome::Rows {
            rows: rows
                .iter()
                .map(|row| row.values().iter().map(remote_value).collect())
                .collect(),
        },
        QueryV2Outcome::Documents(documents) => RemoteOutcome::Documents {
            documents: documents
                .iter()
                .map(|document| document.values().iter().map(remote_field_value).collect())
                .collect(),
        },
        QueryV2Outcome::Count(value) => RemoteOutcome::Count { value: *value },
        QueryV2Outcome::Exists(value) => RemoteOutcome::Exists { value: *value },
    }
}

pub(crate) fn remote_outcome_v2(outcome: &QueryV2Outcome) -> RemoteOutcomeV2 {
    match outcome {
        QueryV2Outcome::Rows(rows) => RemoteOutcomeV2::Rows {
            rows: rows
                .iter()
                .map(|row| row.values().iter().map(remote_value).collect())
                .collect(),
        },
        QueryV2Outcome::Documents(documents) => RemoteOutcomeV2::Documents {
            documents: documents
                .iter()
                .map(|document| document.values().iter().map(remote_field_value).collect())
                .collect(),
        },
        QueryV2Outcome::Count(value) => RemoteOutcomeV2::Count { value: *value },
        QueryV2Outcome::Exists(value) => RemoteOutcomeV2::Exists { value: *value },
    }
}

fn remote_row(row: &QueryResultRow) -> Vec<RemoteValue> {
    row.values().iter().map(remote_value).collect()
}

fn remote_document(document: &QueryResultDocument) -> Vec<RemoteFieldValue> {
    document.values().iter().map(remote_field_value).collect()
}

fn remote_value(value: &QueryRowValue) -> RemoteValue {
    match value {
        QueryRowValue::Thing { type_id, iid } => RemoteValue::Thing {
            iid: iid.clone(),
            type_id: type_id.clone(),
        },
        QueryRowValue::Attribute { type_id, value } => RemoteValue::Attribute {
            type_id: type_id.clone(),
            value: value.clone(),
        },
        QueryRowValue::Value { value } => RemoteValue::Value {
            value: value.clone(),
        },
        QueryRowValue::Absent => RemoteValue::Absent,
    }
}

fn remote_field_value(value: &DocumentFieldValue) -> RemoteFieldValue {
    match value {
        DocumentFieldValue::Scalar(value) => RemoteFieldValue::Scalar {
            value: value.clone(),
        },
        DocumentFieldValue::Absent => RemoteFieldValue::Absent,
        DocumentFieldValue::List(values) => RemoteFieldValue::List {
            values: values.clone(),
        },
    }
}

fn outcome_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "query_remote_outcome_mismatch",
        "response outcome kind does not match the invoked operation",
    )
}

fn evidence_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "query_remote_evidence_mismatch",
        "response evidence does not conform to the validated output schema",
    )
}

fn response_oversized() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_response_oversized",
        "the typed response exceeds the effective byte budget",
    )
}

fn response_oversized_v2() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_response_oversized",
        "typed V2 response exceeds the effective byte budget",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::backend::AnswerCancellation;

    fn unbounded_caller() -> RemoteLimits {
        RemoteLimits {
            deadline_ms: None,
            max_bytes: u64::MAX,
            max_items: u64::MAX,
            max_collection_members: u64::MAX,
        }
    }

    fn unbounded_ceiling() -> QueryV2AnswerLimits {
        QueryV2AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: u64::MAX,
                max_bytes: u64::MAX,
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
            max_collection_members: u64::MAX,
        }
    }

    #[test]
    fn remote_limit_tightening_is_monotonic_and_shape_aware() {
        let collection = u64::try_from(MAX_CANONICAL_COLLECTION_LEN).expect("collection ceiling");
        let bytes = u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling");
        let expiry = Instant::now() + Duration::from_secs(1);

        let rows = tighten_limits(
            unbounded_caller(),
            unbounded_ceiling(),
            expiry,
            RemoteExecutionShape::Rows,
        );
        assert_eq!(rows.answer.max_items, collection);
        assert_eq!(rows.answer.max_bytes, bytes);
        assert_eq!(rows.max_collection_members, u64::MAX);

        let documents = tighten_limits(
            unbounded_caller(),
            unbounded_ceiling(),
            expiry,
            RemoteExecutionShape::Documents,
        );
        assert_eq!(documents.answer.max_items, collection);
        assert_eq!(documents.max_collection_members, collection);

        let count = tighten_limits(
            unbounded_caller(),
            unbounded_ceiling(),
            expiry,
            RemoteExecutionShape::Count,
        );
        assert_eq!(count.answer.max_items, u64::MAX);
        assert_eq!(count.max_collection_members, u64::MAX);

        let mut caller = unbounded_caller();
        caller.max_bytes = 17;
        caller.max_items = 11;
        caller.max_collection_members = 13;
        let tightened = tighten_limits(
            caller,
            unbounded_ceiling(),
            expiry,
            RemoteExecutionShape::Documents,
        );
        assert_eq!(tightened.answer.max_bytes, 17);
        assert_eq!(tightened.answer.max_items, 11);
        assert_eq!(tightened.max_collection_members, 13);
    }

    #[test]
    fn remote_v2_limit_tightening_never_widens_transport_or_graph_budgets() {
        let hard_collection =
            u64::try_from(MAX_CANONICAL_COLLECTION_LEN).expect("collection ceiling");
        let hard_bytes = u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling");
        let now = Instant::now();
        let ceiling_deadline = now + Duration::from_millis(250);
        let request_expiry = now + Duration::from_secs(1);
        let ceilings = QueryV2AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: 23,
                max_bytes: 29,
                deadline: Some(ceiling_deadline),
                cancellation: AnswerCancellation::default(),
            },
            max_collection_members: 31,
        };
        let caller = RemoteLimitsV2 {
            deadline_ms: Some(1_000),
            max_bytes: 19,
            max_items: 17,
            max_collection_members: 13,
            max_graph_nodes: 11,
            max_attribute_values: 7,
            max_role_players: 5,
        };

        let (execution, reply) = tighten_limits_v2(
            caller,
            ceilings,
            request_expiry,
            RemoteExecutionShape::Documents,
        );
        assert_eq!(execution.answer.max_bytes, 19);
        assert_eq!(execution.answer.max_items, 17);
        assert_eq!(execution.max_collection_members, 13);
        assert_eq!(execution.answer.deadline, Some(ceiling_deadline));
        assert_eq!(reply.max_bytes, 19);
        assert_eq!(reply.max_items, 17);
        assert_eq!(reply.max_collection_members, 13);
        assert_eq!(reply.max_graph_nodes, 11);
        assert_eq!(reply.max_attribute_values, 7);
        assert_eq!(reply.max_role_players, 5);

        let unbounded = RemoteLimitsV2 {
            deadline_ms: None,
            max_bytes: u64::MAX,
            max_items: u64::MAX,
            max_collection_members: u64::MAX,
            max_graph_nodes: u64::MAX,
            max_attribute_values: u64::MAX,
            max_role_players: u64::MAX,
        };
        let (execution, reply) = tighten_limits_v2(
            unbounded,
            unbounded_ceiling(),
            request_expiry,
            RemoteExecutionShape::Rows,
        );
        assert_eq!(execution.answer.max_bytes, hard_bytes);
        assert_eq!(execution.answer.max_items, hard_collection);
        assert_eq!(reply.max_bytes, hard_bytes);
        assert_eq!(reply.max_items, hard_collection);
        assert_eq!(reply.max_graph_nodes, hard_collection);
        assert_eq!(reply.max_attribute_values, hard_collection);
        assert_eq!(reply.max_role_players, hard_collection);
    }

    #[test]
    fn wall_clock_remainder_stays_anchored_to_the_paired_monotonic_sample() {
        let observed_now = Instant::now();
        let monotonic_anchor = observed_now
            .checked_sub(Duration::from_millis(50))
            .expect("test monotonic anchor");
        let clock = PreflightClockSample {
            monotonic_anchor,
            now_unix_ms: 1_800_000_000_000,
        };

        let deadline = clock.deadline_after(10).expect("anchored deadline");
        assert!(
            deadline <= observed_now,
            "remaining wall-clock lifetime must not be rebased at the end of preflight",
        );
    }
}
