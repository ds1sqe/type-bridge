//! Production one-way adaptation of released V1 match requests onto V2 plans.
//!
//! A validated V1 request is translated into one closed V2 compatibility
//! program. The ordinary plan contains only a canonical target/output
//! skeleton; the compatibility tree is the sole predicate source consumed by
//! compatibility lowering. Schema-aware validation recomputes the hydration
//! graph and rejects any second native semantic program before provider I/O.
//!
//! Arbitrary V1 strings predate the fixed V2 canonical artifact ceiling. A
//! request whose exact V2 representation cannot fit that envelope receives a
//! typed, non-error legacy disposition so the released direct executor can
//! preserve its behavior. This fallback is decided only after V1 validation
//! and registry recheck; decoded V2 input can never request it.

use std::collections::BTreeSet;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_CANONICAL_BYTES, StructuralLimits};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, HydrationBindingV2, HydrationDescriptorV2, HydrationFieldV2,
    HydrationPlayerV2, HydrationProjectionV2, HydrationRoleV2, ModelQueryV2, QueryBindingPairV2,
    QueryComparatorV2, QueryFieldV2, QueryMissingOrderV2, QueryModelOutputSlotV2,
    QueryModelOutputV2, QueryNamedOutputSlotV2, QueryOperation, QueryOrderDirectionV2,
    QueryOrderTermV2, QueryOutput, QueryPattern, QueryPatternV2, QueryPlanV2Compatibility,
    QueryRowCardinalityV2, QueryStableOrderV2, QueryWindowV2, ReadStage,
};
use type_bridge_contract::schema::{DocumentId, ManagedSchemaState};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, Cardinality, DecimalValue, ValueTypeTag,
};
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery};
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, ResolvedType, managed_schema_state, resolve,
};
use type_bridge_schema_compat::released_typeql_to_declared_projection;

use crate::AttributeValue;
use crate::attribute::ValueType;
use crate::descriptor::{
    OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptorRef,
};
use crate::entity::Annotation;
use crate::match_request::{
    BoundFieldId, Capability, ComparisonOp, FetchShape, FetchSlot, MatchExpr, MatchMode,
    MatchOperation, MatchRequest, MatchRequestVersion, MissingOrder, RowCardinality, SortDirection,
    StableOrderSpec, ThingKind, ValidatedMatchRequest,
};
use crate::query_v2_builder::{QueryCompatibilityPlanInput, QueryPlanBuilder};
use crate::registry::DescriptorRegistry;
use crate::schema::{SchemaInfo, generator::generate_define_block};

// This identifies the current Rust resolver table; it is not a connected
// server or protocol-band selector. Released descriptor projections
// materialize field cardinality, and compatibility lowering is checked against
// the released typed provider AST. The transaction's negotiated band alone
// selects inline (bands 7/8) versus `given` (band 9) transport.
const V1_ADAPTER_SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const V1_ADAPTER_MANAGED_SCOPE: &str = "typebridge-v1-descriptor-registry";
const V1_ADAPTER_SCHEMA_DOCUMENT: &str = "typebridge-v1-descriptor-registry.tql";

/// Owned schema authority reconstructed from the exact descriptor snapshot
/// which validated a released request.
///
/// The registry remains the V1 public authority. This internal projection is
/// deterministic and performs no provider, filesystem, or transport I/O.
pub(crate) struct MatchRequestAdapterAuthority {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

impl MatchRequestAdapterAuthority {
    /// Reconstruct the strict V2 schema authority represented by one released
    /// descriptor registry.
    pub(crate) fn from_registry(registry: &DescriptorRegistry) -> Result<Self, Diagnostic> {
        let schema = SchemaInfo::from_descriptors(&registry.snapshot());
        let source = generate_define_block(&schema);
        // The registry, not this generated TypeQL, remains the exact released
        // authority. Use schema-compat's portable projection so released list
        // capability markers and @distinct do not make an otherwise valid V1
        // descriptor snapshot unrepresentable in the strict fact graph.
        let declared = released_typeql_to_declared_projection(
            DocumentId::new(V1_ADAPTER_SCHEMA_DOCUMENT)?,
            &source,
        )
        .map_err(|_| {
            integrity(
                "query_v2_adapter_registry_projection",
                "validated descriptor registry cannot be projected into V2 schema authority",
            )
        })?;
        let profile = SemanticProfileId::new(V1_ADAPTER_SEMANTIC_PROFILE)?;
        let resolved = resolve(&declared, &profile).map_err(|_| {
            integrity(
                "query_v2_adapter_registry_projection",
                "validated descriptor registry cannot be resolved as V2 schema authority",
            )
        })?;
        let delta_context = ManagedDeltaContext::new(
            ManagedScopeId::new(V1_ADAPTER_MANAGED_SCOPE)?,
            profile,
            declared.required_capabilities().clone(),
        );
        let managed = managed_schema_state(&declared, &delta_context).map_err(|_| {
            integrity(
                "query_v2_adapter_registry_projection",
                "validated descriptor registry cannot form managed V2 schema authority",
            )
        })?;
        Ok(Self { managed, resolved })
    }

