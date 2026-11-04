//! Additive V2 remote-query envelopes and hydrated model evidence.
//!
//! This module deliberately does not widen any V1 DTO. V2 requests, replies,
//! failures, fingerprints, and hydration graphs have distinct Rust types and
//! exact format discriminators. The authenticated outer reply and executor
//! advertisement remain the existing open V1 containers.

use crate::query_model_evidence::{
    HydratedRow, HydrationGraph, HydrationNode, HydrationNodeId, HydrationReference, HydrationSlot,
    remote_evidence_mismatch,
};
use crate::query_model_evidence::{
    ModelOutcome, ModelReducedValue, ModelReductionGroup, ModelReductionRow,
};
use crate::query_model_limits::{
    DEFAULT_MODEL_MAX_STATEMENTS, ModelReplyDecodeLimits, RemoteModelLimits,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticDetailValue};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDigest, FingerprintDomain,
};
use crate::id::{AttributeId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::limits::{
    CodecLimits, MAX_DIAGNOSTIC_BYTES, MAX_REMOTE_ENVELOPE_BYTES, MAX_SELECTED_SLOTS, QueryLimits,
    REMOTE_ENVELOPE_CODEC_LIMITS,
};
use crate::migration_assertion::BindingId;
use crate::query_plan::{
    InputRow, ModelReductionKey, ModelReductionTerm, ModelValue, QueryInvocation, QueryOperation,
    QueryOutput, QueryPlan, QueryPlanFingerprint, decode_query_plan_with_limits,
};
use crate::query_remote::{
    DEFAULT_REMOTE_DEADLINE_MS, MAX_REMOTE_CLOCK_SKEW_MS, MAX_REMOTE_DEADLINE_MS,
    QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN, RemoteCapabilities, RemoteCapabilitiesFingerprint,
    RemoteFieldValue, RemoteReplySigner, RemoteReplyVerifier, RemoteSigningPublicKey, RemoteValue,
};
use crate::query_remote_diagnostic::{
    RemoteDiagnostic, RemoteDiagnosticDetail, RemoteDiagnosticPathSegment,
    validate_remote_diagnostic_wire,
};
use crate::value::CanonicalValue;

/// The exact wire discriminator for additive V2 remote requests.
pub const QUERY_REMOTE_REQUEST_FORMAT_V2: &str = "typebridge.query-remote-request/v2";
/// The exact wire discriminator for additive V2 remote responses.
pub const QUERY_REMOTE_RESPONSE_FORMAT_V2: &str = "typebridge.query-remote-response/v2";
/// The exact wire discriminator for additive V2 remote failures.
pub const QUERY_REMOTE_FAILURE_FORMAT_V2: &str = "typebridge.query-remote-failure/v2";
/// The distinct canonicalization identifier for V2 request fingerprints.
pub const QUERY_REMOTE_REQUEST_CANONICALIZATION_V2: &str = "typebridge.query-remote-request/v2";

/// V2-plan admission capability.
pub const CAP_QUERY_PLAN_V2: &str = "query.plan.v2";
/// V2 remote-envelope admission capability.
pub const CAP_QUERY_REMOTE_ENVELOPE_V2: &str = "query.remote.envelope-v2";
/// Complete structured remote-diagnostic admission capability.
pub const CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC: &str = "query.remote.structured-diagnostic";
/// Hydrated model-output admission capability.
pub const CAP_QUERY_OUTPUT_HYDRATED: &str = "query.output.hydrated";
/// Same-snapshot hydration admission capability.
pub const CAP_QUERY_SAME_SNAPSHOT_HYDRATION: &str = "query.execution.same-snapshot-hydration";
/// Admission capability for an explicitly tightened remote statement budget.
pub const CAP_QUERY_REMOTE_STATEMENT_LIMIT: &str = "query.execution.statement-limit";

const NONCE_MIN_BYTES: usize = 16;
const NONCE_MAX_BYTES: usize = 128;
const FAILURE_FRAMING_ALLOWANCE_BYTES: usize = 1_024;

/// Return the exact base capabilities required by every V2 remote envelope.
#[must_use]
pub fn query_remote_v2_required_capabilities(carries_hydration_graph: bool) -> CapabilitySet {
    let mut capabilities = [
        CAP_QUERY_PLAN_V2,
        CAP_QUERY_REMOTE_ENVELOPE_V2,
        CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC,
    ]
    .into_iter()
    .map(static_capability)
    .collect::<CapabilitySet>();
    if carries_hydration_graph {
        capabilities.insert(static_capability(CAP_QUERY_OUTPUT_HYDRATED));
        capabilities.insert(static_capability(CAP_QUERY_SAME_SNAPSHOT_HYDRATION));
    }
    capabilities
}

fn static_capability(value: &'static str) -> CapabilityId {
    CapabilityId::new(value).expect("static V2 remote capability is canonical")
}

/// The response family bound into a V2 request fingerprint.
///
/// The complete model output, hydration projection, ordering, cardinality,
/// page, and root contract lives in the embedded V2 plan. This discriminator
/// prevents a valid plan/request from being replayed under another terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteResultKindV2 {
    /// Low-level positional rows.
    Rows,
    /// Low-level fetched documents.
    Documents,
    /// Low-level scalar count.
    Count,
    /// Low-level scalar existence.
    Exists,
    /// Model-oriented hydrated rows, including the exactly-one contract.
    HydratedRows,
    /// Model-oriented distinct-root page.
    HydratedPage,
    /// Model-oriented distinct-root count.
    DistinctCount,
    /// Model-oriented distinct-root existence.
    DistinctExists,
    /// Model-oriented typed reduction rows.
    ModelReduction,
}

impl RemoteResultKindV2 {
    const fn operation(self) -> QueryOperation {
        match self {
            Self::Rows
            | Self::Documents
            | Self::HydratedRows
            | Self::HydratedPage
            | Self::ModelReduction => QueryOperation::Rows,
            Self::Count | Self::DistinctCount => QueryOperation::Count,
            Self::Exists | Self::DistinctExists => QueryOperation::Exists,
        }
    }

    const fn compatibility_terminal(self) -> bool {
        matches!(
            self,
            Self::HydratedRows
                | Self::HydratedPage
                | Self::DistinctCount
                | Self::DistinctExists
                | Self::ModelReduction
        )
    }

    const fn carries_hydration_graph(self) -> bool {
        matches!(
            self,
            Self::HydratedRows | Self::HydratedPage | Self::ModelReduction
        )
    }
}

/// One complete V2 remote invocation of a reusable V2 plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryRequestV2 {
    advertisement: String,
    expires_at_unix_ms: u64,
    format: String,
    limits: RemoteModelLimits,
    nonce: String,
    plan: serde_json::Value,
    prepared_at_unix_ms: u64,
    // The limits this request was prepared or decoded under; never wire
    // state, because each peer applies its own.
    #[serde(skip)]
    query_limits: QueryLimits,
    // The preparing caller's own limits, kept only to say whether a refusal
    // came from them or from the advertisement.
    #[serde(skip)]
    configured_limits: QueryLimits,
    result: RemoteResultKindV2,
    rows: Vec<Vec<Option<CanonicalValue>>>,
}

impl RemoteQueryRequestV2 {
    /// Bind one V2 plan invocation, result contract, advertisement, and budget.
    ///
    /// `result` is checked against both the invocation operation and the
    /// embedded plan. Model-oriented result kinds additionally require the
    /// model-query contract carried by the V2 plan.
    pub fn new(
        plan: &QueryPlan,
        invocation: &QueryInvocation,
        result: RemoteResultKindV2,
        advertisement: &RemoteCapabilities,
        limits: RemoteModelLimits,
        nonce: impl Into<String>,
        prepared_at_unix_ms: u64,
    ) -> Result<Self, Diagnostic> {
        validate_v2_plan_and_result(plan, result)?;
        if !invocation.binds(plan)? || invocation.operation() != result.operation() {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_invocation_plan_mismatch",
                "V2 invocation, result contract, and plan do not bind one another",
            ));
        }
        let mut required = plan.required_capabilities().clone();
        for capability in invocation.transport_capabilities() {
            required.insert(capability);
        }
        for capability in query_remote_v2_required_capabilities(result.carries_hydration_graph()) {
            required.insert(capability);
        }
        if limits.max_statements != DEFAULT_MODEL_MAX_STATEMENTS {
            required.insert(static_capability(CAP_QUERY_REMOTE_STATEMENT_LIMIT));
        }
        required.ensure_supported_by(advertisement.capabilities())?;

        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_remote_limits_v2(limits)?;
        let expires_at_unix_ms = prepared_at_unix_ms
            .checked_add(limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS))
            .ok_or_else(remote_time_invalid_v2)?;
        validate_remote_time_shape_v2(prepared_at_unix_ms, expires_at_unix_ms, limits)?;
        let configured_limits = plan.limits();
        let query_limits = crate::query_remote::negotiate_query_limits(plan, advertisement)?;
        let plan = serde_json::from_slice::<serde_json::Value>(&plan.canonical_bytes()?).map_err(
            |_| {
                remote_v2_failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_v2_plan_unencodable",
                    "the V2 plan cannot be embedded as canonical JSON",
                )
            },
        )?;
        Ok(Self {
            advertisement: advertisement.fingerprint()?.digest_hex(),
            expires_at_unix_ms,
            format: QUERY_REMOTE_REQUEST_FORMAT_V2.to_owned(),
            limits,
            nonce,
            plan,
            prepared_at_unix_ms,
            query_limits,
            configured_limits,
            result,
            rows: invocation
                .inputs()
                .iter()
                .map(|row| row.values().to_vec())
                .collect(),
        })
    }

    /// Encode exact canonical V2 request bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, self.query_limits.remote_request_codec()).map_err(
            |diagnostic| {
                self.query_limits.mark_advertised(
                    self.configured_limits,
                    crate::query_remote::annotate_request_envelope(self.query_limits, diagnostic),
                )
            },
        )
    }

    /// Decode one canonical V2 request, rejecting V1 and unknown versions
    /// before reconstructing the embedded plan.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        Self::decode_with_limits(bytes, QueryLimits::CANONICAL)
    }

    /// Decode one canonical V2 request under the receiver's own limits.
    ///
    /// The request and its embedded plan are bounded by `limits` alone;
    /// nothing the sender wrote can widen them.
    pub fn decode_with_limits(bytes: &[u8], limits: QueryLimits) -> Result<Self, Diagnostic> {
        require_format(
            bytes,
            QUERY_REMOTE_REQUEST_FORMAT_V2,
            "remote request",
            limits.remote_request_bytes,
        )
        .map_err(|diagnostic| limits.annotate(diagnostic))?;
        let mut request =
            from_canonical_json_with_limits::<Self>(bytes, limits.remote_request_codec()).map_err(
                |diagnostic| crate::query_remote::annotate_request_envelope(limits, diagnostic),
            )?;
        request.query_limits = limits;
        request.configured_limits = limits;
        check_nonce_v2(&request.nonce)?;
        validate_remote_limits_v2(request.limits)?;
        FingerprintDigest::from_hex(&request.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
        validate_remote_time_shape_v2(
            request.prepared_at_unix_ms,
            request.expires_at_unix_ms,
            request.limits,
        )?;
        let plan = request.plan()?;
        validate_v2_plan_and_result(&plan, request.result)?;
        request.invocation(&plan)?;
        if request.encode()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_request_wire_mismatch",
                "V2 request bytes normalize after trusted reconstruction",
            ));
        }
        Ok(request)
    }

    /// Rebuild the trusted V2 plan from the embedded canonical document.
    pub fn plan(&self) -> Result<QueryPlan, Diagnostic> {
        // The envelope is transport; the plan format is the artifact's own
        // version, and either released one travels in it.
        decode_query_plan_with_limits(
            &to_canonical_json_with_limits(&self.plan, self.query_limits.codec)?,
            self.query_limits,
        )
    }

    /// Rebuild the validated invocation against the carried plan.
    pub fn invocation(&self, plan: &QueryPlan) -> Result<QueryInvocation, Diagnostic> {
        QueryInvocation::new(
            plan,
            self.result.operation(),
            self.rows
                .iter()
                .map(|row| InputRow::new(row.clone()))
                .collect(),
        )
    }

    /// Compute the V2 request fingerprint over exact canonical bytes.
    pub fn fingerprint(&self) -> Result<RemoteRequestFingerprintV2, Diagnostic> {
        RemoteRequestFingerprintV2::compute(&self.encode()?)
    }

    /// Return the caller nonce echoed by a correlated reply.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Return the response family bound into this request.
    #[must_use]
    pub const fn result_kind(&self) -> RemoteResultKindV2 {
        self.result
    }

    /// Return the caller execution budgets.
    #[must_use]
    pub const fn limits(&self) -> RemoteModelLimits {
        self.limits
    }

    /// Return whether this request binds the executor's exact advertisement.
    pub fn binds_advertisement(
        &self,
        advertisement: &RemoteCapabilities,
    ) -> Result<bool, Diagnostic> {
        let expected = advertisement.fingerprint()?;
        let actual = FingerprintDigest::from_hex(&self.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
        Ok(actual == expected.as_fingerprint().digest())
    }

    /// Recheck exact advertisement identity and every plan, invocation, and
    /// envelope capability before an executor constructs provider resources.
    pub fn validate_advertisement(
        &self,
        advertisement: &RemoteCapabilities,
    ) -> Result<(), Diagnostic> {
        if !self.binds_advertisement(advertisement)? {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_advertisement_mismatch",
                "V2 request does not bind this executor advertisement",
            ));
        }
        let plan = self.plan()?;
        let invocation = self.invocation(&plan)?;
        let mut required = plan.required_capabilities().clone();
        for capability in invocation.transport_capabilities() {
            required.insert(capability);
        }
        for capability in
            query_remote_v2_required_capabilities(self.result.carries_hydration_graph())
        {
            required.insert(capability);
        }
        if self.limits.max_statements != DEFAULT_MODEL_MAX_STATEMENTS {
            required.insert(static_capability(CAP_QUERY_REMOTE_STATEMENT_LIMIT));
        }
        required.ensure_supported_by(advertisement.capabilities())
    }

    /// Validate the absolute request lifetime at one executor clock sample.
    pub fn remaining_lifetime_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        validate_remote_time_shape_v2(
            self.prepared_at_unix_ms,
            self.expires_at_unix_ms,
            self.limits,
        )?;
        if self.prepared_at_unix_ms > now_unix_ms.saturating_add(MAX_REMOTE_CLOCK_SKEW_MS) {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_time_future",
                "V2 request preparation time exceeds the allowed clock skew",
            ));
        }
        self.expires_at_unix_ms
            .checked_sub(now_unix_ms)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| {
                remote_v2_failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_v2_request_expired",
                    "V2 request absolute expiry has elapsed",
                )
            })
    }

    /// Return remaining execution time without granting positive clock skew.
    pub fn remaining_execution_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        let absolute_remaining = self.remaining_lifetime_ms(now_unix_ms)?;
        let declared_lifetime = self
            .expires_at_unix_ms
            .checked_sub(self.prepared_at_unix_ms)
            .ok_or_else(remote_time_invalid_v2)?;
        Ok(absolute_remaining.min(declared_lifetime))
    }

    /// Return the exclusive absolute expiry timestamp.
    #[must_use]
    pub const fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

