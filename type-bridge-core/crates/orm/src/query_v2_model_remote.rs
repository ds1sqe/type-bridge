//! Native one-exchange remote execution for released model-oriented queries.
//!
//! This boundary adapts one already validated released [`MatchRequest`] onto
//! the additive V2 model-query contract, prepares the ordinary authenticated
//! envelope, and converts the fully validated hydration outcome back into the
//! same opaque [`ValidatedMatchResult`] proof consumed by direct execution.
//! No serialized outcome or host-owned model value crosses this boundary.

use std::sync::{Arc, Mutex};

use thiserror::Error;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::query_plan::{QueryInvocation, query_given_rows_capability};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;

use crate::_registry::DescriptorRegistry;
use crate::match_request::provider_order::check_provider_orders;
use crate::match_request::result_validation::validated_match_result_from_v2;
use crate::match_request::{
    Capability, MatchError, MatchErrorCategory, MatchErrorPathSegment, ValidatedMatchRequest,
    ValidatedMatchResult,
};
use crate::query_execution_limits::QueryExecutionDeadline;
use crate::query_v2::failure;
use crate::query_v2_adapter::adapt_match_request;
use crate::query_v2_prepared::{
    ClaimedRemoteReplyV2, PendingRemoteQueryV2, QueryAuthority,
    prepare_validated_remote_query_v2_with_budget,
};
use crate::session::backend::AnswerCancellation;

/// A model-query remote failure retains its native structured error family.
#[derive(Debug, Error)]
pub enum RemoteModelQueryV2Error {
    /// Additive V2 contract, authority, capability, or envelope failure.
    #[error(transparent)]
    Diagnostic(#[from] Diagnostic),
    /// Released match-result validation or hydration-projection failure.
    #[error(transparent)]
    Match(#[from] MatchError),
}

/// One adapted model request and its request-bound one-shot decoder.
pub struct PendingRemoteModelQueryV2 {
    pending: PendingRemoteQueryV2,
    registry: Arc<DescriptorRegistry>,
    request: Mutex<Option<ValidatedMatchRequest>>,
}

/// The sole claimed reply slot for one adapted model request.
pub struct ClaimedRemoteModelReplyV2 {
    claimed: ClaimedRemoteReplyV2,
    registry: Arc<DescriptorRegistry>,
    request: ValidatedMatchRequest,
}

impl PendingRemoteModelQueryV2 {
    /// Borrow the exact canonical request bytes for the caller-owned exchange.
    #[must_use]
    pub fn request_bytes(&self) -> &[u8] {
        self.pending.request_bytes()
    }

    /// Close this pending request before its reply slot is claimed.
    ///
    /// Closing is idempotent and does not change an already claimed request
    /// back into a closable resource.
    pub fn close(&self) {
        self.pending.close();
    }

    /// Whether this request was explicitly closed before claim.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.pending.is_closed()
    }

    /// Atomically reserve the sole reply before binding code snapshots bytes.
    pub fn claim_reply(&self) -> Result<ClaimedRemoteModelReplyV2, RemoteModelQueryV2Error> {
        self.claim_reply_with_cancellation(&AnswerCancellation::default())
    }

    /// Reserve the sole reply after checking the cancellation owner shared
    /// with preparation, caller transport, decode, and materialization.
    #[doc(hidden)]
    pub fn claim_reply_with_cancellation(
        &self,
        cancellation: &AnswerCancellation,
    ) -> Result<ClaimedRemoteModelReplyV2, RemoteModelQueryV2Error> {
        // Serialize the model proof with the lower one-shot reservation. This
        // ensures a cancellation/deadline failure after reservation consumes
        // both layers, and prevents a losing concurrent claimant from taking
        // the winner's request proof.
        let mut request_guard = self.request.lock().map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_model_claim_state",
                "remote model-query claim state is unavailable",
            )
        })?;
        let claimed = self.pending.claim_reply_with_cancellation(cancellation);
        let request = request_guard.take();
        drop(request_guard);
        let claimed = claimed?;
        let request = request.ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_model_claim_state",
                "remote model-query request proof was already consumed",
            )
        })?;
        Ok(ClaimedRemoteModelReplyV2 {
            claimed,
            registry: Arc::clone(&self.registry),
            request,
        })
    }
}

