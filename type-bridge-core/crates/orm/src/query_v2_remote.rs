//! Remote execution of validated query plans over the versioned envelope.
//!
//! Both halves of one plan/result contract live here. The client half
//! encodes a validated invocation into a [`RemoteQueryRequest`] and
//! evidence-validates the returned outcome against the same derived output
//! schema the local executor uses — oversized, replayed, foreign, or
//! mistyped evidence is rejected before any host object is constructed.
//! The server half decodes a request, re-validates the carried plan against
//! its own schema authority (staleness), checks advertised capabilities,
//! tightens caller budgets under its ceilings, executes through the local
//! engine, and encodes the typed outcome. Transport stays out: callers move
//! the envelope bytes however they like.

use std::time::{Duration, Instant};

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::query_plan::{QueryInvocation, QueryOperation, QueryPlanFingerprint};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteFieldValue, RemoteLimits, RemoteOutcome, RemoteQueryFailure,
    RemoteQueryRequest, RemoteQueryResponse, RemoteReply, RemoteRequestFingerprint, RemoteValue,
    decode_remote_reply,
};
use type_bridge_query::{
    DocumentColumnShape, MigrationAssertionValidationContext, OutputSchema, ValidatedQuery,
    validate_query_plan,
};

use crate::query_v2::{
    DocumentFieldValue, QueryResultDocument, QueryResultRow, QueryRowValue, QueryV2Outcome,
    execute_with_provider, failure,
};
use crate::session::backend::BoundedAnswerLimits;

/// Encode one validated invocation into request envelope bytes.
pub fn encode_remote_request(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: RemoteLimits,
    nonce: impl Into<String>,
) -> Result<Vec<u8>, Diagnostic> {
    RemoteQueryRequest::new(validated.plan(), invocation, limits, nonce)?.encode()
}

/// Refuse one invocation the executor's advertisement cannot execute.
///
/// The contract promises clients check required capabilities against the
/// exact advertisement and refuse unsupported plans before any I/O. Both
/// the plan's derived capabilities and the invocation's transport
/// capabilities (multi-row `given` batches) are checked; the executor
/// re-checks the same sets at admission.
pub fn check_advertised_capabilities(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    advertisement: &RemoteCapabilities,
) -> Result<(), Diagnostic> {
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

/// Decode and evidence-validate one response into a typed outcome.
///
/// The caller supplies the exact validated plan, the operation it invoked,
/// the nonce it sent, and its budgets. Every rejection here happens before
/// host object construction.
pub fn decode_remote_outcome(
    bytes: &[u8],
    validated: &ValidatedQuery,
    operation: QueryOperation,
    nonce: &str,
    request: &RemoteRequestFingerprint,
    limits: RemoteLimits,
) -> Result<QueryV2Outcome, Diagnostic> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_bytes {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_response_oversized",
            "response envelope exceeds the caller byte budget",
        ));
    }
    let fingerprint = QueryPlanFingerprint::compute(validated.plan())?;
    let response = match decode_remote_reply(bytes, nonce, &fingerprint, request)? {
        RemoteReply::Response(response) => response,
        // An authenticated failure surfaces its stable server diagnostic
        // instead of collapsing into a generic decode error.
        RemoteReply::Failure(failure) => return Err(failure.diagnostic()?),
    };
    match (operation, response.outcome()) {
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
                                Ok(QueryRowValue::Value {
                                    value: value.clone(),
                                })
                            }
                            RemoteValue::Attribute { type_id, value }
                                if domain.type_ids().contains(type_id)
                                    && domain.value_type() == Some(value.value_type()) =>
                            {
                                Ok(QueryRowValue::Attribute {
                                    type_id: type_id.clone(),
                                    value: value.clone(),
                                })
                            }
                            RemoteValue::Thing { iid, type_id }
                                if domain.type_ids().contains(type_id)
                                    && domain.value_type().is_none()
                                    && !iid.is_empty() =>
                            {
                                Ok(QueryRowValue::Thing {
                                    type_id: type_id.clone(),
                                    iid: iid.clone(),
                                })
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
                            Ok(DocumentFieldValue::Scalar(value.clone()))
                        }
                        (
                            DocumentColumnShape::List { element_type, .. },
                            RemoteFieldValue::List { values },
                        ) if values
                            .iter()
                            .all(|value| value.value_type() == *element_type) =>
                        {
                            Ok(DocumentFieldValue::List(values.clone()))
                        }
                        _ => Err(evidence_mismatch()),
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                validated_documents.push(QueryResultDocument::from_values(values));
            }
            Ok(QueryV2Outcome::Documents(validated_documents))
        }
        (QueryOperation::Count, RemoteOutcome::Count { value }) => {
            Ok(QueryV2Outcome::Count(*value))
        }
        (QueryOperation::Exists, RemoteOutcome::Exists { value }) => {
            Ok(QueryV2Outcome::Exists(*value))
        }
        _ => Err(outcome_mismatch()),
    }
}