/// The canonical fingerprint of one complete V2 request envelope.
///
/// The domain is shared with V1 while the canonicalization identifier is
/// distinct, so identical-looking versioned documents cannot share identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRequestFingerprintV2(Fingerprint);

impl RemoteRequestFingerprintV2 {
    /// Compute a V2 request fingerprint from exact request bytes.
    pub fn compute(request_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_REQUEST_CANONICALIZATION_V2)?,
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

/// One successful V2 remote execution bound to its request and plan.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryResponseV2 {
    format: String,
    nonce: String,
    outcome: ModelOutcome,
    plan: String,
    request: String,
    // The codec limits this reply is encoded and decoded under; never wire
    // state, because each peer derives them from the budgets it agreed to.
    #[serde(skip, default = "default_reply_codec_v2")]
    codec: CodecLimits,
}

const fn default_reply_codec_v2() -> CodecLimits {
    REMOTE_ENVELOPE_CODEC_LIMITS
}

// Two replies are equal when their wire content is.
impl PartialEq for RemoteQueryResponseV2 {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && self.nonce == other.nonce
            && self.outcome == other.outcome
            && self.plan == other.plan
            && self.request == other.request
    }
}

impl RemoteQueryResponseV2 {
    /// Bind one V2 outcome to the nonce, V2 plan, whole V2 request, and
    /// request-selected result family, under the default reply budgets.
    pub fn new(
        nonce: impl Into<String>,
        plan: &QueryPlan,
        request: &RemoteRequestFingerprintV2,
        expected_result: RemoteResultKindV2,
        outcome: ModelOutcome,
    ) -> Result<Self, Diagnostic> {
        let mut response = Self::new_with_limits(
            nonce,
            plan,
            request,
            expected_result,
            outcome,
            ModelReplyDecodeLimits::DEFAULT_CEILING,
        )?;
        response.codec = default_reply_codec_v2();
        Ok(response)
    }

    /// Bind one V2 outcome under the budgets the executor ran under: the
    /// stricter of the request's and its own. The outcome is checked against
    /// them and the reply is encoded under them.
    pub fn new_with_limits(
        nonce: impl Into<String>,
        plan: &QueryPlan,
        request: &RemoteRequestFingerprintV2,
        expected_result: RemoteResultKindV2,
        outcome: ModelOutcome,
        limits: ModelReplyDecodeLimits,
    ) -> Result<Self, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_v2_plan_and_result(plan, expected_result)?;
        validate_outcome_kind_v2(&outcome, expected_result)?;
        validate_outcome_evidence_v2(&outcome, expected_result, limits, Some(plan))?;
        Ok(Self {
            format: QUERY_REMOTE_RESPONSE_FORMAT_V2.to_owned(),
            nonce,
            outcome,
            plan: plan.fingerprint()?.as_fingerprint().digest().to_hex(),
            request: request.digest_hex(),
            codec: limits.codec(),
        })
    }

    /// Encode one authenticated outer reply using the unchanged signed-reply
    /// and capability-advertisement formats.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        crate::query_remote::encode_signed_reply_payload_bounded(
            &self.encode_payload()?,
            advertisement,
            signer,
            self.codec.max_bytes,
        )
    }

    /// Return the exact signed outer-envelope length without signing.
    pub fn signed_encoded_len(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
    ) -> Result<usize, Diagnostic> {
        Ok(crate::query_remote::signed_reply_payload_encoded_len(
            &self.encode_payload()?,
            advertisement,
            key,
        ))
    }

    /// Return a signed-envelope framing lower bound for one result family.
    ///
    /// Model outcomes may require provider-owned graph evidence before a fully
    /// valid response can be constructed. This method uses the shortest
    /// syntactic member of the selected family, while still validating the
    /// nonce, plan terminal, and correlation fingerprints. Every valid signed
    /// response for the same bindings is at least this large, so executors can
    /// reject an impossible caller byte budget before constructing host state.
    pub fn signed_framing_floor(
        nonce: impl Into<String>,
        plan: &QueryPlan,
        request: &RemoteRequestFingerprintV2,
        expected_result: RemoteResultKindV2,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
        max_items: u64,
    ) -> Result<usize, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_v2_plan_and_result(plan, expected_result)?;
        let root = BindingId::new(0)?;
        let outcome = match expected_result {
            RemoteResultKindV2::Rows => ModelOutcome::Rows { rows: Vec::new() },
            RemoteResultKindV2::Documents => ModelOutcome::Documents {
                documents: Vec::new(),
            },
            RemoteResultKindV2::Count => ModelOutcome::Count { value: 0 },
            RemoteResultKindV2::Exists if max_items == 0 => ModelOutcome::Exists { value: false },
            RemoteResultKindV2::Exists => ModelOutcome::Exists { value: true },
            RemoteResultKindV2::HydratedRows => ModelOutcome::HydratedRows {
                graph: HydrationGraph { nodes: Vec::new() },
                rows: Vec::new(),
            },
            RemoteResultKindV2::HydratedPage => ModelOutcome::HydratedPage {
                entries: Vec::new(),
                graph: HydrationGraph { nodes: Vec::new() },
                limit: 0,
                offset: 0,
                root,
                total: Some(0),
            },
            RemoteResultKindV2::DistinctCount => ModelOutcome::DistinctCount { root, value: 0 },
            RemoteResultKindV2::DistinctExists if max_items == 0 => {
                ModelOutcome::DistinctExists { root, value: false }
            }
            RemoteResultKindV2::DistinctExists => {
                ModelOutcome::DistinctExists { root, value: true }
            }
            RemoteResultKindV2::ModelReduction => {
                let (root, group, reducers) = plan
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.model_query())
                    .and_then(|model| match model {
                        crate::query_plan::ModelQueryV2::Reduction {
                            root,
                            group,
                            reducers,
                            ..
                        } => Some((*root, group.clone(), reducers.clone())),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        remote_evidence_mismatch(
                            "model reduction framing lacks its exact terminal contract",
                        )
                    })?;
                ModelOutcome::ModelReduction {
                    graph: HydrationGraph { nodes: Vec::new() },
                    root,
                    group,
                    reducers,
                    rows: Vec::new(),
                }
            }
        };
        let response = Self {
            format: QUERY_REMOTE_RESPONSE_FORMAT_V2.to_owned(),
            nonce,
            outcome,
            plan: plan.fingerprint()?.as_fingerprint().digest().to_hex(),
            request: request.digest_hex(),
            codec: default_reply_codec_v2(),
        };
        response.signed_encoded_len(advertisement, key)
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, self.codec)
    }

    fn decode_bound(
        bytes: &[u8],
        expected_result: RemoteResultKindV2,
        limits: ModelReplyDecodeLimits,
        plan: Option<&QueryPlan>,
    ) -> Result<Self, Diagnostic> {
        let codec = limits.codec();
        require_format(
            bytes,
            QUERY_REMOTE_RESPONSE_FORMAT_V2,
            "remote response",
            codec.max_bytes,
        )?;
        let output_width = match expected_result {
            RemoteResultKindV2::Rows | RemoteResultKindV2::Documents => {
                plan.and_then(low_level_output_width)
            }
            RemoteResultKindV2::HydratedRows | RemoteResultKindV2::HydratedPage => {
                model_output_width(plan, expected_result)
            }
            RemoteResultKindV2::Count
            | RemoteResultKindV2::Exists
            | RemoteResultKindV2::DistinctCount
            | RemoteResultKindV2::DistinctExists
            | RemoteResultKindV2::ModelReduction => None,
        };
        preflight_response_evidence_v2(bytes, expected_result, output_width, limits)?;
        let mut response = from_canonical_json_with_limits::<Self>(bytes, codec)?;
        response.codec = codec;
        validate_outcome_kind_v2(&response.outcome, expected_result)?;
        validate_outcome_evidence_v2(&response.outcome, expected_result, limits, plan)?;
        if response.encode_payload()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_response_wire_mismatch",
                "V2 response bytes normalize after trusted reconstruction",
            ));
        }
        Ok(response)
    }

    /// Return the typed outcome.
    #[must_use]
    pub const fn outcome(&self) -> &ModelOutcome {
        &self.outcome
    }

    /// Consume the response and return its typed outcome.
    #[must_use]
    pub fn into_outcome(self) -> ModelOutcome {
        self.outcome
    }
}

