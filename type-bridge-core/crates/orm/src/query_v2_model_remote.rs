//! One-exchange native staged reads for released model query terminals.

use crate::_registry::DescriptorRegistry;
use crate::match_request::descriptor_plan::MatchRequestAdapterAuthority;
use crate::match_request::plan_adapter::{
    AdaptedMatchRequest, adapt_match_request_under_authority,
};
use crate::match_request::provider_order::check_provider_orders;
use crate::match_request::result_validation::validated_match_result_from_plan_stack;
use crate::match_request::selected_result_executor::released_plan_execution_error;
use crate::match_request::{
    MatchError, MatchErrorCategory, MatchErrorPathSegment, ValidatedMatchRequest,
    ValidatedMatchResult,
};
use crate::query_execution_limits::{
    AnswerTransport, QueryExecutionDeadline, QueryExecutionResourceLimits,
};
use crate::query_plan_exec::PlanAnswer;
use crate::query_v2::failure;
use crate::query_v2_prepared::QueryAuthority;
use crate::session::backend::AnswerCancellation;
use rand_core::{OsRng, RngCore};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{LIMIT_SOURCE_ADVERTISED, MAX_REMOTE_ENVELOPE_BYTES};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_contract::query_remote_v3::{
    StagedKindV3, StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3, StagedRequestV3,
    VerifiedStagedReplyV3,
};

/// A native model-query failure retains its structured error authority.
#[derive(Debug, Error)]
pub enum RemoteModelQueryV2Error {
    /// Transport, authority or canonical-envelope refusal.
    #[error(transparent)]
    Diagnostic(Diagnostic),
    /// Typed match validation or model projection refusal.
    #[error(transparent)]
    Match(#[from] MatchError),
    /// Authenticated server execution failure, preserving provider certainty.
    #[error("{0:?}")]
    Execution(type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic),
}

impl From<Diagnostic> for RemoteModelQueryV2Error {
    fn from(error: Diagnostic) -> Self {
        Self::Diagnostic(typed_transport_diagnostic(error))
    }
}

fn typed_transport_diagnostic(error: Diagnostic) -> Diagnostic {
    let suffix = error
        .code()
        .as_str()
        .strip_prefix("query_remote_v3_")
        .or_else(|| error.code().as_str().strip_prefix("query_remote_v2_"));
    let Some(suffix) = suffix else {
        return error;
    };
    let mut mapped = Diagnostic::new(
        error.category(),
        DiagnosticCode::new(format!("query_remote_{suffix}"))
            .expect("versionless transport diagnostic remains canonical"),
        error.message(),
    )
    .with_path(error.path().clone());
    for (key, value) in error.details() {
        mapped = mapped.with_detail(key.clone(), value.clone());
    }
    mapped
}

struct PreparedModel {
    request: ValidatedMatchRequest,
    stack: AdaptedMatchRequest,
    staged: StagedRequestV3,
}
struct RemoteModelState {
    lifecycle: AtomicU8,
    prepared: Mutex<Option<PreparedModel>>,
    advertisement: RemoteCapabilities,
    registry: Arc<DescriptorRegistry>,
    wanted: QueryExecutionResourceLimits,
    held: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    bytes: Vec<u8>,
}

/// One read-kind staged request and its one-shot reply decoder.
pub struct PendingRemoteModelQueryV2 {
    state: Arc<RemoteModelState>,
}
/// The sole claimed reply capability, reserved before any host byte snapshot.
pub struct ClaimedRemoteModelReplyV2 {
    state: Arc<RemoteModelState>,
    prepared: PreparedModel,
    closed: AtomicBool,
}

impl PendingRemoteModelQueryV2 {
    /// Exact canonical bytes to send to `/v3/staged`.
    pub fn request_bytes(&self) -> &[u8] {
        &self.state.bytes
    }
    /// Close an unclaimed request. Repeated close is harmless.
    pub fn close(&self) {
        if self
            .state
            .lifecycle
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && let Ok(mut prepared) = self.state.prepared.lock()
        {
            prepared.take();
        }
    }
    /// Whether this request was explicitly closed before claim.
    pub fn is_closed(&self) -> bool {
        self.state.lifecycle.load(Ordering::Acquire) == 2
    }
    /// Reserve the only reply capability.
    pub fn claim_reply(&self) -> Result<ClaimedRemoteModelReplyV2, RemoteModelQueryV2Error> {
        self.claim_reply_with_cancellation(&AnswerCancellation::default())
    }
    /// Reserve before semantic cancellation/deadline checks, consuming all outcomes.
    pub fn claim_reply_with_cancellation(
        &self,
        cancellation: &AnswerCancellation,
    ) -> Result<ClaimedRemoteModelReplyV2, RemoteModelQueryV2Error> {
        let mut guard = self.state.prepared.lock().map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_model_claim_state",
                "model request state is unavailable",
            )
        })?;
        self.state
            .lifecycle
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|state| {
                if state == 2 {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "query_resource_closed",
                        "pending model query is closed",
                    )
                } else {
                    failure(
                        DiagnosticCategory::Integrity,
                        "query_remote_reply_replayed",
                        "pending model query already consumed a reply",
                    )
                }
            })?;
        let prepared = guard.take().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_model_claim_state",
                "model request proof was already consumed",
            )
        })?;
        drop(guard);
        check_reply_budget(self.state.deadline, cancellation, true)?;
        Ok(ClaimedRemoteModelReplyV2 {
            state: Arc::clone(&self.state),
            prepared,
            closed: AtomicBool::new(false),
        })
    }
}
impl ClaimedRemoteModelReplyV2 {
    /// Close this claimed capability before decode.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    /// Whether this claimed capability was explicitly closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    /// A signed failure envelope remains admissible when successful output is capped lower.
    pub fn response_snapshot_limit(&self) -> usize {
        MAX_REMOTE_ENVELOPE_BYTES
    }
    /// Verify and publish the complete model result.
    pub fn decode(
        self,
        bytes: &[u8],
    ) -> Result<
        (
            ValidatedMatchRequest,
            ValidatedMatchResult,
            Arc<DescriptorRegistry>,
        ),
        RemoteModelQueryV2Error,
    > {
        self.decode_with_cancellation(bytes, &AnswerCancellation::default())
    }
    /// Verify under the invocation's absolute deadline and cancellation owner.
    pub fn decode_with_cancellation(
        self,
        bytes: &[u8],
        cancellation: &AnswerCancellation,
    ) -> Result<
        (
            ValidatedMatchRequest,
            ValidatedMatchResult,
            Arc<DescriptorRegistry>,
        ),
        RemoteModelQueryV2Error,
    > {
        if self.is_closed() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_resource_closed",
                "claimed model reply is closed",
            )
            .into());
        }
        check_reply_budget(self.state.deadline, cancellation, false)?;
        let verified = VerifiedStagedReplyV3::decode(
            bytes,
            &self.prepared.staged,
            &self.state.advertisement,
            self.state.advertisement.reply_key(),
            &crate::query_v2_remote::Ed25519RemoteReplyVerifier,
        )
        .map_err(|error| annotate_refusal(error, self.state.wanted, self.state.held))?;
        check_reply_budget(self.state.deadline, cancellation, false)?;
        if let Some(diagnostic) = verified.diagnostic() {
            use type_bridge_contract::sdk_diagnostic::{
                SdkDiagnosticDetailValue, SdkDiagnosticName, SdkQueryDiagnosticIdentity,
            };
            let mut diagnostic = diagnostic
                .to_query_diagnostic()
                .map_err(typed_transport_diagnostic)?;
            let limit_name = SdkDiagnosticName::new("limit").expect("static detail");
            if !diagnostic.details().contains_key(&limit_name)
                && let Some((limit, maximum, source)) = self
                    .state
                    .held
                    .refusal_details(AnswerTransport::Proxy, diagnostic.code().as_str())
            {
                let source = if self
                    .state
                    .wanted
                    .get(limit)
                    .is_some_and(|value| value > maximum)
                {
                    LIMIT_SOURCE_ADVERTISED
                } else {
                    source
                };
                for (name, value) in [
                    (
                        "limit",
                        SdkDiagnosticDetailValue::QueryIdentity(
                            SdkQueryDiagnosticIdentity::new(limit).expect("static limit"),
                        ),
                    ),
                    ("maximum", SdkDiagnosticDetailValue::Count(maximum)),
                    (
                        "source",
                        SdkDiagnosticDetailValue::QueryIdentity(
                            SdkQueryDiagnosticIdentity::new(source).expect("static source"),
                        ),
                    ),
                ] {
                    let name = SdkDiagnosticName::new(name).expect("static detail");
                    if !diagnostic.details().contains_key(&name) {
                        diagnostic=diagnostic.try_with_detail(name,value).map_err(|_| RemoteModelQueryV2Error::Execution(type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure()))?;
                    }
                }
            }
            return Err(RemoteModelQueryV2Error::Execution(diagnostic));
        }
        let PreparedModel {
            request,
            mut stack,
            staged,
        } = self.prepared;
        stack
            .plans
            .extend(staged.into_plans().into_iter().map(StagedPlanV3::into_plan));
        let answers = verified
            .answers()
            .iter()
            .zip(&stack.plans)
            .map(|(answer, plan)| PlanAnswer::from_staged_v3(answer, plan))
            .collect::<Result<Vec<_>, _>>()?;
        let result = validated_match_result_from_plan_stack(
            &self.state.registry,
            &request,
            &stack,
            &answers,
            self.state.held,
        )
        .map_err(|error| {
            if error.details().contains_key("limit") {
                return error;
            }
            let Some((limit, maximum, source)) = self
                .state
                .held
                .refusal_details(AnswerTransport::Proxy, error.code().as_str())
            else {
                return error;
            };
            let source = if self
                .state
                .wanted
                .get(limit)
                .is_some_and(|value| value > maximum)
            {
                LIMIT_SOURCE_ADVERTISED
            } else {
                source
            };
            error
                .with_detail("limit", limit)
                .with_detail("maximum", maximum)
                .with_detail("source", source)
        })?;
        check_reply_budget(self.state.deadline, cancellation, false)?;
        Ok((request, result, Arc::clone(&self.state.registry)))
    }
}

