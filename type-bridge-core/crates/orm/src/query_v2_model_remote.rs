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
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::query_plan::QueryInvocation;
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;

use crate::match_request::lowering::preflight_released_match_execution;
use crate::match_request::result_validation::validated_match_result_from_v2;
use crate::match_request::{MatchError, ValidatedMatchRequest, ValidatedMatchResult};
use crate::query_v2::failure;
use crate::query_v2_adapter::{
    MatchRequestAdaptation, V1ResourceEnvelopeReason, adapt_match_request,
};
use crate::query_v2_prepared::{
    ClaimedRemoteReplyV2, PendingRemoteQueryV2, QueryAuthority, prepare_validated_remote_query_v2,
};
use crate::registry::DescriptorRegistry;

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

    /// Atomically reserve the sole reply before binding code snapshots bytes.
    pub fn claim_reply(&self) -> Result<ClaimedRemoteModelReplyV2, RemoteModelQueryV2Error> {
        let claimed = self.pending.claim_reply()?;
        let request = self
            .request
            .lock()
            .map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_v2_model_claim_state",
                    "remote model-query claim state is unavailable",
                )
            })?
            .take()
            .ok_or_else(|| {
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
        let Self {
            claimed,
            registry,
            request,
        } = self;
        let outcome = claimed.decode_outcome(response_bytes)?;
        let result = validated_match_result_from_v2(&registry, &request, outcome)?;
        Ok((request, result, registry))
    }
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
    let registry = Arc::new(registry.owned_registry_snapshot().map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_registry_snapshot_failed",
            "descriptor registry could not be snapshotted for remote execution",
        )
    })?);
    request.recheck_schema(&registry).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_registry_snapshot_mismatch",
            "validated model query does not belong to the fenced registry snapshot",
        )
    })?;
    preflight_released_match_execution(&registry, &request)?;

    let adapted = match adapt_match_request(
        &request,
        &registry,
        &authority.context(),
        StructuralLimits::CANONICAL,
    )? {
        MatchRequestAdaptation::Adapted(adapted) => adapted,
        MatchRequestAdaptation::LegacyRequired(reason) => {
            return Err(resource_envelope_failure(reason).into());
        }
    };
    let invocation =
        QueryInvocation::new(adapted.validated().plan(), adapted.operation(), Vec::new())?;
    let pending = prepare_validated_remote_query_v2(
        authority,
        adapted.validated().clone(),
        invocation,
        advertisement_bytes,
        limits,
    )?;
    Ok(PendingRemoteModelQueryV2 {
        pending,
        registry,
        request: Mutex::new(Some(request)),
    })
}

fn resource_envelope_failure(reason: V1ResourceEnvelopeReason) -> Diagnostic {
    let message = match reason {
        V1ResourceEnvelopeReason::LiteralExceedsCanonicalArtifact => {
            "released model-query literal exceeds the canonical V2 remote artifact ceiling"
        }
        V1ResourceEnvelopeReason::EncodedPlanExceedsCanonicalArtifact => {
            "adapted model query exceeds the canonical V2 remote artifact ceiling"
        }
    };
    failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_model_artifact_limit",
        message,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_schema_compat::released_typeql_to_declared_projection;

    use super::*;
    use crate::attribute::ValueType;
    use crate::descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::entity::Annotation;
    use crate::match_request::{
        BindingId, BoundFieldId, FetchShape, FetchSlot, FieldId, MatchBinding, MatchErrorCategory,
        MatchErrorPathSegment, MatchMode, MatchOperation, MatchOrder, MatchPlan, MatchRequest,
        MissingOrder, RowCardinality, SortDirection, ThingKind, Window, validate_match_request,
    };
    use crate::query_v2_prepared::QueryAuthority;
    use crate::schema::{SchemaInfo, generator::generate_define_block};

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
}