fn encode_minimal_signed_failure_v2(
    original: &RemoteQueryFailureV2,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"category\":\"integrity\",\"code\":\"query_remote_v2_internal_failure\",\"details\":{},\"format\":\"typebridge.query-remote-failure/v2\",\"message\":\"executor could not encode the original V2 remote failure\",\"nonce\":";
    let nonce = original
        .nonce
        .as_deref()
        .filter(|nonce| check_nonce_v2(nonce).is_ok());
    let request = original.request.as_deref().filter(|request| {
        request.len() == 64
            && request
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    let mut payload = Vec::with_capacity(PREFIX.len() + 256);
    payload.extend_from_slice(PREFIX);
    append_optional_safe_ascii_v2(&mut payload, nonce);
    payload.extend_from_slice(b",\"path\":[],\"request\":");
    append_optional_safe_ascii_v2(&mut payload, request);
    payload.push(b'}');
    crate::query_remote::encode_signed_reply_payload_unchecked(&payload, advertisement, signer)
}

fn append_optional_safe_ascii_v2(encoded: &mut Vec<u8>, value: Option<&str>) {
    if let Some(value) = value {
        encoded.push(b'"');
        encoded.extend_from_slice(value.as_bytes());
        encoded.push(b'"');
    } else {
        encoded.extend_from_slice(b"null");
    }
}

/// One complete structured V2 remote failure bound to its request when one
/// decoded far enough to establish correlation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryFailureV2 {
    category: DiagnosticCategory,
    code: DiagnosticCode,
    details: BTreeMap<String, RemoteDiagnosticDetail>,
    format: String,
    message: String,
    nonce: Option<String>,
    path: Vec<RemoteDiagnosticPathSegment>,
    request: Option<String>,
}

impl RemoteQueryFailureV2 {
    /// Construct an uncorrelated V2 failure for a pre-request transport error.
    pub fn new(nonce: Option<String>, diagnostic: &Diagnostic) -> Result<Self, Diagnostic> {
        let diagnostic = RemoteDiagnostic::new(diagnostic)?;
        if let Some(nonce) = nonce.as_deref() {
            check_nonce_v2(nonce)?;
        }
        Ok(Self {
            category: diagnostic.category,
            code: diagnostic.code,
            details: diagnostic.details,
            format: QUERY_REMOTE_FAILURE_FORMAT_V2.to_owned(),
            message: diagnostic.message,
            nonce,
            path: diagnostic.path,
            request: None,
        })
    }

    /// Bind all five diagnostic fields to the exact V2 request.
    pub fn bound(
        nonce: impl Into<String>,
        request: &RemoteRequestFingerprintV2,
        diagnostic: &Diagnostic,
    ) -> Result<Self, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        let mut failure = Self::new(Some(nonce), diagnostic)?;
        failure.request = Some(request.digest_hex());
        Ok(failure)
    }

    /// Verify the mandatory request correlation of a request-bound failure.
    pub fn verify_binding(
        &self,
        expected_nonce: &str,
        expected_request: &RemoteRequestFingerprintV2,
    ) -> Result<(), Diagnostic> {
        verify_failure_binding_v2(
            self.nonce.as_deref(),
            self.request.as_deref(),
            expected_nonce,
            expected_request,
        )
    }

    /// Encode one authenticated V2 failure in the unchanged signed outer
    /// envelope.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        crate::query_remote::encode_signed_reply_payload(
            &self.encode_payload()?,
            advertisement,
            signer,
        )
    }

    /// Encode an authenticated failure, replacing an unencodable diagnostic
    /// with a fixed bounded V2 internal failure while retaining safe request
    /// bindings.
    #[must_use]
    pub fn encode_signed_or_fallback(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Vec<u8> {
        match self.encode_signed(advertisement, signer) {
            Ok(encoded) => encoded,
            Err(_) => encode_minimal_signed_failure_v2(self, advertisement, signer),
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        self.remote_diagnostic()?.validate_size()?;
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode an already authenticated V2 failure payload.
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, Diagnostic> {
        require_format(
            bytes,
            QUERY_REMOTE_FAILURE_FORMAT_V2,
            "remote failure",
            MAX_REMOTE_ENVELOPE_BYTES,
        )?;
        let maximum = MAX_DIAGNOSTIC_BYTES.saturating_add(FAILURE_FRAMING_ALLOWANCE_BYTES);
        if bytes.len() > maximum {
            return Err(remote_v2_failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_diagnostic_too_large",
                "V2 remote diagnostic exceeds the canonical byte ceiling",
            ));
        }
        let failure = from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if let Some(nonce) = failure.nonce.as_deref() {
            check_nonce_v2(nonce)?;
        }
        failure.remote_diagnostic()?.validate_size()?;
        if failure.encode_payload()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_failure_wire_mismatch",
                "V2 failure bytes normalize after trusted reconstruction",
            ));
        }
        Ok(failure)
    }

    /// Rebuild the exact structured diagnostic.
    pub fn diagnostic(&self) -> Result<Diagnostic, Diagnostic> {
        self.remote_diagnostic()?.diagnostic()
    }

    /// Return the echoed nonce, when request decoding reached it.
    #[must_use]
    pub fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    fn remote_diagnostic(&self) -> Result<RemoteDiagnostic, Diagnostic> {
        let diagnostic = RemoteDiagnostic {
            category: self.category,
            code: self.code.clone(),
            details: self.details.clone(),
            message: self.message.clone(),
            path: self.path.clone(),
        };
        validate_remote_diagnostic_wire(&diagnostic)?;
        Ok(diagnostic)
    }
}

/// One decoded and correlated V2 remote reply.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteReplyV2 {
    /// A successful V2 response.
    Response(RemoteQueryResponseV2),
    /// A fully structured V2 failure.
    Failure(RemoteQueryFailureV2),
}

#[derive(Deserialize)]
struct RemoteReplyBindingPeekV2<'wire> {
    #[serde(borrow)]
    format: &'wire str,
    #[serde(borrow)]
    nonce: Option<&'wire str>,
    #[serde(borrow)]
    plan: Option<&'wire str>,
    #[serde(borrow)]
    request: Option<&'wire str>,
}

/// Authenticate, correlate, preflight, and decode one V2 remote reply.
///
/// Signature and request correlation checks precede allocation of the typed
/// outcome or reconstruction of a payload-selected diagnostic.
#[expect(
    clippy::too_many_arguments,
    reason = "the trust-boundary API keeps every expected binding and verifier explicit"
)]
pub fn decode_remote_reply_v2(
    bytes: &[u8],
    expected_request_envelope: &RemoteQueryRequestV2,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    limits: ModelReplyDecodeLimits,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteReplyV2, Diagnostic> {
    validate_reply_limits_v2(expected_request_envelope.limits(), limits)?;
    // An error reply is not charged to the success-data byte budget, so the
    // first check is the envelope ceiling alone.
    let envelope_bytes = limits.envelope_bytes();
    crate::query_remote::preflight_signed_reply_size(
        bytes,
        u64::try_from(envelope_bytes).unwrap_or(u64::MAX),
    )?;
    let payload = crate::query_remote::verify_signed_reply_payload(
        bytes,
        expected_advertisement,
        trusted_key,
        verifier,
    )?;
    let plan = validate_expected_request_binding_v2(
        expected_request_envelope,
        expected_plan,
        expected_request,
        expected_advertisement,
    )?;
    let peek = peek_remote_reply_binding_v2(payload, envelope_bytes)?;
    if peek.format == QUERY_REMOTE_RESPONSE_FORMAT_V2 {
        verify_response_binding_v2(
            peek.nonce,
            peek.plan,
            peek.request,
            expected_request_envelope.nonce(),
            expected_plan,
            expected_request,
        )?;
        crate::query_remote::preflight_signed_reply_size(bytes, limits.max_bytes)?;
        let response = RemoteQueryResponseV2::decode_bound(
            payload,
            expected_request_envelope.result_kind(),
            limits,
            Some(&plan),
        )?;
        return Ok(RemoteReplyV2::Response(response));
    }
    if peek.format == QUERY_REMOTE_FAILURE_FORMAT_V2 {
        verify_failure_binding_v2(
            peek.nonce,
            peek.request,
            expected_request_envelope.nonce(),
            expected_request,
        )?;
        let failure = RemoteQueryFailureV2::decode_payload(payload)?;
        failure.verify_binding(expected_request_envelope.nonce(), expected_request)?;
        return Ok(RemoteReplyV2::Failure(failure));
    }
    Err(remote_v2_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_v2_format_unsupported",
        "signed payload is not a V2 remote response or failure",
    ))
}

fn validate_reply_limits_v2(
    request: RemoteModelLimits,
    reply: ModelReplyDecodeLimits,
) -> Result<(), Diagnostic> {
    let widened = reply.max_bytes > request.max_bytes
        || reply.max_items > request.max_items
        || reply.max_collection_members > request.max_collection_members
        || reply.max_graph_nodes > request.max_graph_nodes
        || reply.max_attribute_values > request.max_attribute_values
        || reply.max_role_players > request.max_role_players;
    if widened {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_limits_widened",
            "reply decode budgets must monotonically tighten the request-bound limits",
        ));
    }
    Ok(())
}

fn validate_expected_request_binding_v2(
    request: &RemoteQueryRequestV2,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
    expected_advertisement: &RemoteCapabilitiesFingerprint,
) -> Result<QueryPlan, Diagnostic> {
    let plan = request.plan()?;
    let actual_advertisement =
        FingerprintDigest::from_hex(&request.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
    let coherent = plan.fingerprint()? == *expected_plan
        && request.fingerprint()? == *expected_request
        && actual_advertisement == expected_advertisement.as_fingerprint().digest();
    if !coherent {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_expected_binding_mismatch",
            "trusted V2 reply bindings do not describe the expected request envelope",
        ));
    }
    Ok(plan)
}

/// Authenticate and decode an uncorrelated V2 pre-request failure.
pub fn decode_signed_remote_failure_v2(
    bytes: &[u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    max_bytes: u64,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteQueryFailureV2, Diagnostic> {
    // A failure carries no result data, so it never needs more than the
    // default envelope whatever byte budget the caller raised.
    crate::query_remote::preflight_signed_reply_size(
        bytes,
        max_bytes.min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX)),
    )?;
    let payload = crate::query_remote::verify_signed_reply_payload(
        bytes,
        expected_advertisement,
        trusted_key,
        verifier,
    )?;
    let failure = RemoteQueryFailureV2::decode_payload(payload)?;
    if failure.request.is_some() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unexpected_binding",
            "a pre-request V2 failure must not carry a request fingerprint",
        ));
    }
    Ok(failure)
}

#[derive(Deserialize)]
struct FormatPeek<'wire> {
    #[serde(borrow)]
    format: &'wire str,
}

fn require_format(
    bytes: &[u8],
    expected: &str,
    subject: &str,
    maximum_bytes: usize,
) -> Result<(), Diagnostic> {
    if bytes.len() > maximum_bytes {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_envelope_too_large",
            "V2 remote envelope exceeds the hard byte ceiling",
        ));
    }
    let peek = serde_json::from_slice::<FormatPeek<'_>>(bytes).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_format_missing",
            "remote envelope has no readable format discriminator",
        )
    })?;
    if peek.format != expected {
        // The one envelope: a V1 or unknown format is refused here, and the
        // refusal names the format the executor does accept.
        return Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_format_unsupported",
            match subject {
                "remote request" => "remote request is not the V2 wire format",
                "remote response" => "remote response is not the V2 wire format",
                "remote failure" => "remote failure is not the V2 wire format",
                _ => "remote envelope is not the expected V2 wire format",
            },
        )
        .with_detail(
            "supported",
            DiagnosticDetailValue::Text(expected.to_owned()),
        ));
    }
    Ok(())
}