impl ClaimedRemoteModelReplyV2 {
    /// Explicitly release this claimed reply capability before decode.
    pub fn close(&self) {
        self.claimed.close();
    }

    /// Whether this claimed reply capability was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.claimed.is_closed()
    }

    /// Maximum immutable response snapshot admitted by the native decoder.
    #[must_use]
    pub fn response_snapshot_limit(&self) -> usize {
        self.claimed.response_snapshot_limit()
    }

    /// Authenticate and validate one reply, then construct the ordinary
    /// released match-result proof without a host JSON round trip.
    pub fn decode(
        self,
        response_bytes: &[u8],
    ) -> Result<
        (
            ValidatedMatchRequest,
            ValidatedMatchResult,
            Arc<DescriptorRegistry>,
        ),
        RemoteModelQueryV2Error,
    > {
        self.decode_with_cancellation(response_bytes, &AnswerCancellation::default())
    }

    /// Authenticate and reconstruct one reply with cooperative cancellation
    /// checkpoints around each bounded native decode phase.
    #[doc(hidden)]
    pub fn decode_with_cancellation(
        self,
        response_bytes: &[u8],
        cancellation: &AnswerCancellation,
    ) -> Result<
        (
            ValidatedMatchRequest,
            ValidatedMatchResult,
            Arc<DescriptorRegistry>,
        ),
        RemoteModelQueryV2Error,
    > {
        check_decode_cancellation(cancellation)?;
        let Self {
            claimed,
            registry,
            request,
        } = self;
        let outcome = claimed.decode_outcome(response_bytes)?;
        check_decode_cancellation(cancellation)?;
        let result = validated_match_result_from_v2(&registry, &request, outcome)?;
        check_decode_cancellation(cancellation)?;
        Ok((request, result, registry))
    }
}