fn annotate_refusal(
    error: Diagnostic,
    wanted: QueryExecutionResourceLimits,
    held: QueryExecutionResourceLimits,
) -> Diagnostic {
    if error.category() != DiagnosticCategory::ResourceLimit
        || error.details().contains_key("limit")
    {
        return error;
    }
    let released = if error.code().as_str().starts_with("query_v2_model_") {
        released_plan_execution_error(&error)
            .code()
            .as_str()
            .to_owned()
    } else {
        error.code().as_str().to_owned()
    };
    let Some((limit, maximum, source)) = held.refusal_details(AnswerTransport::Proxy, &released)
    else {
        return error;
    };
    let source = if wanted.get(limit).is_some_and(|value| value > maximum) {
        LIMIT_SOURCE_ADVERTISED
    } else {
        source
    };
    error
        .with_detail("limit", limit)
        .with_detail("maximum", i64::try_from(maximum).unwrap_or(i64::MAX))
        .with_detail("source", source)
}

/// Prepare one model terminal for one read-kind staged exchange.
pub fn prepare_remote_model_query_v2(
    authority: &QueryAuthority,
    registry: &DescriptorRegistry,
    request: ValidatedMatchRequest,
    advertisement: &[u8],
    limits: RemoteLimitsV2,
) -> Result<PendingRemoteModelQueryV2, RemoteModelQueryV2Error> {
    let policy = QueryExecutionResourceLimits::from_remote(limits);
    prepare_remote_model_query_with_policy(
        authority,
        registry,
        request,
        advertisement,
        policy,
        QueryExecutionDeadline::for_limits(policy),
        &AnswerCancellation::default(),
        type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
    )
}
/// Prepare with the caller's already captured deadline and cancellation owner.
pub fn prepare_remote_model_query_v2_with_budget(
    authority: &QueryAuthority,
    registry: &DescriptorRegistry,
    request: ValidatedMatchRequest,
    advertisement: &[u8],
    limits: RemoteLimitsV2,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<PendingRemoteModelQueryV2, RemoteModelQueryV2Error> {
    prepare_remote_model_query_with_policy(
        authority,
        registry,
        request,
        advertisement,
        QueryExecutionResourceLimits::from_remote(limits),
        deadline,
        cancellation,
        type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_remote_model_query_with_policy(
    authority: &QueryAuthority,
    registry: &DescriptorRegistry,
    request: ValidatedMatchRequest,
    advertisement: &[u8],
    policy: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
    admission_window_milliseconds: u64,
) -> Result<PendingRemoteModelQueryV2, RemoteModelQueryV2Error> {
    check_prepare_budget(deadline, cancellation)?;
    let registry = Arc::new(registry.owned_registry_snapshot().map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_registry_snapshot_failed",
            "descriptor registry cannot be fenced for model execution",
        )
    })?);
    request.recheck_schema(&registry)?;
    check_provider_orders(&registry, &request)?;
    let authority_for_adapter = MatchRequestAdapterAuthority::from_query_authority(authority);
    let mut stack =
        adapt_match_request_under_authority(&registry, &request, &authority_for_adapter)?;
    check_prepare_budget(deadline, cancellation)?;
    let advertisement = RemoteCapabilities::decode(advertisement)?;
    if request
        .capabilities()
        .contains(crate::match_request::Capability::SchemaFunctionCall)
        && !advertisement
            .capabilities()
            .contains(&type_bridge_contract::query_plan::query_given_rows_capability())
    {
        return Err(
            crate::match_request::capability::schema_function_given_rows_unsupported().into(),
        );
    }
    let wanted = policy.for_adapter_statements(stack.plans.len());
    let advertised = QueryExecutionResourceLimits::from_overrides(
        AnswerTransport::Proxy,
        advertisement
            .answer_limits()
            .iter()
            .map(|(key, value)| (key.as_str(), *value)),
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_answer_limits",
            "advertised answer limits are invalid",
        )
    })?;
    let held = wanted.constrained_by(advertised);
    if stack.plans.len() > held.statement_limit() as usize {
        return Err(MatchError::new(
            MatchErrorCategory::ResourceLimit,
            "statement_count_limit",
            "model stack exceeds the provider-statement ceiling",
        )
        .with_detail("limit", "statements")
        .with_detail("maximum", u64::from(held.statement_limit()))
        .with_detail(
            "source",
            if held.statement_limit() < wanted.statement_limit() {
                "advertised"
            } else {
                "configured"
            },
        )
        .into());
    }
    let plans = stack
        .plans
        .iter()
        .map(|plan| {
            StagedPlanV3::new(
                plan,
                plan.given_rows().unwrap_or_default().to_vec(),
                authority.declared(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_clock",
            "remote request clock is unavailable",
        )
    })?;
    let staged = StagedRequestV3::new_with_limits(
        StagedRequestSpecV3 {
            nonce: nonce.iter().map(|byte| format!("{byte:02x}")).collect(),
            kind: Some(StagedKindV3::Read),
            limits: StagedLimitsV3 {
                collect_phase_timestamps: held.collect_phase_timestamps,
                admission_window_milliseconds,
                deadline_ms: wire_execution_deadline(
                    deadline.earliest(QueryExecutionDeadline::for_limits(held)),
                ),
                max_bytes: held.bytes,
                max_items: held.items,
                max_graph_nodes: Some(held.graph_nodes),
                max_attribute_values: Some(held.attribute_values),
                max_collection_members: Some(held.collection_members),
                max_role_players: Some(held.role_players),
                max_read_statements: held.statement_limit(),
                ..Default::default()
            },
            plans,
            prepared_at_unix_ms: u64::try_from(now.as_millis()).unwrap_or(u64::MAX),
        },
        authority.declared(),
        &advertisement,
        stack.plans[0].limits(),
    )
    .map_err(|error| {
        let tighter = error
            .details()
            .get("limit")
            .and_then(|value| {
                if let type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(name) = value {
                    Some(name)
                } else {
                    None
                }
            })
            .is_some_and(|name| {
                request
                    .limits()
                    .get(name)
                    .zip(advertisement.query_limits().get(name))
                    .is_some_and(|(wanted, held)| held < wanted)
            });
        if tighter {
            error.with_detail("source", LIMIT_SOURCE_ADVERTISED)
        } else {
            error
        }
    })?;
    let bytes = staged.encode()?;
    stack.plans.clear();
    check_prepare_budget(deadline, cancellation)?;
    Ok(PendingRemoteModelQueryV2 {
        state: Arc::new(RemoteModelState {
            lifecycle: AtomicU8::new(0),
            prepared: Mutex::new(Some(PreparedModel {
                request,
                stack,
                staged,
            })),
            advertisement,
            registry,
            wanted,
            held,
            deadline,
            bytes,
        }),
    })
}