fn validate_v2_plan_and_result(
    plan: &QueryPlan,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let has_model_contract = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
        .is_some();
    if has_model_contract != result.compatibility_terminal() {
        return Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_result_contract_mismatch",
            "V2 result family does not match the plan's model-query contract",
        ));
    }
    if let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    {
        let matches = matches!(
            (result, model),
            (
                RemoteResultKindV2::HydratedRows,
                crate::query_plan::ModelQueryV2::Rows { .. }
            ) | (
                RemoteResultKindV2::HydratedPage,
                crate::query_plan::ModelQueryV2::Page { .. }
            ) | (
                RemoteResultKindV2::DistinctCount,
                crate::query_plan::ModelQueryV2::DistinctCount { .. }
            ) | (
                RemoteResultKindV2::DistinctExists,
                crate::query_plan::ModelQueryV2::DistinctExists { .. }
            ) | (
                RemoteResultKindV2::ModelReduction,
                crate::query_plan::ModelQueryV2::Reduction { .. }
            )
        );
        if !matches {
            return Err(remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_result_contract_mismatch",
                "V2 result family contradicts the plan's exact model-query terminal",
            ));
        }
    } else {
        match (result, plan.output()) {
            (RemoteResultKindV2::Rows, QueryOutput::Rows { .. })
            | (RemoteResultKindV2::Documents, QueryOutput::Documents { .. })
            | (RemoteResultKindV2::Count | RemoteResultKindV2::Exists, _) => {}
            _ => {
                return Err(remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_result_contract_mismatch",
                    "low-level result family contradicts the plan output",
                ));
            }
        }
    }
    Ok(())
}

fn check_nonce_v2(nonce: &str) -> Result<(), Diagnostic> {
    let valid = (NONCE_MIN_BYTES..=NONCE_MAX_BYTES).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_nonce_invalid",
            "V2 request nonces are 16-128 ASCII alphanumeric or dash bytes",
        ))
    }
}

fn validate_remote_limits_v2(limits: RemoteModelLimits) -> Result<(), Diagnostic> {
    if limits
        .deadline_ms
        .is_some_and(|value| value > MAX_REMOTE_DEADLINE_MS)
    {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_deadline_limit",
            "V2 remote deadline exceeds the maximum supported duration",
        ));
    }
    if limits.max_statements > DEFAULT_MODEL_MAX_STATEMENTS {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_statement_limit",
            "V2 remote statement budget exceeds the canonical ceiling",
        ));
    }
    Ok(())
}

fn validate_remote_time_shape_v2(
    prepared_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    limits: RemoteModelLimits,
) -> Result<(), Diagnostic> {
    let lifetime = expires_at_unix_ms
        .checked_sub(prepared_at_unix_ms)
        .ok_or_else(remote_time_invalid_v2)?;
    if lifetime != limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS) {
        return Err(remote_time_invalid_v2());
    }
    Ok(())
}

fn remote_time_invalid_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_v2_time_invalid",
        "V2 request timestamps do not form a bounded absolute lifetime",
    )
}

fn peek_remote_reply_binding_v2(
    bytes: &[u8],
    max_bytes: usize,
) -> Result<RemoteReplyBindingPeekV2<'_>, Diagnostic> {
    if bytes.len() > max_bytes {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_envelope_too_large",
            "V2 remote reply exceeds the envelope byte ceiling",
        ));
    }
    serde_json::from_slice(bytes).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_reply_malformed",
            "V2 remote reply has no readable correlation fields",
        )
    })
}

fn verify_response_binding_v2(
    nonce: Option<&str>,
    plan: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
) -> Result<(), Diagnostic> {
    if nonce != Some(expected_nonce) {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_nonce_mismatch",
            "V2 response does not echo the request nonce",
        ));
    }
    let Some(plan) = plan else {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_plan_mismatch",
            "V2 response does not bind the invoked plan",
        ));
    };
    let echoed_plan = FingerprintDigest::from_hex(plan)?;
    if echoed_plan != expected_plan.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_plan_mismatch",
            "V2 response does not bind the invoked plan",
        ));
    }
    let Some(request) = request else {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 response does not bind the exact request",
        ));
    };
    let echoed_request = FingerprintDigest::from_hex(request)?;
    if echoed_request != expected_request.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 response does not bind the exact request",
        ));
    }
    Ok(())
}

fn verify_failure_binding_v2(
    nonce: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_request: &RemoteRequestFingerprintV2,
) -> Result<(), Diagnostic> {
    let nonce = nonce.ok_or_else(|| {
        remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unbound",
            "V2 failure is not bound to the request nonce",
        )
    })?;
    if nonce != expected_nonce {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_nonce_mismatch",
            "V2 failure does not echo the request nonce",
        ));
    }
    let request = request.ok_or_else(|| {
        remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unbound",
            "V2 failure is not bound to the request fingerprint",
        )
    })?;
    let echoed_request = FingerprintDigest::from_hex(request)?;
    if echoed_request != expected_request.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 failure does not bind the exact request",
        ));
    }
    Ok(())
}

fn validate_outcome_kind_v2(
    outcome: &ModelOutcome,
    expected: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let actual = match outcome {
        ModelOutcome::Rows { .. } => RemoteResultKindV2::Rows,
        ModelOutcome::Documents { .. } => RemoteResultKindV2::Documents,
        ModelOutcome::Count { .. } => RemoteResultKindV2::Count,
        ModelOutcome::Exists { .. } => RemoteResultKindV2::Exists,
        ModelOutcome::HydratedRows { .. } => RemoteResultKindV2::HydratedRows,
        ModelOutcome::HydratedPage { .. } => RemoteResultKindV2::HydratedPage,
        ModelOutcome::DistinctCount { .. } => RemoteResultKindV2::DistinctCount,
        ModelOutcome::DistinctExists { .. } => RemoteResultKindV2::DistinctExists,
        ModelOutcome::ModelReduction { .. } => RemoteResultKindV2::ModelReduction,
    };
    if actual == expected {
        Ok(())
    } else {
        Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_outcome_mismatch",
            "V2 response outcome kind does not match the request-bound terminal",
        ))
    }
}

/// Validate one locally constructed V2 outcome against the exact plan,
/// terminal, and request-tightened evidence budgets.
///
/// This is the contract-owned gate for compatibility executors before they
/// hand evidence to the response serializer. It checks terminal identity,
/// low-level widths, model projections, hydration graph authority, and every
/// item/collection/graph budget. The authenticated outer-envelope byte budget
/// remains a property of the final signed response and is checked by
/// [`RemoteQueryResponseV2::encode_signed`] consumers.
pub fn validate_remote_outcome_v2(
    outcome: &ModelOutcome,
    expected: RemoteResultKindV2,
    limits: ModelReplyDecodeLimits,
    plan: &QueryPlan,
) -> Result<(), Diagnostic> {
    validate_outcome_evidence_v2(outcome, expected, limits, Some(plan))
}

fn validate_outcome_evidence_v2(
    outcome: &ModelOutcome,
    expected: RemoteResultKindV2,
    limits: ModelReplyDecodeLimits,
    plan: Option<&QueryPlan>,
) -> Result<(), Diagnostic> {
    validate_outcome_kind_v2(outcome, expected)?;
    match outcome {
        ModelOutcome::Rows { rows } => {
            check_item_budget(rows.len(), limits.max_items)?;
            let expected_width = plan.and_then(low_level_output_width);
            validate_low_level_rows(rows, expected_width)?;
        }
        ModelOutcome::Documents { documents } => {
            check_item_budget(documents.len(), limits.max_items)?;
            let expected_width = plan.and_then(low_level_output_width);
            let mut members = 0_u64;
            let width = expected_width.or_else(|| documents.first().map(Vec::len));
            for document in documents {
                if width.is_some_and(|width| document.len() != width)
                    || document.is_empty()
                    || document.len() > MAX_SELECTED_SLOTS
                {
                    return Err(remote_evidence_mismatch(
                        "V2 document width contradicts the validated output",
                    ));
                }
                for field in document {
                    if matches!(
                        field,
                        RemoteFieldValue::Document { .. } | RemoteFieldValue::Documents { .. }
                    ) {
                        return Err(remote_evidence_mismatch(
                            "V2 documents cannot contain nested fields",
                        ));
                    }
                    if let RemoteFieldValue::List { values } = field {
                        add_budgeted(
                            &mut members,
                            values.len(),
                            limits.max_collection_members,
                            remote_collection_limit_v2,
                        )?;
                    }
                }
            }
        }
        ModelOutcome::Count { value } => {
            if *value > limits.max_items {
                return Err(remote_item_limit_v2());
            }
        }
        ModelOutcome::Exists { value } => {
            if *value && limits.max_items == 0 {
                return Err(remote_item_limit_v2());
            }
        }
        ModelOutcome::HydratedRows { graph, rows } => {
            check_item_budget(rows.len(), limits.max_items)?;
            validate_hydration_graph_budget(graph, limits)?;
            validate_hydrated_rows(graph, rows, limits, plan, expected)?;
        }
        ModelOutcome::HydratedPage {
            entries,
            graph,
            limit,
            offset,
            root,
            total,
        } => {
            check_item_budget(entries.len(), limits.max_items)?;
            if u64::try_from(entries.len()).unwrap_or(u64::MAX) > *limit {
                return Err(remote_evidence_mismatch(
                    "V2 page contains more entries than its exact limit",
                ));
            }
            if let Some(total) = total {
                let remaining = total.saturating_sub(*offset);
                let exact_len = remaining.min(*limit);
                if u64::try_from(entries.len()).unwrap_or(u64::MAX) != exact_len {
                    return Err(remote_evidence_mismatch(
                        "V2 page entries contradict its same-snapshot total and window",
                    ));
                }
            }
            validate_hydration_graph_budget(graph, limits)?;
            validate_hydrated_rows(graph, entries, limits, plan, expected)?;
            validate_page_root_distinct(entries, *root, plan)?;
            validate_page_contract(plan, *root, *offset, *limit, total.is_some())?;
        }
        ModelOutcome::DistinctCount { root, value } => {
            validate_scalar_root(plan, expected, *root)?;
            if *value > limits.max_items {
                return Err(remote_item_limit_v2());
            }
        }
        ModelOutcome::DistinctExists { root, value } => {
            validate_scalar_root(plan, expected, *root)?;
            if *value && limits.max_items == 0 {
                return Err(remote_item_limit_v2());
            }
        }
        ModelOutcome::ModelReduction {
            graph,
            root,
            group,
            reducers,
            rows,
        } => validate_model_reduction(graph, *root, group, reducers, rows, limits, plan, expected)?,
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_model_reduction(
    graph: &HydrationGraph,
    root: BindingId,
    group: &Option<ModelReductionKey>,
    reducers: &[ModelReductionTerm],
    rows: &[ModelReductionRow],
    limits: ModelReplyDecodeLimits,
    plan: Option<&QueryPlan>,
    expected: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    if expected != RemoteResultKindV2::ModelReduction {
        return Err(remote_evidence_mismatch(
            "typed reduction outcome has the wrong terminal family",
        ));
    }
    let Some(crate::query_plan::ModelQueryV2::Reduction {
        hydration,
        root: expected_root,
        group: expected_group,
        reducers: expected_reducers,
    }) = plan
        .and_then(QueryPlan::v2_compatibility)
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch(
            "typed reduction outcome has no request-bound terminal contract",
        ));
    };
    if root != *expected_root || group != expected_group || reducers != expected_reducers {
        return Err(remote_evidence_mismatch(
            "typed reduction outcome does not echo its exact root, group, and reducer contract",
        ));
    }
    check_item_budget(rows.len(), limits.max_items)?;
    if group.is_none() && rows.len() != 1 {
        return Err(remote_evidence_mismatch(
            "ungrouped reduction must carry exactly one total row",
        ));
    }
    let mut cells = 0_u64;
    for row in rows {
        let group_cells = match (group.as_ref(), row.group()) {
            (None, None) => 0,
            (Some(ModelReductionKey::Binding { .. }), Some(ModelReductionGroup::Thing { .. }))
            | (Some(ModelReductionKey::Field { .. }), Some(ModelReductionGroup::Field { .. })) => 1,
            (
                Some(ModelReductionKey::Fields { fields }),
                Some(ModelReductionGroup::Fields { values }),
            ) if fields.len() == values.len() => u64::try_from(values.len()).unwrap_or(u64::MAX),
            _ => {
                return Err(remote_evidence_mismatch(
                    "typed reduction row group shape contradicts its exact contract",
                ));
            }
        };
        if row.values().len() != reducers.len() {
            return Err(remote_evidence_mismatch(
                "typed reduction row width contradicts its exact reducer contract",
            ));
        }
        cells = cells
            .checked_add(group_cells)
            .and_then(|value| {
                value.checked_add(u64::try_from(row.values().len()).unwrap_or(u64::MAX))
            })
            .ok_or_else(remote_collection_limit_v2)?;
        if cells > limits.max_collection_members {
            return Err(remote_collection_limit_v2());
        }
        validate_reduction_group_value(group.as_ref(), row.group())?;
        for (term, value) in reducers.iter().zip(row.values()) {
            validate_reduced_value(term, value, limits.max_items)?;
        }
    }
    validate_reduction_group_order(rows)?;
    validate_hydration_graph_budget(graph, limits)?;
    validate_reduction_graph(graph, rows, group.as_ref(), hydration)
}

