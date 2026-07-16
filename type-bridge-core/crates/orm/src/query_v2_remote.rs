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
use type_bridge_contract::query_plan::{
    QueryInvocation, QueryOperation, QueryPlanFingerprint,
};
use type_bridge_contract::query_remote::{
    RemoteFieldValue, RemoteLimits, RemoteOutcome, RemoteQueryFailure,
    RemoteQueryRequest, RemoteQueryResponse, RemoteValue,
};
use type_bridge_query::{
    DocumentColumnShape, MigrationAssertionValidationContext, OutputSchema,
    ValidatedQuery, validate_query_plan,
};

use crate::query_v2::{
    DocumentFieldValue, QueryResultDocument, QueryResultRow, QueryRowValue,
    QueryV2Outcome, execute_with_provider, failure,
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
    let response = RemoteQueryResponse::decode(bytes, nonce, &fingerprint)?;
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
                            RemoteValue::Absent if column.optional() => {
                                Ok(QueryRowValue::Absent)
                            }
                            RemoteValue::Value { value }
                                if domain.type_ids().is_empty()
                                    && domain.value_type()
                                        == Some(value.value_type()) =>
                            {
                                Ok(QueryRowValue::Value { value: value.clone() })
                            }
                            RemoteValue::Attribute { type_id, value }
                                if domain.type_ids().contains(type_id)
                                    && domain.value_type()
                                        == Some(value.value_type()) =>
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
            if u64::try_from(documents.len()).unwrap_or(u64::MAX) > limits.max_items
            {
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

/// Execute one request envelope through the local engine, server-side.
///
/// Always returns envelope bytes: a typed response on success, a structured
/// failure otherwise. The plan re-validates against this server's schema
/// authority, so a stale managed fingerprint fails closed; capabilities the
/// server does not advertise reject before any data I/O; caller budgets
/// tighten under the supplied ceilings and never raise them.
pub async fn execute_remote_envelope(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertised: &CapabilitySet,
    transaction: &mut crate::session::transaction::Transaction,
    ceilings: BoundedAnswerLimits,
) -> Vec<u8> {
    match serve_remote_request(
        request_bytes,
        context,
        advertised,
        transaction,
        ceilings,
    )
    .await
    {
        Ok(response) => response,
        Err((nonce, diagnostic)) => RemoteQueryFailure::new(nonce, &diagnostic)
            .encode()
            .unwrap_or_default(),
    }
}

async fn serve_remote_request(
    request_bytes: &[u8],
    context: &MigrationAssertionValidationContext<'_>,
    advertised: &CapabilitySet,
    transaction: &mut crate::session::transaction::Transaction,
    ceilings: BoundedAnswerLimits,
) -> Result<Vec<u8>, (Option<String>, Diagnostic)> {
    let request = RemoteQueryRequest::decode(request_bytes)
        .map_err(|diagnostic| (None, diagnostic))?;
    let nonce = request.nonce().to_owned();
    let fail = |diagnostic: Diagnostic| (Some(nonce.clone()), diagnostic);

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
    let validated = validate_query_plan(&plan, context, StructuralLimits::CANONICAL)
        .map_err(&fail)?;
    let invocation = request.invocation(&plan).map_err(&fail)?;
    let limits = tighten_limits(request.limits(), ceilings);
    let byte_budget = limits.max_bytes;

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
    let outcome =
        execute_with_provider(&mut provider, &validated, &invocation, limits)
            .await
            .map_err(|error| {
                fail(match error {
                    crate::query_v2::QueryV2ExecutionError::Validation(diagnostic) => {
                        diagnostic
                    }
                    crate::query_v2::QueryV2ExecutionError::Provider(_) => failure(
                        DiagnosticCategory::Integrity,
                        "query_remote_provider_failed",
                        "the executor provider call failed",
                    ),
                })
            })?;

    let fingerprint = QueryPlanFingerprint::compute(&plan).map_err(&fail)?;
    let response = RemoteQueryResponse::new(
        nonce.clone(),
        &fingerprint,
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
fn tighten_limits(
    caller: RemoteLimits,
    ceilings: BoundedAnswerLimits,
) -> BoundedAnswerLimits {
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

fn remote_outcome(outcome: &QueryV2Outcome) -> RemoteOutcome {
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
                .map(|document| {
                    document.values().iter().map(remote_field_value).collect()
                })
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
        QueryRowValue::Value { value } => RemoteValue::Value { value: value.clone() },
        QueryRowValue::Absent => RemoteValue::Absent,
    }
}

fn remote_field_value(value: &DocumentFieldValue) -> RemoteFieldValue {
    match value {
        DocumentFieldValue::Scalar(value) => {
            RemoteFieldValue::Scalar { value: value.clone() }
        }
        DocumentFieldValue::Absent => RemoteFieldValue::Absent,
        DocumentFieldValue::List(values) => {
            RemoteFieldValue::List { values: values.clone() }
        }
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
