//! Remote execution of validated query plans over the versioned envelope.
//!
//! Both halves of one plan/result contract live here. The client half
//! encodes a validated invocation into a [`RemoteQueryRequestV2`] and
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
use type_bridge_contract::limits::REMOTE_ENVELOPE_CODEC_LIMITS;
use type_bridge_contract::query_plan::{QueryInvocation, QueryOperation, QueryPlanFingerprint};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteCapabilitiesFingerprint, RemoteFieldValue, RemoteReplySignature,
    RemoteReplySigner, RemoteReplySigningDigest, RemoteReplyVerifier, RemoteSigningPublicKey,
    RemoteValue,
};
use type_bridge_contract::query_remote_v2::{
    RemoteLimitsV2, RemoteOutcomeV2, RemoteQueryFailureV2, RemoteQueryRequestV2,
    RemoteQueryResponseV2, RemoteReplyDecodeLimitsV2, RemoteReplyV2, RemoteRequestFingerprintV2,
    RemoteResultKindV2, decode_remote_reply_v2, validate_remote_outcome_v2,
};
use type_bridge_query::{
    MigrationAssertionValidationContext, OutputSchema, ValidatedQuery, validate_query_plan,
};

use crate::query_execution_limits::{AnswerTransport, QueryExecutionResourceLimits};
use crate::query_v2::{
    DocumentFieldValue, QueryResultDocument, QueryResultRow, QueryRowValue, QueryV2Outcome,
    QueryV2ValidatedItemObserver, execute_with_provider_observer, failure,
    preflight_invocation_transport,
};
use crate::session::backend::{AnswerLimits, BoundedAnswerLimits};

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
            type_bridge_contract::query_plan::ModelQueryV2::Reduction { .. } => {
                RemoteResultKindV2::ModelReduction
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
            max_statements: 3,
        },
        "capability-check-0000000000000000",
        0,
    )
    .map(|_| ())
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

/// A provider-independent V2 request admitted under the server's exact
/// schema, advertisement epoch, and monotonically tightened budgets.
pub struct AdmittedRemoteRequestV2 {
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    byte_budget: u64,
    invocation: QueryInvocation,
    limits: AnswerLimits,
    max_statements: u8,
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