    /// Borrow the ordinary validator context for adaptation.
    pub(crate) fn context(&self) -> MigrationAssertionValidationContext<'_> {
        MigrationAssertionValidationContext::new(&self.resolved, &self.managed)
    }
}

/// Why a released request must remain on the direct V1 executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V1ResourceEnvelopeReason {
    /// One exact released literal is larger than the complete V2 artifact.
    LiteralExceedsCanonicalArtifact,
    /// The complete canonical plan, including escaping and hydration, is too large.
    EncodedPlanExceedsCanonicalArtifact,
}

/// Result of production V1-to-V2 adaptation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MatchRequestAdaptation {
    /// Execute the schema-validated ordinary V2 plan.
    Adapted(AdaptedMatchRequest),
    /// Preserve released behavior through the already-validated direct V1 path.
    LegacyRequired(V1ResourceEnvelopeReason),
}

/// The V2 program one V1 request adapts to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdaptedMatchRequest {
    operation: QueryOperation,
    validated: Box<ValidatedQuery>,
}

impl AdaptedMatchRequest {
    /// Return the schema-validated adapted query.
    #[must_use]
    pub(crate) const fn validated(&self) -> &ValidatedQuery {
        &self.validated
    }

    /// Return the adapted closed operation.
    #[must_use]
    pub(crate) const fn operation(&self) -> QueryOperation {
        self.operation
    }
}