fn check_decode_cancellation(
    cancellation: &AnswerCancellation,
) -> Result<(), RemoteModelQueryV2Error> {
    if cancellation.is_cancelled() {
        Err(MatchError::new(
            MatchErrorCategory::Cancelled,
            "provider_cancelled",
            "remote model execution was cancelled",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into())
    } else {
        Ok(())
    }
}
fn check_prepare_budget(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<(), RemoteModelQueryV2Error> {
    check_decode_cancellation(cancellation)?;
    if deadline.is_expired() {
        return Err(MatchError::new(
            MatchErrorCategory::ResourceLimit,
            "transaction_deadline_exceeded",
            "remote model execution deadline expired",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into());
    }
    Ok(())
}
fn check_reply_budget(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
    claiming: bool,
) -> Result<(), RemoteModelQueryV2Error> {
    if cancellation.is_cancelled() {
        return if claiming {
            Err(failure(
                DiagnosticCategory::Cancelled,
                "provider_cancelled",
                "remote reply claim was cancelled",
            )
            .into())
        } else {
            check_decode_cancellation(cancellation)
        };
    }
    if deadline.is_expired() {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "transaction_deadline_exceeded",
            "remote reply deadline expired",
        )
        .into());
    }
    Ok(())
}

// Expiry after the budget check remains an execution refusal, not malformed wire limits.
fn wire_execution_deadline(deadline: QueryExecutionDeadline) -> Option<u64> {
    deadline
        .remaining_milliseconds()
        .map(|remaining| remaining.max(1))
}

#[cfg(test)]
fn answer_limit_refusal(
    error: Diagnostic,
    wanted: RemoteLimitsV2,
    held: RemoteLimitsV2,
) -> Diagnostic {
    annotate_refusal(
        error,
        QueryExecutionResourceLimits::from_remote(wanted),
        QueryExecutionResourceLimits::from_remote(held),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use type_bridge_contract::capability::CapabilitySet as QueryCapabilitySet;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::query_plan::query_given_rows_capability;
    use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
    use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_contract::value::CanonicalValue;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_compat::released_typeql_to_declared_projection;

    use type_bridge_contract::limits::QueryLimits;

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::_entity::Annotation;
    use crate::_schema::SchemaInfo;
    use crate::match_request::{
        BindingId, BoundFieldId, FetchShape, FetchSlot, FieldId, MatchBinding, MatchErrorCategory,
        MatchErrorPathSegment, MatchMode, MatchOperation, MatchOrder, MatchPlan, MatchRequest,
        MatchResult, MissingOrder, ReduceTerm, ReducedValue, Reduction, RowCardinality,
        SessionHandle, SortDirection, ThingKind, Window, validate_match_request,
    };
    use crate::query_v2_prepared::QueryAuthority;
    use crate::query_v2_remote::RemoteReplySigningKey;
    use crate::{InstalledRuntimeProjection, ProjectedAttributeValue};

    fn native_capabilities() -> QueryCapabilitySet {
        let mut capabilities =
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        for name in [
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
        ] {
            capabilities.insert(type_bridge_contract::capability::CapabilityId::new(name).unwrap());
        }
        capabilities
    }

    fn budget(items: u64) -> RemoteLimitsV2 {
        RemoteLimitsV2 {
            deadline_ms: Some(1000),
            max_items: items,
            max_bytes: 8192,
            max_graph_nodes: 100,
            max_attribute_values: 100,
            max_collection_members: 100,
            max_role_players: 100,
            max_statements: 3,
        }
    }

    fn nullable_order_registry() -> DescriptorRegistry {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    OwnedAttributeDescriptor {
                        field_name: "name".into(),
                        attr_name: "person-name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                        is_optional: false,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeDescriptor {
                        field_name: "ranking".into(),
                        attr_name: "person-ranking".into(),
                        value_type: ValueType::Long,
                        annotations: vec![Annotation::Card(0, Some(1))],
                        is_optional: true,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                doc: None,
                meta: Default::default(),
            })
            .expect("register person");
        registry
    }

    fn matching_authority(registry: &DescriptorRegistry) -> QueryAuthority {
        let schema = SchemaInfo::from_descriptors(&registry.snapshot());
        let source = schema.to_typeql().expect("fixture schema");
        let declared = released_typeql_to_declared_projection(
            DocumentId::new("remote-model-nullable-order.tql").expect("document"),
            &source,
        )
        .expect("released descriptor projection");
        QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared schema bytes"),
            "typebridge-v1-descriptor-registry",
            "typedb-3.12.1/v1",
        )
        .expect("matching query authority")
    }

    fn function_authority() -> (InstalledRuntimeProjection, QueryAuthority) {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("remote-functions.yaml").expect("document"),
            r#"format: typebridge.schema/v2
attributes:
  score: { value: integer }
entities:
  person:
    owns:
      score: { card: 1 }
functions:
  qualifying-score:
    parameters:
      - { name: person, type: person }
      - { name: minimum, type: integer }
    returns: { scalar: integer }
    body: { typeql: "match $person has score $score-attribute; let $score = $score-attribute; $score >= $minimum; return first $score;" }
  identity-score:
    parameters:
      - { name: score, type: integer }
    returns: { scalar: integer }
    body: { typeql: "match $score == $score; return first $score;" }
  person-score:
    parameters:
      - { name: person, type: person }
    returns: { scalar: integer }
    body: { typeql: "match $person has score $score-attribute; let $score = $score-attribute; return first $score;" }
"#,
        )])
        .expect("function schema documents");
        let declared = normalize_documents(&documents).expect("normalized function schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let resolved = resolve(&declared, &profile).expect("resolved function schema");
        let installed = InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .expect("function runtime projection"),
        )
        .expect("installed function projection");
        let authority = QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared function schema bytes"),
            "remote-functions",
            profile.as_str(),
        )
        .expect("function query authority");
        (installed, authority)
    }

    #[test]
    fn remote_model_preparation_preserves_released_nullable_order_error_before_transport() {
        let registry = nullable_order_registry();
        let descriptor = registry.descriptor_id("person").expect("person descriptor");
        let nullable_field = FieldId::new(descriptor.clone(), "ranking");
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: BindingId::new(0),
                    descriptor,
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: vec![MatchOrder {
                    field: BoundFieldId::new(BindingId::new(0), nullable_field.clone()),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                }],
                window: Window {
                    offset: 0,
                    limit: 2,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        );
        let validated = validate_match_request(&registry, request).expect("valid V1 request");
        let authority = matching_authority(&registry);
        let result = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated,
            b"advertisement decoding must not run",
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        );
        let error = match result {
            Err(RemoteModelQueryV2Error::Match(error)) => error,
            Err(error) => panic!("expected released MatchError, got {error:?}"),
            Ok(_) => panic!("nullable V1 ordering must fail before remote preparation"),
        };
        assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
        assert_eq!(error.code().as_str(), "nullable_order_field_unsupported");
        assert_eq!(
            error.message(),
            "the selected provider cannot window by a nullable order field without filtering missing roots"
        );
        assert_eq!(
            error.path().segments(),
            &[MatchErrorPathSegment::Field(nullable_field)]
        );
        assert!(error.details().is_empty());
    }

    fn try_prepare_person_count(
        limits: QueryLimits,
        advertisement_bytes: &[u8],
        compose: impl Fn(
            crate::match_request::QueryHandle,
            crate::match_request::PredicateHandle,
            usize,
        ) -> crate::match_request::QueryHandle,
    ) -> Result<(PendingRemoteModelQueryV2, QueryAuthority), RemoteModelQueryV2Error> {
        let registry = Arc::new(nullable_order_registry());
        let authority = matching_authority(&registry);
        let session = SessionHandle::with_limits(Arc::clone(&registry), limits);
        let root = session.exact("person").expect("person binding");
        let shape = session.positional([root.one()]).expect("shape");
        let name = root.field("name").expect("name field");
        let mut query = session.query(shape).expect("query");
        for index in 0..46 {
            let term = name.compare_value(
                crate::match_request::ComparisonOp::NotEqual,
                crate::AttributeValue::String(format!("excluded-{index}")),
            );
            query = compose(query, term, index);
        }
        let validated = match query.validate_count_by(&root) {
            Ok(validated) => validated,
            Err(crate::OrmError::Match(error)) => return Err(error.into()),
            Err(error) => panic!("unexpected validation failure: {error:?}"),
        };
        prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated,
            advertisement_bytes,
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .map(|pending| (pending, authority))
    }

    fn prepare_person_count(
        limits: QueryLimits,
        compose: impl Fn(
            crate::match_request::QueryHandle,
            crate::match_request::PredicateHandle,
            usize,
        ) -> crate::match_request::QueryHandle,
    ) -> RemoteModelQueryV2Error {
        match try_prepare_person_count(limits, b"not an advertisement", compose) {
            Err(error) => error,
            Ok(_) => panic!("an undecodable advertisement cannot prepare a request"),
        }
    }

    fn contract_diagnostic(error: RemoteModelQueryV2Error) -> Diagnostic {
        match error {
            RemoteModelQueryV2Error::Diagnostic(diagnostic) => diagnostic,
            RemoteModelQueryV2Error::Execution(error) => {
                panic!("unexpected execution error: {error:?}")
            }
            RemoteModelQueryV2Error::Match(error) => {
                panic!("expected a contract diagnostic, got {error:?}")
            }
        }
    }

    /// Alternating disjunction and conjunction is real nesting that no
    /// flattening removes: forty-six terms exceed the native default codec depth.
    fn nested_person_count(limits: QueryLimits) -> RemoteModelQueryV2Error {
        match try_nested_person_count(limits, b"not an advertisement") {
            Err(error) => error,
            Ok(_) => panic!("an undecodable advertisement cannot prepare a request"),
        }
    }

    fn try_nested_person_count(
        limits: QueryLimits,
        advertisement_bytes: &[u8],
    ) -> Result<(PendingRemoteModelQueryV2, QueryAuthority), RemoteModelQueryV2Error> {
        let nested = std::cell::RefCell::new(None::<crate::match_request::PredicateHandle>);
        try_prepare_person_count(limits, advertisement_bytes, |query, term, index| {
            let combined = match nested.borrow().as_ref() {
                None => term,
                Some(previous) if index % 2 == 0 => previous.and(&term).expect("and"),
                Some(previous) => previous.or(&term).expect("or"),
            };
            *nested.borrow_mut() = Some(combined.clone());
            if index == 45 {
                query.where_predicate(combined).expect("attached predicate")
            } else {
                query
            }
        })
    }

    #[test]
    fn remote_model_preparation_adapts_a_wide_where_conjunction() {
        // Forty-six `where` calls once nested past the canonical codec
        // depth. Adaptation now succeeds, so the only failure left is the
        // deliberately undecodable advertisement.
        let diagnostic = contract_diagnostic(prepare_person_count(
            QueryLimits::CANONICAL,
            |query, term, _| query.where_predicate(term).expect("attached predicate"),
        ));
        assert_ne!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
    }

    #[test]
    fn remote_model_preparation_reports_the_limit_that_refused_adaptation() {
        let diagnostic = contract_diagnostic(nested_person_count(QueryLimits::CANONICAL));
        assert_eq!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
        assert_eq!(diagnostic.code().as_str(), "canonical_json_too_deep");
        use type_bridge_contract::diagnostic::DiagnosticDetailValue;
        let detail = |key: &str| diagnostic.details().get(key).cloned();
        assert_eq!(
            detail("maximum_depth"),
            Some(DiagnosticDetailValue::Long(64))
        );
        assert_eq!(
            detail("limit"),
            Some(DiagnosticDetailValue::Text("canonical_depth".into()))
        );
        assert_eq!(detail("maximum"), Some(DiagnosticDetailValue::Long(64)));
        assert_eq!(
            detail("source"),
            Some(DiagnosticDetailValue::Text("default".into()))
        );
    }

    #[test]
    fn a_session_with_raised_depth_prepares_the_plan_default_limits_refuse() {
        let mut raised = QueryLimits::CANONICAL;
        raised.codec.max_depth = type_bridge_contract::limits::MAX_SUPPORTED_CANONICAL_DEPTH;
        let raised = raised.validate().expect("raised limits");
        // Adaptation and encoding pass; only the advertisement is undecodable.
        let diagnostic = contract_diagnostic(nested_person_count(raised));
        assert_ne!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
    }

    #[test]
    fn a_session_with_lowered_limits_refuses_what_default_limits_accept() {
        let mut lowered = QueryLimits::CANONICAL;
        lowered.structural.boolean_terms = 8;
        let lowered = lowered.validate().expect("lowered limits");
        let error = prepare_person_count(lowered, |query, term, _| {
            query.where_predicate(term).expect("attached predicate")
        });
        let RemoteModelQueryV2Error::Match(error) = error else {
            panic!("a lowered structural limit refuses at request validation");
        };
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), "structural_limit_exceeded");
        let detail = |key: &str| error.details().get(key).cloned();
        assert_eq!(
            detail("limit"),
            Some(crate::match_request::MatchErrorDetailValue::Text(
                "boolean_terms".into()
            ))
        );
        assert_eq!(
            detail("maximum"),
            Some(crate::match_request::MatchErrorDetailValue::Unsigned(8))
        );
        assert_eq!(
            detail("source"),
            Some(crate::match_request::MatchErrorDetailValue::Text(
                "configured".into()
            ))
        );
    }

    fn executor_advertisement(limits: QueryLimits) -> RemoteCapabilities {
        let mut capabilities = native_capabilities()
            .iter()
            .cloned()
            .collect::<QueryCapabilitySet>();
        for capability in query_remote_v2_required_capabilities(false) {
            capabilities.insert(capability);
        }
        RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("remote-limits-executor", "epoch-00000000001")
                .expect("executor binding"),
            RemoteReplySigningKey::from_secret_bytes([0x62; 32]).public_key(),
        )
        .with_query_limits(limits)
        .expect("advertised limits")
    }

    fn raised_depth_limits() -> QueryLimits {
        let mut raised = QueryLimits::CANONICAL;
        raised.codec.max_depth = type_bridge_contract::limits::MAX_SUPPORTED_CANONICAL_DEPTH;
        raised.validate().expect("raised limits")
    }

    #[test]
    fn an_executor_admits_what_it_advertises_and_a_default_receiver_refuses_it() {
        let raised = raised_depth_limits();
        let advertisement = executor_advertisement(raised);
        let (pending, authority) = try_nested_person_count(
            raised,
            &advertisement.encode().expect("advertisement bytes"),
        )
        .unwrap_or_else(|error| panic!("matching limits prepare the request: {error:?}"));

        StagedRequestV3::decode_with_limits(
            pending.request_bytes(),
            authority.declared(),
            &advertisement,
            raised,
        )
        .expect("receiver admits advertised depth");
        let refused = match StagedRequestV3::decode(
            pending.request_bytes(),
            authority.declared(),
            &advertisement,
        ) {
            Err(error) => error,
            Ok(_) => panic!("default receiver accepted the deeper plan"),
        };
        assert_eq!(refused.code().as_str(), "canonical_json_too_deep");
    }

    #[test]
    fn a_raised_session_is_refused_before_sending_by_a_default_advertisement() {
        let advertisement = executor_advertisement(QueryLimits::CANONICAL);
        let error = try_nested_person_count(
            raised_depth_limits(),
            &advertisement.encode().expect("advertisement bytes"),
        )
        .err()
        .expect("the advertisement is stricter than the session");
        let diagnostic = contract_diagnostic(error);
        use type_bridge_contract::diagnostic::DiagnosticDetailValue;
        assert_eq!(diagnostic.category(), DiagnosticCategory::ResourceLimit);
        assert_eq!(diagnostic.code().as_str(), "canonical_json_too_deep");
        let detail = |key: &str| diagnostic.details().get(key).cloned();
        assert_eq!(
            detail("limit"),
            Some(DiagnosticDetailValue::Text("canonical_depth".into()))
        );
        assert_eq!(detail("maximum"), Some(DiagnosticDetailValue::Long(64)));
        assert_eq!(
            detail("source"),
            Some(DiagnosticDetailValue::Text("advertised".into()))
        );
    }

    #[test]
    fn remote_schema_functions_use_authenticated_invocation_rows_and_nested_call_order() {
        let (installed, authority) = function_authority();
        let registry = Arc::new(installed.match_registry().expect("function match registry"));
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").expect("person binding");
        let minimum = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "score").expect("score type"),
            CanonicalValue::Long(40),
        )
        .expect("projected minimum");
        let minimum = session.function_value(&minimum).expect("function minimum");
        let inner = session
            .function(&FunctionId::new("qualifying-score").expect("function id"))
            .expect("qualifying function")
            .call([person.function_argument(), minimum.function_argument()])
            .expect("inner call");
        let outer = session
            .function(&FunctionId::new("identity-score").expect("function id"))
            .expect("identity function")
            .call([inner.function_argument()])
            .expect("outer call");
        let request = session
            .query(session.positional([person.one()]).expect("shape"))
            .expect("query")
            .where_predicate(
                outer
                    .compare_field(
                        crate::match_request::ComparisonOp::Equal,
                        &person.field("score").expect("score field"),
                    )
                    .expect("function comparison"),
            )
            .expect("where")
            .exists_by(&person)
            .expect("exists request");
        let validated = validate_match_request(&registry, request).expect("valid request");

        let signer = RemoteReplySigningKey::from_secret_bytes([0x61; 32]);
        let mut capabilities = native_capabilities()
            .iter()
            .filter(|capability| **capability != query_given_rows_capability())
            .cloned()
            .collect::<QueryCapabilitySet>();
        let make_ad = |capabilities| {
            RemoteCapabilities::new(
                capabilities,
                RemoteExecutorBinding::new("remote-functions", "epoch-00000000001").unwrap(),
                signer.public_key(),
            )
        };
        let unsupported = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validate_match_request(&registry, validated.request().clone()).unwrap(),
            &make_ad(capabilities.clone()).encode().unwrap(),
            budget(10),
        )
        .err()
        .expect("given capability required");
        assert!(
            matches!(unsupported, RemoteModelQueryV2Error::Match(error) if error.code().as_str()=="schema_function_given_rows_unsupported")
        );
        capabilities.insert(query_given_rows_capability());
        let advertisement = make_ad(capabilities);
        let pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated,
            &advertisement.encode().unwrap(),
            budget(10),
        )
        .unwrap();
        let staged = StagedRequestV3::decode(
            pending.request_bytes(),
            authority.declared(),
            &advertisement,
        )
        .unwrap();
        assert_eq!(staged.kind(), StagedKindV3::Read);
        assert_eq!(staged.plans().len(), 1);
        let plan = staged.plans()[0].plan();
        assert!(plan.compatibility().is_none());
        assert_eq!(
            plan.given_rows().unwrap()[0].values(),
            &[Some(CanonicalValue::Long(40))]
        );
        let lowered = crate::query_plan_exec::lower(plan).unwrap();
        let text = type_bridge_core_lib::__internal::render(&lowered.query).unwrap();
        assert!(text.find("qualifying-score").unwrap() < text.find("identity-score").unwrap());
        let response = type_bridge_contract::query_remote_v3::StagedReplyV3::new(
            &staged,
            type_bridge_contract::query_remote_v3::StagedOutcomeV3::Committed,
            vec![type_bridge_contract::query_remote_v3::StagedAnswerV3::Exists { value: false }],
        )
        .unwrap()
        .encode_signed(&staged, &advertisement, &signer)
        .unwrap();
        let (request, result, _) = pending.claim_reply().unwrap().decode(&response).unwrap();
        assert!(matches!(
            result.for_request(&request).unwrap(),
            MatchResult::Exists { value: false, .. }
        ));
        assert!(
            matches!(pending.claim_reply(),Err(RemoteModelQueryV2Error::Diagnostic(error)) if error.code().as_str()=="query_remote_reply_replayed")
        );
    }

    #[test]
    fn remote_model_preparation_carries_typed_reduction_contract_and_result_kind() {
        let registry = nullable_order_registry();
        let descriptor = registry.descriptor_id("person").expect("person descriptor");
        let validated = || {
            validate_match_request(
                &registry,
                MatchRequest::v1(
                    MatchPlan {
                        bindings: vec![MatchBinding {
                            id: BindingId::new(0),
                            descriptor: descriptor.clone(),
                            thing_kind: ThingKind::Entity,
                            match_mode: MatchMode::Exact,
                        }],
                        predicate: None,
                        allowed_cross_joins: BTreeSet::new(),
                    },
                    MatchOperation::ReduceBy {
                        root: BindingId::new(0),
                        group: None,
                        reducers: vec![ReduceTerm {
                            reduction: Reduction::Count,
                            input: None,
                        }],
                    },
                ),
            )
            .expect("valid reduce request")
        };
        let authority = matching_authority(&registry);
        let signer = RemoteReplySigningKey::from_secret_bytes([0x53; 32]);
        let mut capabilities = native_capabilities();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("remote-model-reduction", "epoch-00000000001")
                .expect("executor binding"),
            signer.public_key(),
        );
        let expired = prepare_remote_model_query_v2_with_budget(
            &authority,
            &registry,
            validated(),
            b"expired preparation must not decode an advertisement",
            RemoteLimitsV2 {
                deadline_ms: Some(0),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
            QueryExecutionDeadline::expired(),
            &AnswerCancellation::default(),
        );
        let RemoteModelQueryV2Error::Match(expired) = expired
            .err()
            .expect("zero-timeout preparation must not publish a pending request")
        else {
            panic!("preparation timeout must retain MatchError authority")
        };
        assert_eq!(expired.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(expired.code().as_str(), "transaction_deadline_exceeded");
        assert_eq!(
            expired.path().segments(),
            &[MatchErrorPathSegment::ProviderEvidence]
        );

        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let cancelled_pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated(),
            &advertisement.encode().expect("advertisement bytes"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("cancelled typed reduction remote preparation");
        let claim_error = match cancelled_pending.claim_reply_with_cancellation(&cancellation) {
            Err(error) => error,
            Ok(_) => panic!("pre-cancelled claim must not publish a claimed reply"),
        };
        let RemoteModelQueryV2Error::Diagnostic(claim_error) = claim_error else {
            panic!("claim cancellation must retain remote Diagnostic authority")
        };
        assert_eq!(claim_error.category(), DiagnosticCategory::Cancelled);
        assert_eq!(claim_error.code().as_str(), "provider_cancelled");
        let replay = match cancelled_pending.claim_reply() {
            Err(error) => error,
            Ok(_) => panic!("cancelled semantic claim must consume the one-shot request"),
        };
        let RemoteModelQueryV2Error::Diagnostic(replay) = replay else {
            panic!("claim replay must retain remote Diagnostic authority")
        };
        assert_eq!(replay.category(), DiagnosticCategory::Integrity);
        assert_eq!(replay.code().as_str(), "query_remote_reply_replayed");

        let expired_pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated(),
            &advertisement.encode().expect("advertisement bytes"),
            RemoteLimitsV2 {
                deadline_ms: Some(100),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("expiring typed reduction remote preparation");
        std::thread::sleep(std::time::Duration::from_millis(125));
        let expired = match expired_pending.claim_reply() {
            Err(error) => error,
            Ok(_) => panic!("expired semantic claim must not publish a claimed reply"),
        };
        let RemoteModelQueryV2Error::Diagnostic(expired) = expired else {
            panic!("claim deadline must retain remote Diagnostic authority")
        };
        assert_eq!(expired.category(), DiagnosticCategory::ResourceLimit);
        assert_eq!(expired.code().as_str(), "transaction_deadline_exceeded");
        let replay = match expired_pending.claim_reply() {
            Err(error) => error,
            Ok(_) => panic!("expired semantic claim must consume the one-shot request"),
        };
        let RemoteModelQueryV2Error::Diagnostic(replay) = replay else {
            panic!("expired claim replay must retain remote Diagnostic authority")
        };
        assert_eq!(replay.category(), DiagnosticCategory::Integrity);
        assert_eq!(replay.code().as_str(), "query_remote_reply_replayed");

        let closed_pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated(),
            &advertisement.encode().expect("advertisement bytes"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("closable typed reduction remote preparation");
        assert!(!closed_pending.is_closed());
        closed_pending.close();
        closed_pending.close();
        assert!(closed_pending.is_closed());
        for _ in 0..2 {
            let closed = match closed_pending.claim_reply() {
                Err(error) => error,
                Ok(_) => panic!("closed pending request must reject claim"),
            };
            let RemoteModelQueryV2Error::Diagnostic(closed) = closed else {
                panic!("closed request must retain remote Diagnostic authority")
            };
            assert_eq!(closed.category(), DiagnosticCategory::InvalidContract);
            assert_eq!(closed.code().as_str(), "query_resource_closed");
        }

        let closed_claim_pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated(),
            &advertisement.encode().expect("advertisement bytes"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("claim-close typed reduction remote preparation");
        let closed_claim = closed_claim_pending.claim_reply().expect("claim");
        closed_claim.close();
        closed_claim.close();
        assert!(closed_claim.is_closed());
        let RemoteModelQueryV2Error::Diagnostic(closed_claim_error) = closed_claim
            .decode(b"must not be inspected")
            .expect_err("closed claim must reject before response decode")
        else {
            panic!("closed claim must retain remote Diagnostic authority")
        };
        assert_eq!(
            closed_claim_error.category(),
            DiagnosticCategory::InvalidContract
        );
        assert_eq!(closed_claim_error.code().as_str(), "query_resource_closed");

        let pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated(),
            &advertisement.encode().expect("advertisement bytes"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 4_096,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("typed reduction remote preparation");
        let envelope = StagedRequestV3::decode(
            pending.request_bytes(),
            authority.declared(),
            &advertisement,
        )
        .unwrap();
        assert_eq!(envelope.kind(), StagedKindV3::Read);
        assert_eq!(envelope.plans().len(), 1);
        let response = type_bridge_contract::query_remote_v3::StagedReplyV3::new(
            &envelope,
            type_bridge_contract::query_remote_v3::StagedOutcomeV3::Committed,
            vec![
                type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows {
                    rows: vec![vec![
                        type_bridge_contract::query_remote::RemoteValue::Value {
                            value: CanonicalValue::Long(7),
                        },
                    ]],
                },
            ],
        )
        .unwrap()
        .encode_signed(&envelope, &advertisement, &signer)
        .unwrap();
        let (request, result, _) = pending
            .claim_reply()
            .expect("single reduction claim")
            .decode(&response)
            .expect("authenticated reduction decode");
        let MatchResult::Reduction { rows, .. } = result
            .for_request(&request)
            .expect("reply remains invocation-bound")
        else {
            panic!("decoded result is not a released reduction")
        };
        assert!(matches!(
            rows.as_slice(),
            [row] if row.group().is_none()
                && row.values() == [ReducedValue::Count(7)]
        ));
        pending.close();
        assert!(!pending.is_closed(), "claim remains distinct from close");
        let replay = match pending.claim_reply() {
            Err(error) => error,
            Ok(_) => panic!("close after claim must retain replay semantics"),
        };
        let RemoteModelQueryV2Error::Diagnostic(replay) = replay else {
            panic!("claim replay must retain remote Diagnostic authority")
        };
        assert_eq!(replay.category(), DiagnosticCategory::Integrity);
        assert_eq!(replay.code().as_str(), "query_remote_reply_replayed");
    }
}

#[cfg(test)]
mod answer_limit_refusal_tests {
    use type_bridge_contract::diagnostic::DiagnosticDetailValue;

    use super::*;

    fn budget(items: u64) -> RemoteLimitsV2 {
        RemoteLimitsV2 {
            max_items: items,
            ..QueryExecutionResourceLimits::default().remote()
        }
    }

    fn refusal(code: &'static str) -> Diagnostic {
        failure(DiagnosticCategory::ResourceLimit, code, "refused")
    }

    fn details(diagnostic: &Diagnostic) -> (Option<String>, Option<i64>, Option<String>) {
        let text = |key: &str| match diagnostic.details().get(key) {
            Some(DiagnosticDetailValue::Text(value)) => Some(value.clone()),
            _ => None,
        };
        let long = match diagnostic.details().get("maximum") {
            Some(DiagnosticDetailValue::Long(value)) => Some(*value),
            _ => None,
        };
        (text("limit"), long, text("source"))
    }

    #[test]
    fn an_executor_refusal_names_its_limit_and_where_the_ceiling_came_from() {
        // The executor's ceiling was the stricter one.
        let advertised = answer_limit_refusal(
            refusal("processed_item_limit"),
            budget(1_000_000),
            budget(65_536),
        );
        assert_eq!(
            details(&advertised),
            (
                Some("items".into()),
                Some(65_536),
                Some("advertised".into())
            )
        );
        // The caller's own configured budget was.
        let configured =
            answer_limit_refusal(refusal("query_v2_model_item_limit"), budget(10), budget(10));
        assert_eq!(
            details(&configured),
            (Some("items".into()), Some(10), Some("configured".into()))
        );
        // Neither side configured anything.
        let default = answer_limit_refusal(
            refusal("solution_scan_limit"),
            budget(65_536),
            budget(65_536),
        );
        assert_eq!(
            details(&default),
            (Some("items".into()), Some(65_536), Some("default".into()))
        );
        // A refusal that reports no answer dimension is left alone.
        let other = answer_limit_refusal(refusal("row_window_exceeded"), budget(1), budget(1));
        assert!(other.details().is_empty());
    }
    #[test]
    fn wire_deadline_race_keeps_expiry_an_execution_error() {
        let expired = QueryExecutionDeadline::expired();
        assert_eq!(wire_execution_deadline(expired), Some(1));
        assert_eq!(
            expired
                .check(&AnswerCancellation::default())
                .unwrap_err()
                .code()
                .as_str(),
            "transaction_deadline_exceeded"
        );
        assert_eq!(
            wire_execution_deadline(QueryExecutionDeadline::from_timeout(
                crate::QueryTimeout::NONE
            )),
            None
        );
    }
}