    /// Snapshot exact reply-correlation state before execution consumes
    /// admission.
    #[must_use]
    pub fn reply_expectation(&self) -> RemoteReplyExpectation {
        RemoteReplyExpectation {
            limits: self.reply_limits,
            plan: self.plan_fingerprint.clone(),
            request: self.request_fingerprint.clone(),
            request_envelope: self.request_envelope.clone(),
        }
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

/// Run every provider-independent check over one additive V2 request.
#[expect(
    clippy::result_large_err,
    reason = "the typed rejection retains bounded correlation state needed to sign the exact V2 failure"
)]
pub fn preflight_remote_request_v2(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertisement: &RemoteCapabilities,
    ceilings: AnswerLimits,
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
    ceilings: AnswerLimits,
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
    ceilings: AnswerLimits,
    clock: PreflightClockSample,
) -> Result<AdmittedRemoteRequestV2, RemoteRejectionV2> {
    // The advertisement is this executor's statement of its limits, so it
    // is also what the executor enforces. Nothing the client sent is trusted.
    let query_limits = advertisement.query_limits();
    let request_envelope = RemoteQueryRequestV2::decode_with_limits(request_bytes, query_limits)
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
    let validated = validate_query_plan(&plan, context, query_limits.structural).map_err(&fail)?;
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

    let advertised = advertised_answer_limits(advertisement).map_err(&fail)?;
    let provider_byte_budget = ceilings.answer.max_bytes.min(advertised.bytes);
    let (limits, reply_limits) = tighten_limits_v2(
        request_envelope.limits(),
        ceilings,
        advertised,
        execution_until,
    );
    let max_statements = u8::try_from(request_envelope.limits().max_statements)
        .expect("validated remote statement ceiling fits u8");
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
        max_statements,
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

/// Exact bindings needed to audit a server-produced reply before release.
pub struct RemoteReplyExpectation {
    /// Authenticated ceilings used while decoding the reply.
    limits: RemoteReplyDecodeLimitsV2,
    /// Fingerprint of the prepared plan selected by the request.
    plan: QueryPlanFingerprint,
    /// Fingerprint of the complete request envelope.
    request: RemoteRequestFingerprintV2,
    /// Exact request retained for correlation and result-shape checks.
    request_envelope: RemoteQueryRequestV2,
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
    /// Authenticate and validate a reply against the exact request.
    pub fn audit(
        &self,
        bytes: &[u8],
        advertisement: &RemoteCapabilitiesFingerprint,
        trusted_key: RemoteSigningPublicKey,
    ) -> Result<AuditedRemoteReply, Diagnostic> {
        match decode_remote_reply_v2(
            bytes,
            &self.request_envelope,
            &self.plan,
            &self.request,
            advertisement,
            trusted_key,
            self.limits,
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
        encode_bound_remote_failure_v2(
            self.request_envelope.nonce(),
            &self.request,
            diagnostic,
            advertisement,
            signer,
        )
    }
}

/// Execute one admitted additive V2 request over exactly one live transaction.
pub async fn execute_admitted_remote_request_v2(
    admitted: AdmittedRemoteRequestV2,
    transaction: &mut crate::session::transaction::OwnedTransaction,
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

async fn run_admitted_request_v2(
    admitted: AdmittedRemoteRequestV2,
    transaction: &mut crate::session::transaction::OwnedTransaction,
    signer: &RemoteReplySigningKey,
) -> Result<Vec<u8>, Diagnostic> {
    let AdmittedRemoteRequestV2 {
        advertisement_fingerprint,
        byte_budget,
        invocation,
        limits,
        max_statements,
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
        | RemoteResultKindV2::DistinctExists
        | RemoteResultKindV2::ModelReduction => None,
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
            | RemoteResultKindV2::ModelReduction
    );
    let outcome = if model_result {
        crate::query_v2::execute_validated_model_query_with_statement_limit(
            transaction,
            &validated,
            &invocation,
            execution_limits,
            reply_limits,
            max_statements,
        )
        .await
        .map_err(execution_diagnostic)?
    } else {
        if max_statements == 0 {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "statement_count_limit",
                "match execution exceeded its statement ceiling",
            ));
        }
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
    let response = RemoteQueryResponseV2::new_with_limits(
        nonce,
        &plan,
        &request_fingerprint,
        result,
        outcome,
        reply_limits,
    )?;
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
        | RemoteResultKindV2::DistinctExists
        | RemoteResultKindV2::ModelReduction => {
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

// The budgets one request runs under: the stricter, per dimension, of what
// the caller asked for, what this executor advertised, and the provider
// ceilings of the hosting process.
fn tighten_limits_v2(
    caller: RemoteLimitsV2,
    ceilings: AnswerLimits,
    advertised: QueryExecutionResourceLimits,
    request_expiry: Instant,
) -> (AnswerLimits, RemoteReplyDecodeLimitsV2) {
    let max_items = ceilings
        .answer
        .max_items
        .min(caller.max_items)
        .min(advertised.items);
    let max_collection_members = ceilings
        .max_collection_members
        .min(caller.max_collection_members)
        .min(advertised.collection_members);
    let max_bytes = ceilings
        .answer
        .max_bytes
        .min(caller.max_bytes)
        .min(advertised.bytes);
    let limits = AnswerLimits {
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
        max_graph_nodes: caller.max_graph_nodes.min(advertised.graph_nodes),
        max_attribute_values: caller.max_attribute_values.min(advertised.attribute_values),
        max_role_players: caller.max_role_players.min(advertised.role_players),
    };
    (limits, reply)
}

/// Read the answer ceilings an executor stated in its advertisement.
pub(crate) fn advertised_answer_limits(
    advertisement: &RemoteCapabilities,
) -> Result<QueryExecutionResourceLimits, Diagnostic> {
    QueryExecutionResourceLimits::from_overrides(
        AnswerTransport::Proxy,
        advertisement
            .answer_limits()
            .iter()
            .map(|(name, value)| (name.as_str(), *value)),
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_advertised_answer_limits",
            "the capability advertisement states answer limits this executor cannot enforce",
        )
    })
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

/// The wire form of a local low-level outcome: what a reply carries for the
/// same result, so a direct and a remote execution can be compared exactly.
#[must_use]
pub fn remote_outcome_v2(outcome: &QueryV2Outcome) -> RemoteOutcomeV2 {
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
    use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES};

    fn unbounded_ceiling() -> AnswerLimits {
        AnswerLimits {
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
    fn remote_v2_limit_tightening_never_widens_transport_or_graph_budgets() {
        let hard_collection =
            u64::try_from(MAX_CANONICAL_COLLECTION_LEN).expect("collection ceiling");
        let hard_bytes = u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling");
        let now = Instant::now();
        let ceiling_deadline = now + Duration::from_millis(250);
        let request_expiry = now + Duration::from_secs(1);
        let ceilings = AnswerLimits {
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
            max_statements: 3,
        };

        let (execution, reply) = tighten_limits_v2(
            caller,
            ceilings,
            QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy),
            request_expiry,
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
            max_statements: 3,
        };
        // A caller that asks for everything gets what the executor
        // advertised: the defaults when it advertised nothing.
        let (execution, reply) = tighten_limits_v2(
            unbounded,
            unbounded_ceiling(),
            QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy),
            request_expiry,
        );
        assert_eq!(execution.answer.max_bytes, hard_bytes);
        assert_eq!(execution.answer.max_items, hard_collection);
        assert_eq!(reply.max_bytes, hard_bytes);
        assert_eq!(reply.max_items, hard_collection);
        assert_eq!(reply.max_graph_nodes, hard_collection);
        assert_eq!(reply.max_attribute_values, hard_collection);
        assert_eq!(reply.max_role_players, hard_collection);

        // An executor that advertised more admits more, per dimension.
        let raised = QueryExecutionResourceLimits {
            items: 1_000_000,
            bytes: 1 << 30,
            graph_nodes: 2_000_000,
            ..QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy)
        };
        let (execution, reply) =
            tighten_limits_v2(unbounded, unbounded_ceiling(), raised, request_expiry);
        assert_eq!(execution.answer.max_items, 1_000_000);
        assert_eq!(execution.answer.max_bytes, 1 << 30);
        assert_eq!(reply.max_graph_nodes, 2_000_000);
        assert_eq!(reply.max_attribute_values, hard_collection);
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