fn validate_reduction_group_value(
    contract: Option<&ModelReductionKey>,
    value: Option<&ModelReductionGroup>,
) -> Result<(), Diagnostic> {
    match (contract, value) {
        (None, None)
        | (Some(ModelReductionKey::Binding { .. }), Some(ModelReductionGroup::Thing { .. })) => {
            Ok(())
        }
        (Some(ModelReductionKey::Field { field }), Some(ModelReductionGroup::Field { value }))
            if value.value_type() == field.value_type() =>
        {
            validate_reduction_group_scalar(value)
        }
        (
            Some(ModelReductionKey::Fields { fields }),
            Some(ModelReductionGroup::Fields { values }),
        ) if fields.len() == values.len()
            && fields
                .iter()
                .zip(values)
                .all(|(field, value)| field.value_type() == value.value_type()) =>
        {
            for value in values {
                validate_reduction_group_scalar(value)?;
            }
            Ok(())
        }
        _ => Err(remote_evidence_mismatch(
            "typed reduction group value contradicts its exact scalar domain",
        )),
    }
}

fn validate_reduction_group_scalar(value: &ModelValue) -> Result<(), Diagnostic> {
    if matches!(
        value.canonical_value(),
        Some(crate::value::CanonicalValue::Double(value))
            if value.bits() == (-0.0_f64).to_bits()
    ) {
        return Err(remote_evidence_mismatch(
            "typed double zero reduction groups must use the canonical positive-zero representative",
        ));
    }
    Ok(())
}

fn validate_reduced_value(
    term: &ModelReductionTerm,
    value: &ModelReducedValue,
    max_items: u64,
) -> Result<(), Diagnostic> {
    let valid = match (term.reduction(), term.input(), value) {
        (crate::query_plan::ModelReductionKind::Count, _, ModelReducedValue::Count { value }) => {
            if *value > max_items {
                return Err(remote_item_limit_v2());
            }
            true
        }
        (reduction, Some(field), ModelReducedValue::Long { .. }) => {
            field.value_type() == crate::value::ValueTypeTag::Long && !reduction.widens_to_double()
        }
        (reduction, Some(field), ModelReducedValue::DoubleBits { value }) => {
            if value.is_some_and(|bits| !f64::from_bits(bits).is_finite()) {
                return Err(remote_evidence_mismatch(
                    "typed double reducer bits must encode a finite IEEE value",
                ));
            }
            field.value_type() == crate::value::ValueTypeTag::Double || reduction.widens_to_double()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(remote_evidence_mismatch(
            "typed reducer value contradicts its exact operation and scalar domain",
        ))
    }
}

fn validate_reduction_group_order(rows: &[ModelReductionRow]) -> Result<(), Diagnostic> {
    for pair in rows.windows(2) {
        let ordered = match (pair[0].group(), pair[1].group()) {
            (None, None) => false,
            (
                Some(ModelReductionGroup::Thing { value: left }),
                Some(ModelReductionGroup::Thing { value: right }),
            ) => left.node() < right.node(),
            (
                Some(ModelReductionGroup::Field { value: left }),
                Some(ModelReductionGroup::Field { value: right }),
            ) => left.semantic_cmp_same_domain(right) == Some(Ordering::Less),
            (
                Some(ModelReductionGroup::Fields { values: left }),
                Some(ModelReductionGroup::Fields { values: right }),
            ) => semantic_tuple_cmp(left, right)? == Ordering::Less,
            _ => false,
        };
        if !ordered {
            return Err(remote_evidence_mismatch(
                "typed reduction groups must be semantically sorted and unique",
            ));
        }
    }
    Ok(())
}

fn semantic_tuple_cmp(left: &[ModelValue], right: &[ModelValue]) -> Result<Ordering, Diagnostic> {
    for (left, right) in left.iter().zip(right) {
        let ordering = left
            .semantic_cmp_same_domain(right)
            .ok_or_else(|| remote_evidence_mismatch("tuple reduction groups are not comparable"))?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(left.len().cmp(&right.len()))
}

fn low_level_output_width(plan: &QueryPlan) -> Option<usize> {
    match plan.output() {
        QueryOutput::Rows { columns } => Some(columns.len()),
        QueryOutput::Documents { fields } => Some(fields.len()),
    }
}

fn validate_low_level_rows(
    rows: &[Vec<RemoteValue>],
    expected_width: Option<usize>,
) -> Result<(), Diagnostic> {
    let width = expected_width.or_else(|| rows.first().map(Vec::len));
    for row in rows {
        if width.is_some_and(|width| row.len() != width)
            || row.is_empty()
            || row.len() > MAX_SELECTED_SLOTS
        {
            return Err(remote_evidence_mismatch(
                "V2 row width contradicts the validated output",
            ));
        }
        for value in row {
            match value {
                RemoteValue::Thing { iid, type_id } => {
                    if !is_canonical_thing_iid(iid)
                        || !matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation)
                    {
                        return Err(remote_evidence_mismatch(
                            "V2 thing evidence has an invalid IID or type kind",
                        ));
                    }
                }
                RemoteValue::Attribute { type_id, .. } => {
                    if type_id.kind() != TypeKind::Attribute {
                        return Err(remote_evidence_mismatch(
                            "V2 attribute evidence has a non-attribute type",
                        ));
                    }
                }
                RemoteValue::Value { .. } | RemoteValue::Absent => {}
            }
        }
    }
    Ok(())
}

fn validate_hydration_graph_budget(
    graph: &HydrationGraph,
    limits: ModelReplyDecodeLimits,
) -> Result<(), Diagnostic> {
    graph.validate()?;
    if u64::try_from(graph.nodes.len()).unwrap_or(u64::MAX) > limits.max_graph_nodes {
        return Err(remote_graph_limit_v2());
    }
    let mut attribute_values = 0_u64;
    let mut role_players = 0_u64;
    for node in &graph.nodes {
        for attribute in &node.attributes {
            add_budgeted(
                &mut attribute_values,
                attribute.values.len(),
                limits.max_attribute_values,
                remote_attribute_limit_v2,
            )?;
        }
        for role in &node.roles {
            add_budgeted(
                &mut role_players,
                role.players.len(),
                limits.max_role_players,
                remote_role_player_limit_v2,
            )?;
        }
    }
    Ok(())
}

fn validate_hydrated_rows(
    graph: &HydrationGraph,
    rows: &[HydratedRow],
    limits: ModelReplyDecodeLimits,
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let expected_width = model_output_width(plan, result);
    let width = expected_width.or_else(|| rows.first().map(|row| row.slots.len()));
    let mut collection_members = 0_u64;
    let mut seen_rows = BTreeSet::new();
    for row in rows {
        if width.is_some_and(|width| row.slots.len() != width)
            || row.slots.is_empty()
            || row.slots.len() > MAX_SELECTED_SLOTS
        {
            return Err(remote_evidence_mismatch(
                "hydrated row width contradicts the model output contract",
            ));
        }
        if !seen_rows.insert(row.clone()) {
            return Err(remote_evidence_mismatch(
                "hydrated rows must be distinct selected-identity tuples",
            ));
        }
        for slot in &row.slots {
            if let HydrationSlot::Collection { values } = slot {
                add_budgeted(
                    &mut collection_members,
                    values.len(),
                    limits.max_collection_members,
                    remote_collection_limit_v2,
                )?;
            }
            for reference in slot.references() {
                validate_graph_reference(graph, reference)?;
            }
        }
    }
    validate_model_rows_against_plan(rows, plan, result)?;
    validate_graph_against_model_plan(graph, rows, plan, result)?;
    validate_model_order_against_plan(graph, rows, plan, result)
}

fn validate_graph_reference(
    graph: &HydrationGraph,
    reference: &HydrationReference,
) -> Result<(), Diagnostic> {
    let index = usize::try_from(reference.node.get())
        .map_err(|_| remote_evidence_mismatch("hydration slot references an unknown node"))?;
    let Some(node) = graph.nodes.get(index) else {
        return Err(remote_evidence_mismatch(
            "hydration slot references an unknown node",
        ));
    };
    if reference.declared.kind() != node.concrete.kind() {
        return Err(remote_evidence_mismatch(
            "hydration slot declared and concrete kinds disagree",
        ));
    }
    Ok(())
}

fn check_item_budget(actual: usize, maximum: u64) -> Result<(), Diagnostic> {
    if u64::try_from(actual).unwrap_or(u64::MAX) > maximum {
        Err(remote_item_limit_v2())
    } else {
        Ok(())
    }
}

fn add_budgeted(
    total: &mut u64,
    increment: usize,
    maximum: u64,
    failure: fn() -> Diagnostic,
) -> Result<(), Diagnostic> {
    *total = total.saturating_add(u64::try_from(increment).unwrap_or(u64::MAX));
    if *total > maximum {
        Err(failure())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
struct ResponseOutcomeRaw<'wire> {
    #[serde(borrow)]
    outcome: &'wire serde_json::value::RawValue,
}

fn preflight_response_evidence_v2(
    payload: &[u8],
    expected: RemoteResultKindV2,
    output_width: Option<usize>,
    limits: ModelReplyDecodeLimits,
) -> Result<(), Diagnostic> {
    let response = serde_json::from_slice::<ResponseOutcomeRaw<'_>>(payload).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_reply_malformed",
            "V2 response has no readable outcome",
        )
    })?;
    let mut state = StreamingPreflightState::default();
    let mut deserializer = serde_json::Deserializer::from_str(response.outcome.get());
    let result = OutcomeScanSeed {
        expected,
        limits,
        output_width,
        state: &mut state,
    }
    .deserialize(&mut deserializer)
    .and_then(|()| deserializer.end());
    result.map_err(|_| {
        state.failure.map_or_else(
            || {
                remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_reply_malformed",
                    "V2 response outcome is malformed",
                )
            },
            StreamingPreflightFailure::diagnostic,
        )
    })
}

#[derive(Clone, Copy)]
enum StreamingPreflightFailure {
    AttributeValues,
    CollectionMembers,
    GraphNodes,
    Items,
    Outcome,
    RolePlayers,
}

impl StreamingPreflightFailure {
    fn diagnostic(self) -> Diagnostic {
        match self {
            Self::AttributeValues => remote_attribute_limit_v2(),
            Self::CollectionMembers => remote_collection_limit_v2(),
            Self::GraphNodes => remote_graph_limit_v2(),
            Self::Items => remote_item_limit_v2(),
            Self::Outcome => remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_outcome_mismatch",
                "V2 response outcome kind does not match the request-bound terminal",
            ),
            Self::RolePlayers => remote_role_player_limit_v2(),
        }
    }
}