fn check_decode_cancellation(
    cancellation: &AnswerCancellation,
) -> Result<(), RemoteModelQueryV2Error> {
    if cancellation.is_cancelled() {
        return Err(MatchError::new(
            MatchErrorCategory::Cancelled,
            "provider_cancelled",
            "remote result reconstruction was cancelled",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into());
    }
    Ok(())
}

/// Adapt and prepare one released model-oriented terminal for remote V2
/// execution.
///
/// The descriptor registry is copied into an independently owned snapshot
/// before adaptation. Later model registrations therefore cannot alter the
/// schema authority against which the reply is decoded.
pub fn prepare_remote_model_query_v2(
    authority: &QueryAuthority,
    registry: &DescriptorRegistry,
    request: ValidatedMatchRequest,
    advertisement_bytes: &[u8],
    limits: RemoteLimitsV2,
) -> Result<PendingRemoteModelQueryV2, RemoteModelQueryV2Error> {
    let deadline = QueryExecutionDeadline::from_timeout_milliseconds(
        limits
            .deadline_ms
            .unwrap_or(type_bridge_contract::query_remote::DEFAULT_REMOTE_DEADLINE_MS),
    );
    let cancellation = AnswerCancellation::default();
    prepare_remote_model_query_v2_with_budget(
        authority,
        registry,
        request,
        advertisement_bytes,
        limits,
        deadline,
        &cancellation,
    )
}

/// Prepare one released model terminal using the absolute deadline captured at
/// public terminal entry and the cancellation owner retained by the caller.
#[doc(hidden)]
pub fn prepare_remote_model_query_v2_with_budget(
    authority: &QueryAuthority,
    registry: &DescriptorRegistry,
    request: ValidatedMatchRequest,
    advertisement_bytes: &[u8],
    limits: RemoteLimitsV2,
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<PendingRemoteModelQueryV2, RemoteModelQueryV2Error> {
    check_prepare_budget(deadline, cancellation)?;
    let registry = Arc::new(registry.owned_registry_snapshot().map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_registry_snapshot_failed",
            "descriptor registry could not be snapshotted for remote execution",
        )
    })?);
    check_prepare_budget(deadline, cancellation)?;
    request.recheck_schema(&registry).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_registry_snapshot_mismatch",
            "validated model query does not belong to the fenced registry snapshot",
        )
    })?;
    check_prepare_budget(deadline, cancellation)?;
    check_provider_orders(&registry, &request)?;
    check_prepare_budget(deadline, cancellation)?;

    // A request that does not fit the session's limits is refused here,
    // before anything is sent.
    let adapted = adapt_match_request(&request, &registry, &authority.context())?;
    check_prepare_budget(deadline, cancellation)?;
    let invocation = QueryInvocation::new(
        adapted.validated().plan(),
        adapted.operation(),
        adapted.inputs().to_vec(),
    )?;
    if request
        .capabilities()
        .contains(Capability::SchemaFunctionCall)
        && !RemoteCapabilities::decode(advertisement_bytes)?
            .capabilities()
            .contains(&query_given_rows_capability())
    {
        return Err(
            crate::match_request::capability::schema_function_given_rows_unsupported().into(),
        );
    }
    check_prepare_budget(deadline, cancellation)?;
    let pending = prepare_validated_remote_query_v2_with_budget(
        authority,
        adapted.validated().clone(),
        invocation,
        advertisement_bytes,
        limits,
        deadline,
        cancellation,
    )?;
    Ok(PendingRemoteModelQueryV2 {
        pending,
        registry,
        request: Mutex::new(Some(request)),
    })
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
            "remote query preparation deadline expired",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use type_bridge_contract::capability::CapabilitySet as QueryCapabilitySet;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::BindingId as QueryBindingId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::query_plan::{
        QueryPattern, ReadStage, query_given_rows_capability, query_plan_v2_capability_vocabulary,
    };
    use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
    use type_bridge_contract::query_remote_v2::{
        HydrationGraphV2, RemoteOutcomeV2, RemoteQueryRequestV2, RemoteQueryResponseV2,
        RemoteReducedValueV2, RemoteReductionRowV2, RemoteResultKindV2,
        query_remote_v2_required_capabilities,
    };
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_contract::value::CanonicalValue;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_compat::released_typeql_to_declared_projection;

    use type_bridge_contract::limits::{QueryLimits, StructuralLimits};

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::_entity::Annotation;
    use crate::_schema::{SchemaInfo, generator::generate_define_block};
    use crate::match_request::{
        BindingId, BoundFieldId, FetchShape, FetchSlot, FieldId, MatchBinding, MatchErrorCategory,
        MatchErrorPathSegment, MatchMode, MatchOperation, MatchOrder, MatchPlan, MatchRequest,
        MatchResult, MissingOrder, ReduceTerm, ReducedValue, Reduction, RowCardinality,
        SessionHandle, SortDirection, ThingKind, Window, validate_match_request,
    };
    use crate::query_v2_prepared::QueryAuthority;
    use crate::query_v2_remote::RemoteReplySigningKey;
    use crate::{InstalledRuntimeProjection, ProjectedAttributeValue};

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
        let source = generate_define_block(&schema);
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
        for index in 0..38 {
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
            RemoteModelQueryV2Error::Match(error) => {
                panic!("expected a contract diagnostic, got {error:?}")
            }
        }
    }

    /// Alternating disjunction and conjunction is real nesting that no
    /// flattening removes: thirty-eight terms nest thirty-seven levels deep.
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
            if index == 37 {
                query.where_predicate(combined).expect("attached predicate")
            } else {
                query
            }
        })
    }

    #[test]
    fn remote_model_preparation_adapts_a_wide_where_conjunction() {
        // Thirty-eight `where` calls once nested past the canonical codec
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
        let mut capabilities = query_plan_v2_capability_vocabulary()
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

        // The executor decodes and validates under its own advertisement.
        crate::query_v2_remote::preflight_remote_request_v2(
            pending.request_bytes(),
            &authority.context(),
            &advertisement,
            crate::session::backend::QueryV2AnswerLimits {
                answer: crate::session::backend::BoundedAnswerLimits {
                    max_items: 100,
                    max_bytes: 1 << 20,
                    deadline: None,
                    cancellation: AnswerCancellation::default(),
                },
                max_collection_members: 1 << 16,
            },
        )
        .unwrap_or_else(|_| panic!("the executor admits a plan within its advertised limits"));

        // The same bytes are beyond a receiver holding the default limits,
        // whatever the sender was configured with.
        let refused = type_bridge_contract::query_remote_v2::RemoteQueryRequestV2::decode(
            pending.request_bytes(),
        )
        .expect_err("a default receiver refuses the deeper request");
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
        let mut capabilities = query_plan_v2_capability_vocabulary()
            .iter()
            .filter(|capability| capability.as_str() != "query.input.given-rows")
            .cloned()
            .collect::<QueryCapabilitySet>();
        for capability in query_remote_v2_required_capabilities(false) {
            capabilities.insert(capability);
        }
        let unsupported_advertisement = RemoteCapabilities::new(
            capabilities.clone(),
            RemoteExecutorBinding::new("remote-functions", "epoch-00000000001")
                .expect("executor binding"),
            signer.public_key(),
        );
        let unsupported = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validate_match_request(&registry, validated.request().clone())
                .expect("independent unsupported-capability request"),
            &unsupported_advertisement
                .encode()
                .expect("unsupported advertisement"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 8_192,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        );
        let RemoteModelQueryV2Error::Match(unsupported) = unsupported
            .err()
            .expect("missing given capability must fail before request publication")
        else {
            panic!("remote function capability failure must stay structured")
        };
        assert_eq!(
            unsupported.category(),
            MatchErrorCategory::UnsupportedCapability
        );
        assert_eq!(
            unsupported.code().as_str(),
            "schema_function_given_rows_unsupported"
        );
        assert_eq!(
            unsupported.path().segments(),
            &[MatchErrorPathSegment::Operation]
        );

        let model_only_call = session
            .function(&FunctionId::new("person-score").expect("function id"))
            .expect("model-only function")
            .call([person.function_argument()])
            .expect("model-only call");
        let model_only_request = session
            .query(session.positional([person.one()]).expect("shape"))
            .expect("query")
            .where_predicate(
                model_only_call
                    .compare_field(
                        crate::match_request::ComparisonOp::Equal,
                        &person.field("score").expect("score field"),
                    )
                    .expect("model-only comparison"),
            )
            .expect("where")
            .exists_by(&person)
            .expect("exists request");
        let model_only_validated =
            validate_match_request(&registry, model_only_request).expect("valid model-only call");
        let model_only_unsupported = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validate_match_request(&registry, model_only_validated.request().clone())
                .expect("independent model-only request"),
            &unsupported_advertisement
                .encode()
                .expect("unsupported advertisement"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 8_192,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        );
        let RemoteModelQueryV2Error::Match(model_only_unsupported) = model_only_unsupported
            .err()
            .expect("request-level given capability applies to model-only calls")
        else {
            panic!("model-only function capability failure must stay structured")
        };
        assert_eq!(
            model_only_unsupported.code().as_str(),
            "schema_function_given_rows_unsupported"
        );

        capabilities.insert(query_given_rows_capability());
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("remote-functions", "epoch-00000000001")
                .expect("executor binding"),
            signer.public_key(),
        );
        let model_only_pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            model_only_validated,
            &advertisement.encode().expect("advertisement"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 8_192,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("given-capable remote admits a model-only function graph");
        let model_only_envelope = RemoteQueryRequestV2::decode(model_only_pending.request_bytes())
            .expect("model-only envelope");
        let model_only_plan = model_only_envelope.plan().expect("model-only plan");
        assert!(
            model_only_plan
                .required_capabilities()
                .contains(&query_given_rows_capability()),
            "request capability remains given-bound without invocation cells"
        );
        let model_only_invocation = model_only_envelope
            .invocation(&model_only_plan)
            .expect("model-only invocation");
        assert!(model_only_invocation.inputs().is_empty());
        let model_only_validated_plan = type_bridge_query::validate_query_plan(
            &model_only_plan,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .expect("model-only plan revalidates");
        let model_only_lowered =
            crate::query_v2_compatibility::lower_validated_compatibility_query(
                &model_only_validated_plan,
                &model_only_invocation,
                model_only_invocation.operation(),
            )
            .expect("model-only remote lowering")
            .expect("model-only compatibility program");
        assert!(model_only_lowered.typeql().starts_with("match\n"));
        assert!(
            model_only_lowered
                .typeql()
                .contains("let $c0 = person-score($b0)")
        );
        let pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            validated,
            &advertisement.encode().expect("advertisement"),
            RemoteLimitsV2 {
                deadline_ms: Some(1_000),
                max_bytes: 8_192,
                max_items: 10,
                max_collection_members: 10,
                max_graph_nodes: 10,
                max_attribute_values: 10,
                max_role_players: 10,
                max_statements: 3,
            },
        )
        .expect("remote function preparation");
        let first_bytes = pending.request_bytes().to_vec();
        let envelope = RemoteQueryRequestV2::decode(&first_bytes).expect("remote request envelope");
        assert_eq!(envelope.result_kind(), RemoteResultKindV2::DistinctExists);
        let plan = envelope.plan().expect("function plan");
        assert!(
            plan.required_capabilities()
                .contains(&query_given_rows_capability())
        );
        let invocation = envelope.invocation(&plan).expect("function invocation");
        assert_eq!(
            invocation.inputs()[0].values(),
            [Some(CanonicalValue::Long(40))]
        );
        let ReadStage::Match { patterns } = &plan.pipeline()[0] else {
            panic!("function plan starts with match")
        };
        let functions = patterns
            .iter()
            .filter_map(|pattern| match pattern {
                QueryPattern::FunctionCall { function, .. } => Some(function.label().as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(functions, ["qualifying-score", "identity-score"]);
        let validated_plan = type_bridge_query::validate_query_plan(
            &plan,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .expect("decoded function plan revalidates against its exact authority");
        let lowered = crate::query_v2_compatibility::lower_validated_compatibility_query(
            &validated_plan,
            &invocation,
            invocation.operation(),
        )
        .expect("remote function plan lowers through the provider compatibility compiler")
        .expect("adapted model plan has a compatibility program");
        assert!(lowered.typeql().starts_with("given $g0: integer;\nmatch\n"));
        let qualifying = lowered
            .typeql()
            .find("let $c0 = qualifying-score($b0, $g0)")
            .expect("inner function call");
        let identity = lowered
            .typeql()
            .find("let $c1 = identity-score($c0)")
            .expect("outer function call");
        assert!(qualifying < identity, "nested calls lower dependency-first");

        // The exact same request is deterministic, while changing only the
        // invocation value changes authenticated request bytes but not plan identity.
        let rebuilt = |threshold| {
            let registry = Arc::new(
                installed
                    .match_registry()
                    .expect("registry")
                    .owned_registry_snapshot()
                    .expect("snapshot"),
            );
            let session = SessionHandle::new(Arc::clone(&registry));
            let person = session.exact("person").expect("person");
            let minimum = ProjectedAttributeValue::try_new(
                &installed,
                TypeId::new(TypeKind::Attribute, "score").expect("score"),
                CanonicalValue::Long(threshold),
            )
            .expect("minimum");
            let minimum = session.function_value(&minimum).expect("branded minimum");
            let inner = session
                .function(&FunctionId::new("qualifying-score").expect("id"))
                .expect("function")
                .call([person.function_argument(), minimum.function_argument()])
                .expect("inner");
            let outer = session
                .function(&FunctionId::new("identity-score").expect("id"))
                .expect("function")
                .call([inner.function_argument()])
                .expect("outer");
            let request = session
                .query(session.positional([person.one()]).expect("shape"))
                .expect("query")
                .where_predicate(
                    outer
                        .compare_field(
                            crate::match_request::ComparisonOp::Equal,
                            &person.field("score").expect("score field"),
                        )
                        .expect("compare"),
                )
                .expect("where")
                .exists_by(&person)
                .expect("exists");
            let validated = validate_match_request(&registry, request).expect("valid");
            prepare_remote_model_query_v2(
                &authority,
                &registry,
                validated,
                &advertisement.encode().expect("advertisement"),
                RemoteLimitsV2 {
                    deadline_ms: Some(1_000),
                    max_bytes: 8_192,
                    max_items: 10,
                    max_collection_members: 10,
                    max_graph_nodes: 10,
                    max_attribute_values: 10,
                    max_role_players: 10,
                    max_statements: 3,
                },
            )
            .expect("reprepared")
        };
        let second = rebuilt(41);
        let second_envelope =
            RemoteQueryRequestV2::decode(second.request_bytes()).expect("request");
        let second_plan = second_envelope.plan().expect("second plan");
        assert_eq!(
            plan.fingerprint().expect("plan fingerprint"),
            second_plan.fingerprint().expect("second fingerprint")
        );
        assert_eq!(
            second_envelope
                .invocation(&second_plan)
                .expect("second invocation")
                .inputs()[0]
                .values(),
            [Some(CanonicalValue::Long(41))]
        );
        let second_invocation = second_envelope
            .invocation(&second_plan)
            .expect("second invocation");
        let fixed_first = RemoteQueryRequestV2::new(
            &plan,
            &invocation,
            RemoteResultKindV2::DistinctExists,
            &advertisement,
            envelope.limits(),
            "fixed-function-request-nonce",
            1_700_000_000_000,
        )
        .expect("fixed first request");
        let fixed_second = RemoteQueryRequestV2::new(
            &plan,
            &second_invocation,
            RemoteResultKindV2::DistinctExists,
            &advertisement,
            envelope.limits(),
            "fixed-function-request-nonce",
            1_700_000_000_000,
        )
        .expect("fixed second request");
        assert_ne!(
            fixed_first.fingerprint().expect("fixed first fingerprint"),
            fixed_second
                .fingerprint()
                .expect("fixed second fingerprint"),
            "changing only a canonical invocation cell changes authenticated request identity"
        );
        assert_ne!(
            envelope.fingerprint().expect("first request fingerprint"),
            second_envelope
                .fingerprint()
                .expect("second request fingerprint"),
            "canonical invocation rows participate in request authentication"
        );
        assert_ne!(
            first_bytes,
            second.request_bytes(),
            "remote nonces make independently prepared authenticated requests unique"
        );

        let response = RemoteQueryResponseV2::new(
            envelope.nonce(),
            &plan,
            &envelope.fingerprint().expect("request fingerprint"),
            RemoteResultKindV2::DistinctExists,
            RemoteOutcomeV2::DistinctExists {
                root: QueryBindingId::new(0).expect("root binding"),
                value: false,
            },
        )
        .expect("request-bound function response")
        .encode_signed(
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            &signer,
        )
        .expect("signed function response");
        let (request, result, _) = pending
            .claim_reply()
            .expect("single function claim")
            .decode(&response)
            .expect("authenticated function decode");
        assert!(matches!(
            result
                .for_request(&request)
                .expect("invocation-bound result"),
            MatchResult::Exists { value: false, .. }
        ));
        let replayed = match pending.claim_reply() {
            Err(RemoteModelQueryV2Error::Diagnostic(replayed)) => replayed,
            Err(error) => panic!("reply replay remains a remote integrity diagnostic: {error}"),
            Ok(_) => panic!("the function request retains the ordinary one-shot claim fence"),
        };
        assert_eq!(replayed.category(), DiagnosticCategory::Integrity);
        assert_eq!(replayed.code().as_str(), "query_remote_v2_reply_replayed");
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
        let mut capabilities = query_plan_v2_capability_vocabulary();
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
            QueryExecutionDeadline::from_timeout_milliseconds(0),
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
        assert_eq!(replay.code().as_str(), "query_remote_v2_reply_replayed");

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
        assert_eq!(replay.code().as_str(), "query_remote_v2_reply_replayed");

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
        let envelope =
            RemoteQueryRequestV2::decode(pending.request_bytes()).expect("request envelope");
        assert_eq!(envelope.result_kind(), RemoteResultKindV2::ModelReduction);
        let plan = envelope.plan().expect("adapted reduction plan");
        let Some(type_bridge_contract::query_plan::ModelQueryV2::Reduction {
            root,
            group: None,
            reducers,
            ..
        }) = plan
            .v2_compatibility()
            .expect("V2 compatibility")
            .model_query()
        else {
            panic!("adapted plan lacks its reduction contract")
        };
        assert_eq!(root.get(), 0);
        assert_eq!(reducers.len(), 1);
        assert_eq!(
            reducers[0].reduction(),
            type_bridge_contract::query_plan::QueryReductionKindV2::Count
        );
        assert!(reducers[0].input().is_none());

        let response = RemoteQueryResponseV2::new(
            envelope.nonce(),
            &plan,
            &envelope.fingerprint().expect("request fingerprint"),
            RemoteResultKindV2::ModelReduction,
            RemoteOutcomeV2::ModelReduction {
                graph: HydrationGraphV2::new(vec![]).expect("empty graph"),
                root: *root,
                group: None,
                reducers: reducers.clone(),
                rows: vec![RemoteReductionRowV2::new(
                    None,
                    vec![RemoteReducedValueV2::Count { value: 7 }],
                )],
            },
        )
        .expect("request-bound reduction response")
        .encode_signed(
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            &signer,
        )
        .expect("signed reduction response");
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
        assert_eq!(replay.code().as_str(), "query_remote_v2_reply_replayed");
    }
}