/// A remote request proven admissible without any provider resource.
///
/// Constructed only by [`preflight_remote_request`], which has no access
/// to a transaction or provider by signature: decode, capability,
/// schema/staleness, invocation, and budget rejection all happen before
/// any provider resource can exist.
pub struct AdmittedRemoteRequest {
    byte_budget: u64,
    invocation: QueryInvocation,
    limits: BoundedAnswerLimits,
    nonce: String,
    plan_fingerprint: QueryPlanFingerprint,
    request_fingerprint: RemoteRequestFingerprint,
    validated: ValidatedQuery,
}

impl AdmittedRemoteRequest {
    /// Return the caller-supplied nonce for failure envelopes produced
    /// after admission (for example a provider that cannot open).
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
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

    /// Encode the failure envelope carrying this rejection.
    #[must_use]
    pub fn into_failure_envelope(self) -> Vec<u8> {
        match (self.nonce, self.request) {
            (Some(nonce), Some(request)) => {
                RemoteQueryFailure::bound(nonce, &request, &self.diagnostic)
            }
            (nonce, _) => RemoteQueryFailure::new(nonce, &self.diagnostic),
        }
        .encode()
        .unwrap_or_default()
    }
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
    advertised: &CapabilitySet,
    ceilings: BoundedAnswerLimits,
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

    let plan = request.plan().map_err(&fail)?;
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
    let limits = tighten_limits(request.limits(), ceilings);
    let byte_budget = limits.max_bytes;
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).map_err(&fail)?;

    Ok(AdmittedRemoteRequest {
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint,
        request_fingerprint,
        validated,
    })
}

/// Execute one admitted request over a live transaction.
///
/// Always returns envelope bytes: a typed response on success, a
/// structured failure carrying the admitted nonce otherwise.
pub async fn execute_admitted_remote_request(
    admitted: AdmittedRemoteRequest,
    transaction: &mut crate::session::transaction::Transaction,
) -> Vec<u8> {
    let nonce = admitted.nonce.clone();
    let request_fingerprint = admitted.request_fingerprint.clone();
    match run_admitted_request(admitted, transaction).await {
        Ok(response) => response,
        Err((_, diagnostic)) => RemoteQueryFailure::bound(nonce, &request_fingerprint, &diagnostic)
            .encode()
            .unwrap_or_default(),
    }
}

/// Execute one request envelope through the local engine, server-side.
///
/// Preflight runs first ([`preflight_remote_request`]); only an admitted
/// request touches the supplied transaction. Always returns envelope
/// bytes: a typed response on success, a structured failure otherwise.
pub async fn execute_remote_envelope(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertised: &CapabilitySet,
    transaction: &mut crate::session::transaction::Transaction,
    ceilings: BoundedAnswerLimits,
) -> Vec<u8> {
    match preflight_remote_request(request_bytes, context, advertised, ceilings) {
        Ok(admitted) => execute_admitted_remote_request(admitted, transaction).await,
        Err(rejection) => rejection.into_failure_envelope(),
    }
}

async fn run_admitted_request(
    admitted: AdmittedRemoteRequest,
    transaction: &mut crate::session::transaction::Transaction,
) -> Result<Vec<u8>, (Option<String>, Diagnostic)> {
    let AdmittedRemoteRequest {
        byte_budget,
        invocation,
        limits,
        nonce,
        plan_fingerprint,
        request_fingerprint,
        validated,
    } = admitted;
    let fail = |diagnostic: Diagnostic| (Some(nonce.clone()), diagnostic);

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
    let outcome = execute_with_provider(&mut provider, &validated, &invocation, limits)
        .await
        .map_err(|error| {
            fail(match error {
                crate::query_v2::QueryV2ExecutionError::Validation(diagnostic) => diagnostic,
                crate::query_v2::QueryV2ExecutionError::Provider(_) => failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_provider_failed",
                    "the executor provider call failed",
                ),
            })
        })?;

    let response = RemoteQueryResponse::new(
        nonce.clone(),
        &plan_fingerprint,
        &request_fingerprint,
        remote_outcome(&outcome),
    )
    .map_err(&fail)?;
    let bytes = response.encode().map_err(&fail)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > byte_budget {
        return Err(fail(failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_response_oversized",
            "the typed response exceeds the effective byte budget",
        )));
    }
    Ok(bytes)
}

/// Tighten caller budgets under executor ceilings; never raise them.
fn tighten_limits(caller: RemoteLimits, ceilings: BoundedAnswerLimits) -> BoundedAnswerLimits {
    let deadline = caller
        .deadline_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));
    BoundedAnswerLimits {
        max_items: ceilings.max_items.min(caller.max_items),
        max_bytes: ceilings.max_bytes.min(caller.max_bytes),
        deadline: match (ceilings.deadline, deadline) {
            (Some(ceiling), Some(caller)) => Some(ceiling.min(caller)),
            (deadline, None) | (None, deadline) => deadline,
        },
        cancellation: ceilings.cancellation,
    }
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