#[derive(Default)]
struct StreamingPreflightState {
    attribute_values: u64,
    collection_members: u64,
    failure: Option<StreamingPreflightFailure>,
    graph_nodes: u64,
    items: u64,
    role_players: u64,
}

impl StreamingPreflightState {
    fn increment(
        &mut self,
        counter: StreamingCounter,
        maximum: u64,
        failure: StreamingPreflightFailure,
    ) -> Result<(), &'static str> {
        let value = match counter {
            StreamingCounter::AttributeValues => &mut self.attribute_values,
            StreamingCounter::CollectionMembers => &mut self.collection_members,
            StreamingCounter::GraphNodes => &mut self.graph_nodes,
            StreamingCounter::Items => &mut self.items,
            StreamingCounter::RolePlayers => &mut self.role_players,
        };
        *value = value.saturating_add(1);
        if *value > maximum {
            self.failure = Some(failure);
            Err("V2 response budget exceeded")
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum StreamingCounter {
    AttributeValues,
    CollectionMembers,
    GraphNodes,
    Items,
    RolePlayers,
}

struct OutcomeScanSeed<'scan> {
    expected: RemoteResultKindV2,
    limits: ModelReplyDecodeLimits,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for OutcomeScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(OutcomeScanVisitor {
            expected: self.expected,
            limits: self.limits,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct OutcomeScanVisitor<'scan> {
    expected: RemoteResultKindV2,
    limits: ModelReplyDecodeLimits,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for OutcomeScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a V2 response outcome")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut kind = None;
        while let Some(field) = map.next_key::<String>()? {
            match (field.as_str(), self.expected) {
                ("kind", _) => kind = Some(map.next_value::<String>()?),
                ("rows", RemoteResultKindV2::Rows) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::Row,
                        counter: Some((
                            StreamingCounter::Items,
                            self.limits.max_items,
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("documents", RemoteResultKindV2::Documents) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::Document,
                        counter: Some((
                            StreamingCounter::Items,
                            self.limits.max_items,
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("rows", RemoteResultKindV2::HydratedRows)
                | ("entries", RemoteResultKindV2::HydratedPage) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::HydratedRow,
                        counter: Some((
                            StreamingCounter::Items,
                            self.limits.max_items,
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("rows", RemoteResultKindV2::ModelReduction) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::ReductionRow,
                        counter: Some((
                            StreamingCounter::Items,
                            self.limits.max_items,
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                (
                    "graph",
                    RemoteResultKindV2::HydratedRows
                    | RemoteResultKindV2::HydratedPage
                    | RemoteResultKindV2::ModelReduction,
                ) => {
                    map.next_value_seed(ObjectScanSeed {
                        kind: ScanObject::Graph,
                        limits: self.limits,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("value", RemoteResultKindV2::Count | RemoteResultKindV2::DistinctCount) => {
                    if map.next_value::<u64>()? > self.limits.max_items {
                        self.state.failure = Some(StreamingPreflightFailure::Items);
                        return Err(serde::de::Error::custom("V2 item budget exceeded"));
                    }
                }
                ("value", RemoteResultKindV2::Exists | RemoteResultKindV2::DistinctExists) => {
                    if map.next_value::<bool>()? && self.limits.max_items == 0 {
                        self.state.failure = Some(StreamingPreflightFailure::Items);
                        return Err(serde::de::Error::custom("V2 item budget exceeded"));
                    }
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let expected = match self.expected {
            RemoteResultKindV2::Rows => "rows",
            RemoteResultKindV2::Documents => "documents",
            RemoteResultKindV2::Count => "count",
            RemoteResultKindV2::Exists => "exists",
            RemoteResultKindV2::HydratedRows => "hydrated_rows",
            RemoteResultKindV2::HydratedPage => "hydrated_page",
            RemoteResultKindV2::DistinctCount => "distinct_count",
            RemoteResultKindV2::DistinctExists => "distinct_exists",
            RemoteResultKindV2::ModelReduction => "model_reduction",
        };
        if kind.as_deref() != Some(expected) {
            self.state.failure = Some(StreamingPreflightFailure::Outcome);
            return Err(serde::de::Error::custom("V2 outcome kind mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ScanChild {
    Attribute,
    Document,
    DocumentField,
    GraphNode,
    HydratedRow,
    HydratedSlot,
    Ignored,
    Row,
    Role,
    ReductionRow,
}

struct SequenceScanSeed<'scan> {
    child: ScanChild,
    counter: Option<(StreamingCounter, u64, StreamingPreflightFailure)>,
    limits: ModelReplyDecodeLimits,
    local_maximum: Option<usize>,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for SequenceScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SequenceScanVisitor {
            child: self.child,
            counter: self.counter,
            limits: self.limits,
            local_maximum: self.local_maximum,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct SequenceScanVisitor<'scan> {
    child: ScanChild,
    counter: Option<(StreamingCounter, u64, StreamingPreflightFailure)>,
    limits: ModelReplyDecodeLimits,
    local_maximum: Option<usize>,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for SequenceScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded V2 response sequence")
    }

    fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let mut local_count = 0_usize;
        loop {
            let present = match self.child {
                ScanChild::Ignored => sequence.next_element::<IgnoredAny>()?.is_some(),
                ScanChild::Document => sequence
                    .next_element_seed(SequenceScanSeed {
                        child: ScanChild::DocumentField,
                        counter: None,
                        limits: self.limits,
                        local_maximum: Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS)),
                        output_width: self.output_width,
                        state: self.state,
                    })?
                    .is_some(),
                ScanChild::Row => sequence
                    .next_element_seed(SequenceScanSeed {
                        child: ScanChild::Ignored,
                        counter: None,
                        limits: self.limits,
                        local_maximum: Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS)),
                        output_width: self.output_width,
                        state: self.state,
                    })?
                    .is_some(),
                ScanChild::Attribute
                | ScanChild::DocumentField
                | ScanChild::GraphNode
                | ScanChild::HydratedRow
                | ScanChild::HydratedSlot
                | ScanChild::Role
                | ScanChild::ReductionRow => {
                    let kind = match self.child {
                        ScanChild::Attribute => ScanObject::Attribute,
                        ScanChild::DocumentField => ScanObject::DocumentField,
                        ScanChild::GraphNode => ScanObject::GraphNode,
                        ScanChild::HydratedRow => ScanObject::HydratedRow,
                        ScanChild::HydratedSlot => ScanObject::HydratedSlot,
                        ScanChild::Role => ScanObject::Role,
                        ScanChild::ReductionRow => ScanObject::ReductionRow,
                        ScanChild::Document | ScanChild::Ignored | ScanChild::Row => {
                            return Err(serde::de::Error::custom(
                                "invalid V2 response preflight state",
                            ));
                        }
                    };
                    sequence
                        .next_element_seed(ObjectScanSeed {
                            kind,
                            limits: self.limits,
                            output_width: self.output_width,
                            state: self.state,
                        })?
                        .is_some()
                }
            };
            if !present {
                break;
            }
            local_count = local_count.saturating_add(1);
            if self
                .local_maximum
                .is_some_and(|maximum| local_count > maximum)
            {
                return Err(serde::de::Error::custom(
                    "V2 response sequence exceeds its output width",
                ));
            }
            if let Some((counter, maximum, failure)) = self.counter {
                self.state
                    .increment(counter, maximum, failure)
                    .map_err(serde::de::Error::custom)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ScanObject {
    Attribute,
    DocumentField,
    Graph,
    GraphNode,
    HydratedRow,
    HydratedSlot,
    Role,
    ReductionGroup,
    ReductionRow,
}

struct ObjectScanSeed<'scan> {
    kind: ScanObject,
    limits: ModelReplyDecodeLimits,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for ObjectScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ObjectScanVisitor {
            kind: self.kind,
            limits: self.limits,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct ObjectScanVisitor<'scan> {
    kind: ScanObject,
    limits: ModelReplyDecodeLimits,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for ObjectScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a V2 response evidence object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        while let Some(field) = map.next_key::<String>()? {
            if self.kind == ScanObject::ReductionRow && field == "group" {
                map.next_value_seed(OptionalObjectScanSeed {
                    kind: ScanObject::ReductionGroup,
                    limits: self.limits,
                    output_width: self.output_width,
                    state: self.state,
                })?;
                continue;
            }
            if self.kind == ScanObject::ReductionGroup && field == "value" {
                self.state
                    .increment(
                        StreamingCounter::CollectionMembers,
                        self.limits.max_collection_members,
                        StreamingPreflightFailure::CollectionMembers,
                    )
                    .map_err(serde::de::Error::custom)?;
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let scan = match (self.kind, field.as_str()) {
                (ScanObject::DocumentField | ScanObject::HydratedSlot, "values") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::CollectionMembers,
                        self.limits.max_collection_members,
                        StreamingPreflightFailure::CollectionMembers,
                    )),
                )),
                (ScanObject::Graph, "nodes") => Some((
                    ScanChild::GraphNode,
                    Some((
                        StreamingCounter::GraphNodes,
                        self.limits.max_graph_nodes,
                        StreamingPreflightFailure::GraphNodes,
                    )),
                )),
                (ScanObject::GraphNode, "attributes") => Some((ScanChild::Attribute, None)),
                (ScanObject::GraphNode, "roles") => Some((ScanChild::Role, None)),
                (ScanObject::Attribute, "values") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::AttributeValues,
                        self.limits.max_attribute_values,
                        StreamingPreflightFailure::AttributeValues,
                    )),
                )),
                (ScanObject::Role, "players") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::RolePlayers,
                        self.limits.max_role_players,
                        StreamingPreflightFailure::RolePlayers,
                    )),
                )),
                (ScanObject::HydratedRow, "slots") => Some((ScanChild::HydratedSlot, None)),
                (ScanObject::ReductionRow | ScanObject::ReductionGroup, "values") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::CollectionMembers,
                        self.limits.max_collection_members,
                        StreamingPreflightFailure::CollectionMembers,
                    )),
                )),
                _ => None,
            };
            if let Some((child, counter)) = scan {
                map.next_value_seed(SequenceScanSeed {
                    child,
                    counter,
                    limits: self.limits,
                    local_maximum: if self.kind == ScanObject::HydratedRow && field == "slots" {
                        Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS))
                    } else {
                        None
                    },
                    output_width: self.output_width,
                    state: self.state,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct OptionalObjectScanSeed<'scan> {
    kind: ScanObject,
    limits: ModelReplyDecodeLimits,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for OptionalObjectScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(OptionalObjectScanVisitor { seed: self })
    }
}

struct OptionalObjectScanVisitor<'scan> {
    seed: OptionalObjectScanSeed<'scan>,
}

impl<'de> Visitor<'de> for OptionalObjectScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional V2 evidence object")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        ObjectScanSeed {
            kind: self.seed.kind,
            limits: self.seed.limits,
            output_width: self.seed.output_width,
            state: self.seed.state,
        }
        .deserialize(deserializer)
    }
}

fn model_output_width(plan: Option<&QueryPlan>, result: RemoteResultKindV2) -> Option<usize> {
    let compatibility = plan?.v2_compatibility()?;
    let model = compatibility.model_query()?;
    match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows { output, .. },
        )
        | (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { output, .. },
        ) => Some(output.slots().len()),
        _ => None,
    }
}

fn validate_model_rows_against_plan(
    rows: &[HydratedRow],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch(
            "hydrated result has no model-query contract",
        ));
    };
    match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows {
                cardinality,
                output,
                window,
                ..
            },
        ) => {
            if cardinality.is_exactly_one() && rows.len() != 1 {
                return Err(remote_evidence_mismatch(
                    "exactly-one terminal must carry exactly one hydrated row",
                ));
            }
            if u64::try_from(rows.len()).unwrap_or(u64::MAX) > window.limit() {
                return Err(remote_evidence_mismatch(
                    "hydrated rows exceed the request-bound row window",
                ));
            }
            validate_slot_shapes(rows, output)
        }
        (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { output, .. },
        ) => validate_slot_shapes(rows, output),
        _ => Err(remote_evidence_mismatch(
            "hydrated outcome contradicts the model-query terminal",
        )),
    }
}