/// Adapt one canonically validated released V1 request.
///
/// Semantic translation failures are diagnostics and indicate a bridge bug or
/// stale/contradictory authority. Only a fixed V2 representation-envelope
/// mismatch returns [`MatchRequestAdaptation::LegacyRequired`].
pub(crate) fn adapt_match_request(
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<MatchRequestAdaptation, Diagnostic> {
    validated.recheck_schema(registry).map_err(|_| {
        reject(
            "query_v2_adapter_registry_mismatch",
            "validated V1 request does not belong to the supplied registry snapshot",
        )
    })?;
    let request = validated.request();
    if request.version != MatchRequestVersion::V1 {
        return Err(reject(
            "query_v2_adapter_version_unsupported",
            "only V1 match requests adapt onto the V2 vocabulary",
        ));
    }
    for capability in validated.capabilities().iter().copied() {
        assert_released_capability_mapped(capability);
    }
    for (index, binding) in request.plan.bindings.iter().enumerate() {
        if usize::from(binding.id.get()) != index {
            return Err(reject(
                "query_v2_adapter_bindings_not_dense",
                "V1 bindings must be dense zero-based ordinals",
            ));
        }
    }
    if request
        .plan
        .predicate
        .as_ref()
        .is_some_and(predicate_has_artifact_sized_literal)
    {
        return Ok(MatchRequestAdaptation::LegacyRequired(
            V1ResourceEnvelopeReason::LiteralExceedsCanonicalArtifact,
        ));
    }

    let schema = context.resolved_schema();
    let hydration = build_hydration(request, registry, schema)?;
    let predicate = request
        .plan
        .predicate
        .as_ref()
        .map(|expression| adapt_expression(expression, request, registry))
        .transpose()?;
    let cross_joins = request
        .plan
        .allowed_cross_joins
        .iter()
        .map(|pair| {
            Ok(QueryBindingPairV2::new(
                BindingId::new(pair.left.get())?,
                BindingId::new(pair.right.get())?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let (operation, model_query, output_columns) = adapt_operation(validated, registry, hydration)?;

    let bindings = request
        .plan
        .bindings
        .iter()
        .enumerate()
        .map(|(index, _)| {
            Ok(AssertionBinding::new(
                binding_ordinal(index)?,
                QueryVariable::new(format!("b{index}"))?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let patterns = request
        .plan
        .bindings
        .iter()
        .map(|binding| {
            Ok(QueryPattern::Isa {
                binding: BindingId::new(binding.id.get())?,
                include_subtypes: binding.match_mode == MatchMode::Subtypes,
                type_id: descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut selected = output_columns.clone();
    selected.sort_unstable();
    selected.dedup();
    let pipeline = vec![
        ReadStage::Match { patterns },
        ReadStage::Select { bindings: selected },
        ReadStage::Distinct,
    ];
    let compatibility = QueryPlanV2Compatibility::new(predicate, cross_joins, Some(model_query));
    let validated = match QueryPlanBuilder::finalize_compatibility(
        QueryCompatibilityPlanInput::new(
            bindings,
            pipeline,
            QueryOutput::Rows {
                columns: output_columns,
            },
            compatibility,
        ),
        context,
        limits,
    ) {
        Ok(validated) => validated,
        Err(error) if error.category() == DiagnosticCategory::ResourceLimit => {
            return Ok(MatchRequestAdaptation::LegacyRequired(
                V1ResourceEnvelopeReason::EncodedPlanExceedsCanonicalArtifact,
            ));
        }
        Err(error) => return Err(error),
    };
    match to_canonical_json(validated.plan()) {
        Ok(_) => Ok(MatchRequestAdaptation::Adapted(AdaptedMatchRequest {
            operation,
            validated: Box::new(validated),
        })),
        Err(error) if error.category() == DiagnosticCategory::ResourceLimit => {
            Ok(MatchRequestAdaptation::LegacyRequired(
                V1ResourceEnvelopeReason::EncodedPlanExceedsCanonicalArtifact,
            ))
        }
        Err(error) => Err(error),
    }
}

/// Compile-time completeness guard for the released provider vocabulary.
const fn assert_released_capability_mapped(capability: Capability) {
    match capability {
        Capability::ResourceBoundedStreaming
        | Capability::ExactEntityTarget
        | Capability::ExactRelationTarget
        | Capability::SubtypeEntityTarget
        | Capability::SubtypeRelationTarget
        | Capability::FieldComparison
        | Capability::BooleanPattern
        | Capability::SelectedTupleDistinct
        | Capability::StableSelectedOrder
        | Capability::DistinctRootSelection
        | Capability::StableRootOrder
        | Capability::SameTransactionRehydration
        | Capability::BatchIdentityRebind
        | Capability::DistinctRootCount
        | Capability::DistinctRootExists
        | Capability::Collect
        | Capability::CollectDistinct
        | Capability::StableCollectionOrder
        | Capability::BoundedReachability => {}
    }
}

fn build_hydration(
    request: &MatchRequest,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<HydrationProjectionV2, Diagnostic> {
    let mut bindings = Vec::with_capacity(request.plan.bindings.len());
    let mut admitted = BTreeSet::new();
    for binding in &request.plan.bindings {
        let declared = descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?;
        let concrete =
            constructible_closure(schema, &declared, binding.match_mode == MatchMode::Subtypes)?;
        admitted.extend(concrete.iter().cloned());
        bindings.push(HydrationBindingV2::new(
            BindingId::new(binding.id.get())?,
            declared,
            concrete,
        ));
    }

    let role_hydrated = admitted
        .iter()
        .filter(|descriptor| descriptor.kind() == TypeKind::Relation)
        .cloned()
        .collect::<BTreeSet<_>>();
    for relation in &role_hydrated {
        let resolved = schema.types().get(relation).ok_or_else(|| {
            integrity(
                "query_v2_adapter_schema_mismatch",
                "binding hydration relation is absent from resolved schema",
            )
        })?;
        for role in resolved.relates().keys() {
            admitted.extend(
                schema
                    .roles()
                    .get(role)
                    .ok_or_else(|| {
                        integrity(
                            "query_v2_adapter_schema_mismatch",
                            "effective role lacks resolved player authority",
                        )
                    })?
                    .accepted_players()
                    .iter()
                    .cloned(),
            );
        }
    }

    let descriptors = admitted
        .iter()
        .map(|descriptor| {
            hydration_descriptor(
                descriptor,
                role_hydrated.contains(descriptor),
                registry,
                schema,
            )
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(HydrationProjectionV2::new(bindings, descriptors))
}

fn hydration_descriptor(
    id: &TypeId,
    hydrate_roles: bool,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<HydrationDescriptorV2, Diagnostic> {
    let resolved = schema.types().get(id).ok_or_else(|| {
        integrity(
            "query_v2_adapter_schema_mismatch",
            "hydration descriptor is absent from resolved schema",
        )
    })?;
    let registered = registered_descriptor(registry, id)?;
    let registered_fields = descriptor_attributes(&registered);
    let mut fields = Vec::with_capacity(resolved.owns().len());
    for (attribute, owns) in resolved.owns() {
        let registered = registered_fields
            .iter()
            .find(|candidate| candidate.attr_name == attribute.label().as_str())
            .ok_or_else(|| {
                integrity(
                    "query_v2_adapter_registry_authority_mismatch",
                    "registered hydration descriptor omits a resolved effective attribute",
                )
            })?;
        let attribute_type = schema
            .types()
            .get(
                &TypeId::new(TypeKind::Attribute, attribute.label().as_str().to_owned()).map_err(
                    |_| {
                        integrity(
                            "query_v2_adapter_schema_mismatch",
                            "resolved attribute label is malformed",
                        )
                    },
                )?,
            )
            .and_then(ResolvedType::value_type)
            .ok_or_else(|| {
                integrity(
                    "query_v2_adapter_schema_mismatch",
                    "resolved owned attribute lacks a scalar value type",
                )
            })?
            .value_type();
        if value_type_tag(registered.value_type) != attribute_type
            || registered_cardinality(registered)? != owns.cardinality()
            || (registered.is_key() || registered.is_unique())
                != (owns.is_key() || owns.is_unique())
        {
            return Err(integrity(
                "query_v2_adapter_registry_authority_mismatch",
                "registered field type, cardinality, or uniqueness contradicts resolved schema",
            ));
        }
        fields.push(HydrationFieldV2::new(
            registered.field_name.clone(),
            field_reference_owners(schema, resolved, attribute),
            attribute.clone(),
            attribute_type,
            owns.cardinality(),
            registered.is_ordered,
            registered
                .annotations
                .iter()
                .any(|annotation| matches!(annotation, Annotation::Distinct)),
            owns.is_key() || owns.is_unique(),
        ));
    }
    fields.sort_by(|left, right| left.alias().cmp(right.alias()));

    let roles = if hydrate_roles {
        let TypeDescriptorRef::Relation(registered) = registered else {
            return Err(integrity(
                "query_v2_adapter_registry_authority_mismatch",
                "resolved relation is registered as an entity",
            ));
        };
        hydration_roles(id, resolved, &registered, registry, schema)?
    } else {
        Vec::new()
    };
    Ok(HydrationDescriptorV2::new(id.clone(), fields, roles))
}

fn hydration_roles(
    concrete: &TypeId,
    resolved: &ResolvedType,
    registered: &RelationDescriptor,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<Vec<HydrationRoleV2>, Diagnostic> {
    let mut roles = Vec::with_capacity(resolved.relates().len());
    for (effective_id, effective) in resolved.relates() {
        let registered_role = registered
            .roles
            .iter()
            .find(|role| role.role_name == effective_id.label().as_str())
            .ok_or_else(|| {
                integrity(
                    "query_v2_adapter_registry_authority_mismatch",
                    "registered relation omits a resolved effective role",
                )
            })?;
        if registered_role_cardinality(registered_role)? != effective.cardinality() {
            return Err(integrity(
                "query_v2_adapter_registry_authority_mismatch",
                "registered role cardinality contradicts resolved schema",
            ));
        }
        let accepted = schema
            .roles()
            .get(effective_id)
            .ok_or_else(|| {
                integrity(
                    "query_v2_adapter_schema_mismatch",
                    "effective role lacks resolved accepted-player authority",
                )
            })?
            .accepted_players();
        let mut players = registered_role
            .player_type_names
            .iter()
            .filter_map(|name| registry.get(name).map(|descriptor| (name, descriptor)))
            .map(|(name, descriptor)| {
                let declared = registered_type_id(name, &descriptor)?;
                let concrete = constructible_closure(schema, &declared, true)?
                    .into_iter()
                    .filter(|candidate| accepted.contains(candidate))
                    .collect();
                Ok(HydrationPlayerV2::new(declared, concrete))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        players.sort_by(|left, right| left.declared_descriptor().cmp(right.declared_descriptor()));
        players.dedup_by(|left, right| left.declared_descriptor() == right.declared_descriptor());
        let covered = players
            .iter()
            .flat_map(|player| player.concrete_descriptors().iter().cloned())
            .collect::<BTreeSet<_>>();
        if &covered != accepted {
            return Err(integrity(
                "query_v2_adapter_registry_authority_mismatch",
                "registered role-player declarations do not cover resolved accepted players",
            ));
        }
        roles.push(HydrationRoleV2::new(
            RoleId::new(
                concrete.label().as_str().to_owned(),
                effective_id.label().as_str().to_owned(),
            )?,
            role_reference_ids(schema, resolved, effective_id),
            players,
            effective.cardinality(),
            registered_role.ordered,
            registered_role.distinct,
        ));
    }
    roles.sort_by(|left, right| left.role().cmp(right.role()));
    Ok(roles)
}

fn adapt_operation(
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    hydration: HydrationProjectionV2,
) -> Result<(QueryOperation, ModelQueryV2, Vec<BindingId>), Diagnostic> {
    let request = validated.request();
    Ok(match &request.operation {
        MatchOperation::FetchRows {
            output,
            window,
            cardinality,
            ..
        } => {
            let model_output = adapt_output(output, validated, registry, request)?;
            let columns = output_slots(output)
                .map(|slot| BindingId::new(slot.binding().get()))
                .collect::<Result<Vec<_>, _>>()?;
            let (cardinality, order) = match cardinality {
                RowCardinality::ExactlyOne => (QueryRowCardinalityV2::ExactlyOne, None),
                RowCardinality::BoundedMany => (
                    QueryRowCardinalityV2::BoundedMany,
                    Some(adapt_stable_order(
                        validated.stable_order(),
                        columns.clone(),
                        registry,
                        request,
                    )?),
                ),
            };
            (
                QueryOperation::Rows,
                ModelQueryV2::Rows {
                    cardinality,
                    hydration,
                    order,
                    output: model_output,
                    window: QueryWindowV2::new(window.offset, window.limit),
                },
                columns,
            )
        }
        MatchOperation::PageBy {
            root,
            output,
            window,
            include_total,
            ..
        } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Rows,
                ModelQueryV2::Page {
                    hydration,
                    include_total: *include_total,
                    order: adapt_stable_order(
                        validated.stable_order(),
                        vec![root],
                        registry,
                        request,
                    )?,
                    output: adapt_output(output, validated, registry, request)?,
                    root,
                    window: QueryWindowV2::new(window.offset, window.limit),
                },
                vec![root],
            )
        }
        MatchOperation::CountBy { root } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Count,
                ModelQueryV2::DistinctCount { hydration, root },
                vec![root],
            )
        }
        MatchOperation::ExistsBy { root } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Exists,
                ModelQueryV2::DistinctExists { hydration, root },
                vec![root],
            )
        }
    })
}

fn adapt_output(
    output: &FetchShape,
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<QueryModelOutputV2, Diagnostic> {
    let adapt_slot = |slot: &FetchSlot| -> Result<QueryModelOutputSlotV2, Diagnostic> {
        let binding = BindingId::new(slot.binding().get())?;
        let declared = request_binding_type(request, slot.binding().get())?;
        Ok(match slot {
            FetchSlot::One { .. } => QueryModelOutputSlotV2::One { binding, declared },
            FetchSlot::Collect {
                distinct, order: _, ..
            } => {
                let proof = validated.collection_order(slot.binding()).ok_or_else(|| {
                    integrity(
                        "query_v2_adapter_collection_order_proof",
                        "validated collection output lacks its stable-order proof",
                    )
                })?;
                QueryModelOutputSlotV2::Collect {
                    binding,
                    declared,
                    distinct: *distinct,
                    order: adapt_stable_order(proof, vec![binding], registry, request)?,
                }
            }
        })
    };
    Ok(match output {
        FetchShape::Positional { slots } => QueryModelOutputV2::Positional {
            slots: slots
                .iter()
                .map(adapt_slot)
                .collect::<Result<Vec<_>, _>>()?,
        },
        FetchShape::Named { slots } => QueryModelOutputV2::Named {
            slots: slots
                .iter()
                .map(|named| {
                    Ok(QueryNamedOutputSlotV2::new(
                        named.name.clone(),
                        adapt_slot(&named.slot)?,
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?,
        },
    })
}

fn adapt_stable_order(
    order: &StableOrderSpec,
    identity_tiebreakers: Vec<BindingId>,
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<QueryStableOrderV2, Diagnostic> {
    let terms = order
        .terms()
        .iter()
        .map(|term| {
            let order = term.order();
            Ok(QueryOrderTermV2::new(
                adapt_field(&order.field, request, registry)?,
                match order.direction {
                    SortDirection::Ascending => QueryOrderDirectionV2::Ascending,
                    SortDirection::Descending => QueryOrderDirectionV2::Descending,
                },
                match order.missing {
                    MissingOrder::Reject => QueryMissingOrderV2::Reject,
                    MissingOrder::First => QueryMissingOrderV2::First,
                    MissingOrder::Last => QueryMissingOrderV2::Last,
                },
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(QueryStableOrderV2::new(terms, identity_tiebreakers))
}

fn adapt_expression(
    expression: &MatchExpr,
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<QueryPatternV2, Diagnostic> {
    Ok(match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => QueryPatternV2::FieldValue {
            field: adapt_field(field, request, registry)?,
            comparator: adapt_comparator(*operator),
            value: adapt_value(value)?,
        },
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => QueryPatternV2::FieldComparison {
            left: adapt_field(left, request, registry)?,
            comparator: adapt_comparator(*operator),
            right: adapt_field(right, request, registry)?,
        },
        MatchExpr::RoleEdge {
            relation,
            role,
            player,
            ..
        } => {
            let relation_binding = request
                .plan
                .bindings
                .get(usize::from(relation.get()))
                .filter(|binding| binding.thing_kind == ThingKind::Relation)
                .ok_or_else(|| {
                    integrity(
                        "query_v2_adapter_unknown_binding",
                        "validated role edge references no relation binding",
                    )
                })?;
            QueryPatternV2::RoleEdge {
                include_relation_subtypes: relation_binding.match_mode == MatchMode::Subtypes,
                player: BindingId::new(player.get())?,
                relation: BindingId::new(relation.get())?,
                relation_type: descriptor_type(
                    relation_binding.descriptor.as_str(),
                    ThingKind::Relation,
                )?,
                role: descriptor_role_id(role.owner.as_str(), &role.name)?,
            }
        }
        MatchExpr::Reachable {
            relation,
            role_from,
            role_to,
            source,
            target,
            min_depth,
            max_depth,
        } => {
            let relation_type = descriptor_type(relation.as_str(), ThingKind::Relation)?;
            QueryPatternV2::Reachable {
                min_depth: *min_depth,
                max_depth: *max_depth,
                role_from: RoleId::new(
                    relation_type.label().as_str().to_owned(),
                    role_from.name.clone(),
                )?,
                role_to: RoleId::new(
                    relation_type.label().as_str().to_owned(),
                    role_to.name.clone(),
                )?,
                relation: relation_type,
                source: BindingId::new(source.get())?,
                target: BindingId::new(target.get())?,
            }
        }
        MatchExpr::And { expressions } => QueryPatternV2::And {
            patterns: expressions
                .iter()
                .map(|child| adapt_expression(child, request, registry))
                .collect::<Result<Vec<_>, _>>()?,
        },
        MatchExpr::Or { expressions } => QueryPatternV2::Or {
            patterns: expressions
                .iter()
                .map(|child| adapt_expression(child, request, registry))
                .collect::<Result<Vec<_>, _>>()?,
        },
        MatchExpr::Not { expression } => QueryPatternV2::Not {
            pattern: Box::new(adapt_expression(expression, request, registry)?),
        },
    })
}

fn adapt_field(
    field: &BoundFieldId,
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<QueryFieldV2, Diagnostic> {
    request
        .plan
        .bindings
        .get(usize::from(field.binding.get()))
        .ok_or_else(|| {
            integrity(
                "query_v2_adapter_unknown_binding",
                "validated field references no declared binding",
            )
        })?;
    let attribute = registered_attribute(registry, &field.field)?;
    Ok(QueryFieldV2::new(
        BindingId::new(field.binding.get())?,
        descriptor_id_type(field.field.owner.as_str())?,
        AttributeId::new(attribute.attr_name.clone())?,
        value_type_tag(attribute.value_type),
    ))
}

fn adapt_comparator(operator: ComparisonOp) -> QueryComparatorV2 {
    match operator {
        ComparisonOp::Equal => QueryComparatorV2::Equal,
        ComparisonOp::NotEqual => QueryComparatorV2::NotEqual,
        ComparisonOp::LessThan => QueryComparatorV2::Less,
        ComparisonOp::LessThanOrEqual => QueryComparatorV2::LessOrEqual,
        ComparisonOp::GreaterThan => QueryComparatorV2::Greater,
        ComparisonOp::GreaterThanOrEqual => QueryComparatorV2::GreaterOrEqual,
        ComparisonOp::Contains => QueryComparatorV2::Contains,
        ComparisonOp::StartsWith => QueryComparatorV2::StartsWith,
        ComparisonOp::EndsWith => QueryComparatorV2::EndsWith,
        ComparisonOp::Regex => QueryComparatorV2::Regex,
    }
}

pub(crate) fn adapt_value(value: &AttributeValue) -> Result<CompatibilityValueV2, Diagnostic> {
    let malformed = || {
        integrity(
            "query_v2_adapter_validated_literal_malformed",
            "a validated V1 literal no longer satisfies its released scalar domain",
        )
    };
    let canonical = match value {
        AttributeValue::String(value) => match CanonicalString::new(value.as_str()) {
            Ok(value) => Some(CanonicalValue::String(value)),
            Err(_) => return CompatibilityValueV2::released_string(value.clone()),
        },
        AttributeValue::Long(value) => Some(CanonicalValue::Long(*value)),
        AttributeValue::Double(value) => Some(CanonicalValue::Double(
            CanonicalDouble::new(*value).map_err(|_| malformed())?,
        )),
        AttributeValue::Boolean(value) => Some(CanonicalValue::Boolean(*value)),
        AttributeValue::Date(value) => Some(CanonicalValue::Date(
            value.parse::<CanonicalDate>().map_err(|_| malformed())?,
        )),
        AttributeValue::DateTime(value) => match value.parse::<CanonicalDateTime>() {
            Ok(value) => Some(CanonicalValue::DateTime(value)),
            Err(_) => return CompatibilityValueV2::released_datetime(value.clone()),
        },
        AttributeValue::DateTimeTZ(value) => match value.parse::<CanonicalDateTimeTz>() {
            Ok(value) => Some(CanonicalValue::DateTimeTz(value)),
            Err(_) => return CompatibilityValueV2::released_datetime_tz(value.clone()),
        },
        AttributeValue::Decimal(value) => {
            let canonical = DecimalValue::new(value.as_str()).map_err(|_| malformed())?;
            if canonical.as_str() != value {
                return CompatibilityValueV2::released_decimal(value.clone());
            }
            Some(CanonicalValue::Decimal(canonical))
        }
        AttributeValue::Duration(value) => match value.parse::<CanonicalDuration>() {
            Ok(value) => Some(CanonicalValue::Duration(value)),
            Err(_) => return CompatibilityValueV2::released_duration(value.clone()),
        },
    };
    Ok(CompatibilityValueV2::canonical(canonical.ok_or_else(
        || {
            integrity(
                "query_v2_adapter_literal_state",
                "validated literal adaptation produced no canonical or released value",
            )
        },
    )?))
}

fn predicate_has_artifact_sized_literal(expression: &MatchExpr) -> bool {
    match expression {
        MatchExpr::FieldValue { value, .. } => {
            textual_value(value).is_some_and(|value| value.len() > MAX_CANONICAL_BYTES)
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            expressions.iter().any(predicate_has_artifact_sized_literal)
        }
        MatchExpr::Not { expression } => predicate_has_artifact_sized_literal(expression),
        MatchExpr::FieldComparison { .. }
        | MatchExpr::RoleEdge { .. }
        | MatchExpr::Reachable { .. } => false,
    }
}

fn textual_value(value: &AttributeValue) -> Option<&str> {
    match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => Some(value),
        AttributeValue::Long(_) | AttributeValue::Double(_) | AttributeValue::Boolean(_) => None,
    }
}

fn constructible_closure(
    schema: &ResolvedSchema,
    declared: &TypeId,
    include_subtypes: bool,
) -> Result<Vec<TypeId>, Diagnostic> {
    let resolved = schema.types().get(declared).ok_or_else(|| {
        integrity(
            "query_v2_adapter_schema_mismatch",
            "registered descriptor is absent from resolved schema",
        )
    })?;
    let candidates = std::iter::once(declared).chain(
        include_subtypes
            .then_some(resolved.subtypes().iter())
            .into_iter()
            .flatten(),
    );
    let mut candidates = candidates
        .filter(|candidate| {
            schema
                .types()
                .get(*candidate)
                .is_some_and(ResolvedType::is_constructible)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

fn registered_descriptor(
    registry: &DescriptorRegistry,
    id: &TypeId,
) -> Result<TypeDescriptorRef, Diagnostic> {
    let descriptor = registry.get(id.label().as_str()).ok_or_else(|| {
        integrity(
            "query_v2_adapter_registry_authority_mismatch",
            "resolved hydration descriptor has no registered model descriptor",
        )
    })?;
    if registered_type_id(id.label().as_str(), &descriptor)? == *id {
        Ok(descriptor)
    } else {
        Err(integrity(
            "query_v2_adapter_registry_authority_mismatch",
            "registered hydration descriptor kind contradicts resolved schema",
        ))
    }
}

fn registered_type_id(name: &str, descriptor: &TypeDescriptorRef) -> Result<TypeId, Diagnostic> {
    TypeId::new(
        match descriptor {
            TypeDescriptorRef::Entity(_) => TypeKind::Entity,
            TypeDescriptorRef::Relation(_) => TypeKind::Relation,
        },
        name.to_owned(),
    )
}

fn descriptor_attributes(descriptor: &TypeDescriptorRef) -> &[OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

fn registered_attribute(
    registry: &DescriptorRegistry,
    field: &crate::match_request::FieldId,
) -> Result<OwnedAttributeDescriptor, Diagnostic> {
    let owner = registry.descriptor_type_name(&field.owner).ok_or_else(|| {
        integrity(
            "query_v2_adapter_registry_authority_mismatch",
            "validated field owner is no longer registered",
        )
    })?;
    let descriptor = registry.get(&owner).ok_or_else(|| {
        integrity(
            "query_v2_adapter_registry_authority_mismatch",
            "validated field owner is no longer registered",
        )
    })?;
    descriptor_attributes(&descriptor)
        .iter()
        .find(|attribute| attribute.field_name == field.name)
        .cloned()
        .ok_or_else(|| {
            integrity(
                "query_v2_adapter_registry_authority_mismatch",
                "validated field is absent from its registered owner",
            )
        })
}

fn field_reference_owners(
    schema: &ResolvedSchema,
    concrete: &ResolvedType,
    attribute: &AttributeId,
) -> Vec<TypeId> {
    let Some(concrete_owns) = concrete.owns().get(attribute) else {
        return Vec::new();
    };
    let declared = concrete_owns.origin().declared();
    let mut owners = std::iter::once(concrete.id())
        .chain(concrete.supertypes())
        .filter(|owner| {
            schema
                .types()
                .get(*owner)
                .and_then(|resolved| resolved.owns().get(attribute))
                .is_some_and(|owns| owns.origin().declared() == declared)
        })
        .cloned()
        .collect::<Vec<_>>();
    owners.sort();
    owners.dedup();
    owners
}

fn role_reference_ids(
    schema: &ResolvedSchema,
    concrete: &ResolvedType,
    effective_id: &RoleId,
) -> Vec<RoleId> {
    let Some(concrete_relates) = concrete.relates().get(effective_id) else {
        return Vec::new();
    };
    let declared = concrete_relates.origin().declared();
    let mut roles = std::iter::once(concrete.id())
        .chain(concrete.supertypes())
        .filter_map(|owner| {
            let resolved = schema.types().get(owner)?;
            let (_, relates) = resolved
                .relates()
                .iter()
                .find(|(role, _)| role.label() == effective_id.label())?;
            (relates.origin().declared() == declared)
                .then(|| RoleId::new(owner.label().as_str(), effective_id.label().as_str()).ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    roles.sort();
    roles.dedup();
    roles
}

fn registered_cardinality(attribute: &OwnedAttributeDescriptor) -> Result<Cardinality, Diagnostic> {
    let (minimum, maximum) = attribute.cardinality().unwrap_or(if attribute.is_optional {
        (0, Some(1))
    } else {
        (1, Some(1))
    });
    Cardinality::new(u64::from(minimum), maximum.map(u64::from))
}

fn registered_role_cardinality(role: &RoleDescriptor) -> Result<Cardinality, Diagnostic> {
    let (minimum, maximum) = role.cardinality.unwrap_or((0, Some(1)));
    Cardinality::new(u64::from(minimum), maximum.map(u64::from))
}

const fn value_type_tag(value_type: ValueType) -> ValueTypeTag {
    match value_type {
        ValueType::String => ValueTypeTag::String,
        ValueType::Long => ValueTypeTag::Long,
        ValueType::Double => ValueTypeTag::Double,
        ValueType::Boolean => ValueTypeTag::Boolean,
        ValueType::Date => ValueTypeTag::Date,
        ValueType::DateTime => ValueTypeTag::DateTime,
        ValueType::DateTimeTz => ValueTypeTag::DateTimeTz,
        ValueType::Decimal => ValueTypeTag::Decimal,
        ValueType::Duration => ValueTypeTag::Duration,
    }
}

fn output_slots(output: &FetchShape) -> impl Iterator<Item = &FetchSlot> {
    let slots: Vec<_> = match output {
        FetchShape::Positional { slots } => slots.iter().collect(),
        FetchShape::Named { slots } => slots.iter().map(|named| &named.slot).collect(),
    };
    slots.into_iter()
}

fn request_binding_type(request: &MatchRequest, binding: u16) -> Result<TypeId, Diagnostic> {
    let binding = request
        .plan
        .bindings
        .get(usize::from(binding))
        .ok_or_else(|| {
            integrity(
                "query_v2_adapter_unknown_binding",
                "validated output references no declared binding",
            )
        })?;
    descriptor_type(binding.descriptor.as_str(), binding.thing_kind)
}

fn descriptor_role_id(owner: &str, role: &str) -> Result<RoleId, Diagnostic> {
    let owner = descriptor_id_type(owner)?;
    if owner.kind() != TypeKind::Relation {
        return Err(integrity(
            "query_v2_adapter_role_owner",
            "validated role owner is not relation-kind",
        ));
    }
    RoleId::new(owner.label().as_str().to_owned(), role.to_owned())
}

fn descriptor_id_type(descriptor: &str) -> Result<TypeId, Diagnostic> {
    let (prefix, label) = descriptor.split_once(':').ok_or_else(|| {
        integrity(
            "query_v2_adapter_descriptor_malformed",
            "validated descriptor identity is not kind-qualified",
        )
    })?;
    let kind = match prefix {
        "entity" => TypeKind::Entity,
        "relation" => TypeKind::Relation,
        _ => {
            return Err(integrity(
                "query_v2_adapter_descriptor_malformed",
                "validated descriptor identity has an unsupported kind",
            ));
        }
    };
    TypeId::new(kind, label.to_owned())
}

fn descriptor_type(descriptor: &str, kind: ThingKind) -> Result<TypeId, Diagnostic> {
    let id = descriptor_id_type(descriptor)?;
    let expected = match kind {
        ThingKind::Entity => TypeKind::Entity,
        ThingKind::Relation => TypeKind::Relation,
    };
    if id.kind() == expected {
        Ok(id)
    } else {
        Err(integrity(
            "query_v2_adapter_descriptor_malformed",
            "validated descriptor kind disagrees with the binding thing kind",
        ))
    }
}

fn binding_ordinal(index: usize) -> Result<BindingId, Diagnostic> {
    u16::try_from(index)
        .map_err(|_| {
            reject(
                "query_v2_adapter_binding_limit",
                "adapted binding count exceeds the dense ordinal range",
            )
        })
        .and_then(BindingId::new)
}

fn reject(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static adapter diagnostic code"),
        message,
    )
}

fn integrity(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static adapter diagnostic code"),
        message,
    )
}