fn validate_graph_against_model_plan(
    graph: &HydrationGraph,
    rows: &[HydratedRow],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch(
            "hydration graph has no request-bound projection",
        ));
    };
    let (output, hydration) = match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows {
                output, hydration, ..
            },
        )
        | (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page {
                output, hydration, ..
            },
        ) => (output, hydration),
        _ => {
            return Err(remote_evidence_mismatch(
                "hydration graph contradicts the model-query terminal",
            ));
        }
    };

    let descriptors = hydration
        .descriptors()
        .iter()
        .map(|descriptor| (descriptor.descriptor(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let slots = output.slots();
    let binding_projections = hydration
        .bindings()
        .iter()
        .map(|binding| (binding.binding(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut full_nodes = BTreeSet::new();
    let mut frontier = Vec::new();
    for row in rows {
        for (slot, slot_contract) in row.slots.iter().zip(&slots) {
            let Some(binding) = binding_projections.get(&slot_contract.binding()) else {
                return Err(remote_evidence_mismatch(
                    "hydrated output binding is absent from the projection",
                ));
            };
            for reference in slot.references() {
                let node_index = usize::try_from(reference.node.get()).map_err(|_| {
                    remote_evidence_mismatch("hydrated output references an unknown node")
                })?;
                let Some(node) = graph.nodes.get(node_index) else {
                    return Err(remote_evidence_mismatch(
                        "hydrated output references an unknown node",
                    ));
                };
                if reference.declared != *binding.declared_descriptor()
                    || !binding.concrete_descriptors().contains(&node.concrete)
                {
                    return Err(remote_evidence_mismatch(
                        "hydrated output violates declared-to-concrete authority",
                    ));
                }
                full_nodes.insert(reference.node);
                frontier.push(reference.node);
            }
        }
    }
    for node in &graph.nodes {
        let Some(projection) = descriptors.get(&node.concrete) else {
            return Err(remote_evidence_mismatch(
                "hydration node concrete descriptor is outside the projection",
            ));
        };
        validate_node_projection(graph, node, projection, full_nodes.contains(&node.id))?;
    }
    let mut unique_values = BTreeMap::<(TypeId, AttributeId), Vec<&ModelValue>>::new();
    for node in &graph.nodes {
        let Some(projection) = descriptors.get(&node.concrete) else {
            return Err(remote_evidence_mismatch(
                "hydration node concrete descriptor is outside the projection",
            ));
        };
        let fields = projection
            .fields()
            .iter()
            .map(|field| (field.attribute(), field))
            .collect::<BTreeMap<_, _>>();
        for evidence in &node.attributes {
            let Some(field) = fields.get(&evidence.attribute) else {
                return Err(remote_evidence_mismatch(
                    "hydration attribute is outside the concrete projection",
                ));
            };
            if field.unique() {
                for owner in field.reference_owners() {
                    let authority = (owner.clone(), evidence.attribute.clone());
                    let seen = unique_values.entry(authority).or_default();
                    for value in &evidence.values {
                        seen.push(value);
                    }
                }
            }
        }
    }
    for values in unique_values.values() {
        if contains_semantic_duplicate(values.iter().copied())? {
            return Err(remote_evidence_mismatch(
                "unique hydration field repeats one value across provider identities",
            ));
        }
    }

    let mut referenced = BTreeSet::new();
    while let Some(node_id) = frontier.pop() {
        if !referenced.insert(node_id) {
            continue;
        }
        let node_index = usize::try_from(node_id.get()).map_err(|_| {
            remote_evidence_mismatch("hydration graph traversal reached an unknown node")
        })?;
        let node = graph.nodes.get(node_index).ok_or_else(|| {
            remote_evidence_mismatch("hydration graph traversal reached an unknown node")
        })?;
        if full_nodes.contains(&node_id) {
            for role in &node.roles {
                for player in &role.players {
                    frontier.push(player.node);
                }
            }
        }
    }
    if referenced.len() != graph.nodes.len() {
        return Err(remote_evidence_mismatch(
            "hydration graph contains unreferenced provider identities",
        ));
    }
    Ok(())
}

fn validate_reduction_graph(
    graph: &HydrationGraph,
    rows: &[ModelReductionRow],
    group: Option<&ModelReductionKey>,
    hydration: &crate::query_plan::HydrationProjection,
) -> Result<(), Diagnostic> {
    let Some(ModelReductionKey::Binding { binding }) = group else {
        return if graph.nodes().is_empty() {
            Ok(())
        } else {
            Err(remote_evidence_mismatch(
                "non-thing reduction groups must not carry a hydration graph",
            ))
        };
    };
    let authority = hydration
        .bindings()
        .iter()
        .find(|authority| authority.binding() == *binding)
        .ok_or_else(|| {
            remote_evidence_mismatch(
                "thing reduction group lacks request-bound descriptor authority",
            )
        })?;
    let descriptors = hydration
        .descriptors()
        .iter()
        .map(|descriptor| (descriptor.descriptor(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let mut full_nodes = BTreeSet::new();
    let mut frontier = Vec::new();
    for row in rows {
        let Some(ModelReductionGroup::Thing { value }) = row.group() else {
            return Err(remote_evidence_mismatch(
                "thing reduction row lacks its graph reference",
            ));
        };
        let index = usize::try_from(value.node().get()).map_err(|_| {
            remote_evidence_mismatch("thing reduction group references an unknown node")
        })?;
        let node = graph.nodes().get(index).ok_or_else(|| {
            remote_evidence_mismatch("thing reduction group references an unknown node")
        })?;
        if value.declared() != authority.declared_descriptor()
            || !authority.concrete_descriptors().contains(node.concrete())
        {
            return Err(remote_evidence_mismatch(
                "thing reduction group violates declared-to-concrete authority",
            ));
        }
        full_nodes.insert(value.node());
        frontier.push(value.node());
    }
    for node in graph.nodes() {
        let projection = descriptors.get(node.concrete()).ok_or_else(|| {
            remote_evidence_mismatch("reduction hydration node is outside the request projection")
        })?;
        validate_node_projection(graph, node, projection, full_nodes.contains(&node.id()))?;
    }
    let mut referenced = BTreeSet::new();
    while let Some(node_id) = frontier.pop() {
        if !referenced.insert(node_id) {
            continue;
        }
        let index = usize::try_from(node_id.get()).map_err(|_| {
            remote_evidence_mismatch("reduction graph traversal reached an unknown node")
        })?;
        let node = graph.nodes().get(index).ok_or_else(|| {
            remote_evidence_mismatch("reduction graph traversal reached an unknown node")
        })?;
        if full_nodes.contains(&node_id) {
            for role in node.roles() {
                frontier.extend(role.players().iter().map(HydrationReference::node));
            }
        }
    }
    if referenced.len() != graph.nodes().len() {
        return Err(remote_evidence_mismatch(
            "reduction hydration graph contains unreferenced provider identities",
        ));
    }
    Ok(())
}

fn validate_model_order_against_plan(
    graph: &HydrationGraph,
    rows: &[HydratedRow],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(model) = plan
        .and_then(QueryPlan::v2_compatibility)
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Ok(());
    };
    let (output, row_order) = match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows { order, output, .. },
        ) => (output, order.as_ref()),
        (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { order, output, .. },
        ) => (output, Some(order)),
        _ => {
            return Err(remote_evidence_mismatch(
                "hydrated result has no request-bound order contract",
            ));
        }
    };

    validate_collection_orders(graph, rows, output)?;
    if let Some(order) = row_order {
        validate_row_order(graph, rows, output, order)?;
    }
    if result == RemoteResultKindV2::HydratedRows {
        validate_selected_row_identities(rows, output)?;
    }
    Ok(())
}

fn validate_selected_row_identities(
    rows: &[HydratedRow],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    let singular_bindings = output
        .slots()
        .into_iter()
        .filter(|slot| !slot.collection())
        .map(crate::query_plan::ModelOutputSlot::binding)
        .collect::<Vec<_>>();
    let mut identities = BTreeSet::new();
    for row in rows {
        let identity = row_identity(row, output, &singular_bindings)?;
        if !identities.insert(identity) {
            return Err(remote_evidence_mismatch(
                "hydrated rows repeat one selected singular-identity tuple",
            ));
        }
    }
    Ok(())
}

fn validate_row_order(
    graph: &HydrationGraph,
    rows: &[HydratedRow],
    output: &crate::query_plan::ModelOutputV2,
    order: &crate::query_plan::ModelStableOrder,
) -> Result<(), Diagnostic> {
    for pair in rows.windows(2) {
        let ordering = compare_rows_by_order(graph, &pair[0], &pair[1], output, order)?;
        if ordering == Ordering::Greater {
            return Err(remote_evidence_mismatch(
                "hydrated rows are not in the request-bound stable order",
            ));
        }
        if ordering == Ordering::Equal
            && row_identity(&pair[0], output, order.identity_tiebreakers())?
                != row_identity(&pair[1], output, order.identity_tiebreakers())?
        {
            return Err(remote_evidence_mismatch(
                "hydrated row order does not distinguish two selected identities",
            ));
        }
    }
    Ok(())
}

fn compare_rows_by_order(
    graph: &HydrationGraph,
    left: &HydratedRow,
    right: &HydratedRow,
    output: &crate::query_plan::ModelOutputV2,
    order: &crate::query_plan::ModelStableOrder,
) -> Result<Ordering, Diagnostic> {
    for term in order.terms() {
        let left = row_binding_reference(left, output, term.field().binding())?;
        let right = row_binding_reference(right, output, term.field().binding())?;
        let ordering = compare_order_values(
            order_value(graph, left, term)?,
            order_value(graph, right, term)?,
            term,
        )?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn validate_collection_orders(
    graph: &HydrationGraph,
    rows: &[HydratedRow],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    let output_slots = output.slots();
    for row in rows {
        for (slot, contract) in row.slots().iter().zip(&output_slots) {
            let (
                HydrationSlot::Collection { values },
                crate::query_plan::ModelOutputSlot::Collect { order, .. },
            ) = (slot, *contract)
            else {
                continue;
            };
            for pair in values.windows(2) {
                let ordering = compare_references_by_order(graph, &pair[0], &pair[1], order)?;
                if ordering == Ordering::Greater {
                    return Err(remote_evidence_mismatch(
                        "hydrated collection members are not in the request-bound stable order",
                    ));
                }
                if ordering == Ordering::Equal && pair[0].node() != pair[1].node() {
                    return Err(remote_evidence_mismatch(
                        "hydrated collection order does not distinguish two provider identities",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn compare_references_by_order(
    graph: &HydrationGraph,
    left: &HydrationReference,
    right: &HydrationReference,
    order: &crate::query_plan::ModelStableOrder,
) -> Result<Ordering, Diagnostic> {
    for term in order.terms() {
        let ordering = compare_order_values(
            order_value(graph, left, term)?,
            order_value(graph, right, term)?,
            term,
        )?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn row_identity(
    row: &HydratedRow,
    output: &crate::query_plan::ModelOutputV2,
    bindings: &[BindingId],
) -> Result<Vec<(BindingId, HydrationNodeId)>, Diagnostic> {
    let mut identity = bindings
        .iter()
        .map(|binding| {
            row_binding_reference(row, output, *binding)
                .map(|reference| (*binding, reference.node()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    identity.sort_unstable();
    Ok(identity)
}

fn row_binding_reference<'row>(
    row: &'row HydratedRow,
    output: &crate::query_plan::ModelOutputV2,
    binding: BindingId,
) -> Result<&'row HydrationReference, Diagnostic> {
    let output_slots = output.slots();
    let Some(index) = output_slots
        .iter()
        .position(|slot| slot.binding() == binding)
    else {
        return Err(remote_evidence_mismatch(
            "stable order references a binding absent from the public output",
        ));
    };
    match row.slots().get(index) {
        Some(HydrationSlot::Singular { value }) => Ok(value),
        _ => Err(remote_evidence_mismatch(
            "stable row order requires a singular public binding",
        )),
    }
}

fn order_value<'graph>(
    graph: &'graph HydrationGraph,
    reference: &HydrationReference,
    term: &crate::query_plan::ModelOrderTerm,
) -> Result<Option<&'graph ModelValue>, Diagnostic> {
    let index = usize::try_from(reference.node().get()).map_err(|_| {
        remote_evidence_mismatch("stable order references an unknown hydration node")
    })?;
    let node = graph.nodes.get(index).ok_or_else(|| {
        remote_evidence_mismatch("stable order references an unknown hydration node")
    })?;
    let evidence = node
        .attributes
        .iter()
        .find(|evidence| evidence.attribute() == term.field().attribute())
        .ok_or_else(|| {
            remote_evidence_mismatch("stable order attribute is absent from hydration evidence")
        })?;
    if evidence.values().len() > 1 {
        return Err(remote_evidence_mismatch(
            "stable order attribute carries non-scalar hydration evidence",
        ));
    }
    Ok(evidence.values().first())
}

fn compare_order_values(
    left: Option<&ModelValue>,
    right: Option<&ModelValue>,
    term: &crate::query_plan::ModelOrderTerm,
) -> Result<Ordering, Diagnostic> {
    let ordering = match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => match term.missing() {
            crate::query_plan::ModelMissingOrder::First => Ordering::Less,
            crate::query_plan::ModelMissingOrder::Last => Ordering::Greater,
            crate::query_plan::ModelMissingOrder::Reject => {
                return Err(remote_evidence_mismatch(
                    "stable order rejects a missing hydration value",
                ));
            }
        },
        (Some(_), None) => match term.missing() {
            crate::query_plan::ModelMissingOrder::First => Ordering::Greater,
            crate::query_plan::ModelMissingOrder::Last => Ordering::Less,
            crate::query_plan::ModelMissingOrder::Reject => {
                return Err(remote_evidence_mismatch(
                    "stable order rejects a missing hydration value",
                ));
            }
        },
        (Some(left), Some(right)) => {
            let ordering = left.semantic_cmp_same_domain(right).ok_or_else(|| {
                remote_evidence_mismatch("stable order values are not semantically comparable")
            })?;
            if term.direction() == crate::query_plan::ModelOrderDirection::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
    };
    Ok(ordering)
}

fn validate_node_projection(
    graph: &HydrationGraph,
    node: &HydrationNode,
    projection: &crate::query_plan::HydrationDescriptor,
    full_output: bool,
) -> Result<(), Diagnostic> {
    if node.attributes.len() != projection.fields().len()
        || (full_output && node.roles.len() != projection.roles().len())
        || (!full_output && !node.roles.is_empty())
    {
        return Err(remote_evidence_mismatch(
            "hydration node contradicts its full-output or shallow-player projection",
        ));
    }
    let fields = projection
        .fields()
        .iter()
        .map(|field| (field.attribute(), field))
        .collect::<BTreeMap<_, _>>();
    for evidence in &node.attributes {
        let Some(field) = fields.get(&evidence.attribute) else {
            return Err(remote_evidence_mismatch(
                "hydration attribute is outside the concrete projection",
            ));
        };
        if !cardinality_allows(field.cardinality(), evidence.values.len())
            || evidence
                .values
                .iter()
                .any(|value| value.value_type() != field.value_type())
        {
            return Err(remote_evidence_mismatch(
                "hydration attribute violates its value type or cardinality",
            ));
        }
        if !field.ordered() {
            for pair in evidence.values.windows(2) {
                let ordering = pair[0].semantic_cmp_same_domain(&pair[1]).ok_or_else(|| {
                    remote_evidence_mismatch(
                        "hydration attribute values are not semantically comparable",
                    )
                })?;
                if ordering != Ordering::Less {
                    return Err(remote_evidence_mismatch(
                        "unordered hydration attribute values must be semantically sorted and unique",
                    ));
                }
            }
        }
        if field.ordered()
            && field.distinct()
            && contains_semantic_duplicate(evidence.values.iter())?
        {
            return Err(remote_evidence_mismatch(
                "distinct ordered hydration attribute contains duplicate values",
            ));
        }
    }
    if !full_output {
        return Ok(());
    }
    let roles = projection
        .roles()
        .iter()
        .map(|role| (role.role(), role))
        .collect::<BTreeMap<_, _>>();
    for evidence in &node.roles {
        let Some(role) = roles.get(&evidence.role) else {
            return Err(remote_evidence_mismatch(
                "hydration role is outside the concrete projection",
            ));
        };
        if !cardinality_allows(role.cardinality(), evidence.players.len()) {
            return Err(remote_evidence_mismatch(
                "hydration role violates its effective cardinality",
            ));
        }
        if !role.ordered()
            && evidence.players.windows(2).any(|pair| {
                (pair[0].node(), pair[0].declared()) > (pair[1].node(), pair[1].declared())
            })
        {
            return Err(remote_evidence_mismatch(
                "unordered hydration role players must be in canonical order",
            ));
        }
        if role.distinct() {
            let distinct_players = evidence
                .players
                .iter()
                .map(HydrationReference::node)
                .collect::<BTreeSet<_>>();
            if distinct_players.len() != evidence.players.len() {
                return Err(remote_evidence_mismatch(
                    "distinct hydration role contains a repeated provider identity",
                ));
            }
        }
        for player in &evidence.players {
            let index = usize::try_from(player.node.get()).map_err(|_| {
                remote_evidence_mismatch("hydration role references an unknown player")
            })?;
            let Some(player_node) = graph.nodes.get(index) else {
                return Err(remote_evidence_mismatch(
                    "hydration role references an unknown player",
                ));
            };
            let Some(player_authority) = role
                .players()
                .iter()
                .find(|authority| authority.declared_descriptor() == &player.declared)
            else {
                return Err(remote_evidence_mismatch(
                    "hydration role player uses an undeclared descriptor",
                ));
            };
            if !player_authority
                .concrete_descriptors()
                .contains(&player_node.concrete)
            {
                return Err(remote_evidence_mismatch(
                    "hydration role player violates descriptor compatibility",
                ));
            }
        }
    }
    Ok(())
}

fn contains_semantic_duplicate<'value>(
    values: impl IntoIterator<Item = &'value ModelValue>,
) -> Result<bool, Diagnostic> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if let Some(first) = values.first().copied() {
        for value in &values[1..] {
            first.semantic_cmp_same_domain(value).ok_or_else(|| {
                remote_evidence_mismatch(
                    "hydration attribute values are not semantically comparable",
                )
            })?;
        }
    }
    let mut incomparable = false;
    values.sort_by(|left, right| match left.semantic_cmp_same_domain(right) {
        Some(ordering) => ordering.then_with(|| left.cmp(right)),
        None => {
            incomparable = true;
            left.cmp(right)
        }
    });
    if incomparable {
        return Err(remote_evidence_mismatch(
            "hydration attribute values are not semantically comparable",
        ));
    }
    Ok(values
        .windows(2)
        .any(|pair| pair[0].semantic_cmp_same_domain(pair[1]) == Some(Ordering::Equal)))
}

fn cardinality_allows(cardinality: crate::value::Cardinality, count: usize) -> bool {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    count >= cardinality.min() && cardinality.max().is_none_or(|maximum| count <= maximum)
}

fn validate_page_contract(
    plan: Option<&QueryPlan>,
    root: BindingId,
    offset: u64,
    limit: u64,
    has_total: bool,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(crate::query_plan::ModelQueryV2::Page {
        root: expected_root,
        window,
        include_total,
        ..
    }) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch(
            "hydrated page has no request-bound page contract",
        ));
    };
    if root != *expected_root
        || offset != window.offset()
        || limit != window.limit()
        || has_total != *include_total
    {
        return Err(remote_evidence_mismatch(
            "hydrated page root, window, or total presence contradicts the request",
        ));
    }
    Ok(())
}

fn validate_slot_shapes(
    rows: &[HydratedRow],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    for row in rows {
        if row.slots.len() != output.slots().len() {
            return Err(remote_evidence_mismatch(
                "hydrated row arity contradicts the model output",
            ));
        }
        for (slot, expected) in row.slots.iter().zip(output.slots()) {
            let shape_matches = match (slot, expected.collection()) {
                (HydrationSlot::Singular { value }, false) => {
                    value.declared() == expected.declared()
                }
                (HydrationSlot::Collection { values }, true) => values
                    .iter()
                    .all(|value| value.declared() == expected.declared()),
                _ => false,
            };
            if !shape_matches {
                return Err(remote_evidence_mismatch(
                    "hydrated slot shape or declared descriptor contradicts the model output",
                ));
            }
            if expected.distinct()
                && matches!(slot, HydrationSlot::Collection { values } if {
                    let unique = values.iter().collect::<BTreeSet<_>>();
                    unique.len() != values.len()
                })
            {
                return Err(remote_evidence_mismatch(
                    "distinct hydrated collection contains duplicate identities",
                ));
            }
        }
    }
    Ok(())
}

fn validate_page_root_distinct(
    entries: &[HydratedRow],
    root: BindingId,
    plan: Option<&QueryPlan>,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(crate::query_plan::ModelQueryV2::Page {
        root: expected_root,
        output,
        ..
    }) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch(
            "hydrated page has no page contract",
        ));
    };
    if root != *expected_root {
        return Err(remote_evidence_mismatch(
            "hydrated page root binding contradicts the request",
        ));
    }
    let Some(root_index) = output
        .slots()
        .iter()
        .position(|slot| slot.binding() == root)
    else {
        return Err(remote_evidence_mismatch(
            "hydrated page root is absent from its output",
        ));
    };
    let mut roots = BTreeSet::new();
    for entry in entries {
        let Some(HydrationSlot::Singular { value }) = entry.slots.get(root_index) else {
            return Err(remote_evidence_mismatch(
                "hydrated page root slot must be singular",
            ));
        };
        if !roots.insert(value.node()) {
            return Err(remote_evidence_mismatch(
                "hydrated page root identities must be distinct",
            ));
        }
    }
    Ok(())
}

fn validate_scalar_root(
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
    root: BindingId,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let expected = match (
        result,
        plan.v2_compatibility()
            .and_then(|compatibility| compatibility.model_query()),
    ) {
        (
            RemoteResultKindV2::DistinctCount,
            Some(crate::query_plan::ModelQueryV2::DistinctCount { root, .. }),
        )
        | (
            RemoteResultKindV2::DistinctExists,
            Some(crate::query_plan::ModelQueryV2::DistinctExists { root, .. }),
        ) => *root,
        _ => {
            return Err(remote_evidence_mismatch(
                "distinct-root scalar contradicts the model-query terminal",
            ));
        }
    };
    if root != expected {
        return Err(remote_evidence_mismatch(
            "distinct-root scalar binds the wrong root",
        ));
    }
    Ok(())
}

fn remote_item_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_item_limit",
        "V2 response exceeds the caller item budget",
    )
}

fn remote_collection_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_collection_member_limit",
        "V2 response exceeds the aggregate collection-member budget",
    )
}

fn remote_graph_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_graph_node_limit",
        "V2 hydration graph exceeds the node budget",
    )
}

fn remote_attribute_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_attribute_value_limit",
        "V2 hydration graph exceeds the attribute-value budget",
    )
}

fn remote_role_player_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_role_player_limit",
        "V2 hydration graph exceeds the role-player budget",
    )
}

fn remote_v2_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static V2 remote diagnostic code is canonical"),
        message,
    )
}
