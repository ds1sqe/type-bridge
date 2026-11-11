//! Descriptor-derived authority and hydration metadata shared by typed reads.
use super::{ComparisonOp, MissingOrder, SortDirection, StableOrderSpec, ValidatedMatchRequest};

use super::{BoundFieldId, FetchShape, FetchSlot, MatchExpr, MatchMode, MatchRequest, ThingKind};
use crate::_descriptor::{
    OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptorRef,
};
use crate::_entity::Annotation;
use crate::_registry::{DescriptorRegistry, InstalledSchemaAuthority};
use crate::_schema::{SchemaInfo, generator::try_generate_define_block};
use crate::AttributeValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    HydrationBinding, HydrationDescriptor, HydrationField, HydrationPlayer, HydrationProjection,
    HydrationRole, ModelComparator, ModelField, ModelMissingOrder, ModelNamedOutputSlot,
    ModelOrderDirection, ModelOrderTerm, ModelOutput, ModelOutputSlot, ModelPredicate,
    ModelStableOrder, ModelValue,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, Cardinality, DecimalValue, ValueTypeTag,
};
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, ResolvedType, managed_schema_state, resolve,
};
use type_bridge_schema_compat::released_typeql_to_declared_projection;

const V1_ADAPTER_SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const V1_ADAPTER_MANAGED_SCOPE: &str = "typebridge-v1-descriptor-registry";
const V1_ADAPTER_SCHEMA_DOCUMENT: &str = "typebridge-v1-descriptor-registry.tql";

/// The schema authority one validated request is adapted under.
///
/// Obtaining it is deterministic and performs no provider, filesystem, or
/// transport I/O.
pub(crate) struct MatchRequestAdapterAuthority(Arc<InstalledSchemaAuthority>);

impl MatchRequestAdapterAuthority {
    /// Return the V2 schema authority one registry adapts under.
    ///
    /// A registry installed from a generated package carries the package's
    /// verified authority and uses it as is. A dynamic registry has none, so
    /// its authority is reconstructed from the registered descriptors.
    pub(crate) fn from_registry(registry: &DescriptorRegistry) -> Result<Self, Diagnostic> {
        if let Some(installed) = registry.installed_schema_authority() {
            return Ok(Self(Arc::clone(installed)));
        }
        let schema = SchemaInfo::from_descriptors(&registry.snapshot());
        let source = try_generate_define_block(&schema).map_err(|_| {
            integrity(
                "query_adapter_registry_projection",
                "registered schema cannot render",
            )
        })?;
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
                "query_adapter_registry_projection",
                "validated descriptor registry cannot be projected into V2 schema authority",
            )
        })?;
        let profile = SemanticProfileId::new(V1_ADAPTER_SEMANTIC_PROFILE)?;
        let resolved = resolve(&declared, &profile).map_err(|_| {
            integrity(
                "query_adapter_registry_projection",
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
                "query_adapter_registry_projection",
                "validated descriptor registry cannot form managed V2 schema authority",
            )
        })?;
        Ok(Self(Arc::new(InstalledSchemaAuthority::new(
            declared, resolved, managed,
        ))))
    }

    pub(crate) fn from_query_authority(authority: &crate::plan_authority::QueryAuthority) -> Self {
        Self(authority.adapter_authority())
    }

    pub(crate) fn declared(&self) -> &type_bridge_contract::schema::DeclaredSchema {
        self.0.declared()
    }

    /// Borrow the ordinary validator context for adaptation.
    pub(crate) fn context(&self) -> MigrationAssertionValidationContext<'_> {
        MigrationAssertionValidationContext::new(self.0.resolved(), self.0.managed())
    }
}

/// One literal policy for released and native-plan typed-read adaptation.
pub(crate) fn validate_match_literal_limits(
    predicate: Option<&MatchExpr>,
    limits: QueryLimits,
) -> Result<(), Diagnostic> {
    if let Some(actual) = predicate
        .and_then(|predicate| artifact_sized_literal_bytes(predicate, limits.codec.max_bytes))
    {
        return Err(limits.annotate(
            Diagnostic::new(
                DiagnosticCategory::ResourceLimit,
                DiagnosticCode::new("query_adapter_literal_byte_limit")
                    .expect("static adapter diagnostic code"),
                "a predicate literal exceeds the canonical artifact byte ceiling",
            )
            .with_detail("actual_bytes", byte_count(actual))
            .with_detail("maximum_bytes", byte_count(limits.codec.max_bytes)),
        ));
    }
    let maximum = limits.codec.max_string_bytes.min(limits.codec.max_bytes);
    if let Some(actual) =
        predicate.and_then(|predicate| artifact_sized_literal_bytes(predicate, maximum))
    {
        return Err(limits.annotate_limit(
            Diagnostic::new(
                DiagnosticCategory::ResourceLimit,
                DiagnosticCode::new("query_string_value_byte_limit")
                    .expect("static adapter diagnostic code"),
                "a string value exceeds the configured byte ceiling",
            )
            .with_detail("actual_bytes", byte_count(actual))
            .with_detail("maximum_bytes", byte_count(maximum)),
            "canonical_string_bytes",
        ));
    }
    Ok(())
}

pub(crate) fn build_hydration(
    request: &MatchRequest,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<HydrationProjection, Diagnostic> {
    let mut bindings = Vec::with_capacity(request.plan.bindings.len());
    let mut admitted = BTreeSet::new();
    for binding in &request.plan.bindings {
        let declared = descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?;
        let concrete =
            constructible_closure(schema, &declared, binding.match_mode == MatchMode::Subtypes)?;
        admitted.extend(concrete.iter().cloned());
        bindings.push(HydrationBinding::new(
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
                "query_adapter_schema_mismatch",
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
                            "query_adapter_schema_mismatch",
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
    Ok(HydrationProjection::new(bindings, descriptors))
}

pub(crate) fn hydration_descriptor(
    id: &TypeId,
    hydrate_roles: bool,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<HydrationDescriptor, Diagnostic> {
    let resolved = schema.types().get(id).ok_or_else(|| {
        integrity(
            "query_adapter_schema_mismatch",
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
                    "query_adapter_registry_authority_mismatch",
                    "registered hydration descriptor omits a resolved effective attribute",
                )
            })?;
        let attribute_type = schema
            .types()
            .get(
                &TypeId::new(TypeKind::Attribute, attribute.label().as_str().to_owned()).map_err(
                    |_| {
                        integrity(
                            "query_adapter_schema_mismatch",
                            "resolved attribute label is malformed",
                        )
                    },
                )?,
            )
            .and_then(ResolvedType::value_type)
            .ok_or_else(|| {
                integrity(
                    "query_adapter_schema_mismatch",
                    "resolved owned attribute lacks a scalar value type",
                )
            })?
            .value_type();
        if ValueTypeTag::from(registered.value_type) != attribute_type
            || registered_cardinality(registered)? != owns.cardinality()
            || (registered.is_key() || registered.is_unique())
                != (owns.is_key() || owns.is_unique())
        {
            return Err(integrity(
                "query_adapter_registry_authority_mismatch",
                "registered field type, cardinality, or uniqueness contradicts resolved schema",
            ));
        }
        fields.push(HydrationField::new(
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
                "query_adapter_registry_authority_mismatch",
                "resolved relation is registered as an entity",
            ));
        };
        hydration_roles(id, resolved, &registered, registry, schema)?
    } else {
        Vec::new()
    };
    Ok(HydrationDescriptor::new(id.clone(), fields, roles))
}

pub(crate) fn hydration_roles(
    concrete: &TypeId,
    resolved: &ResolvedType,
    registered: &RelationDescriptor,
    registry: &DescriptorRegistry,
    schema: &ResolvedSchema,
) -> Result<Vec<HydrationRole>, Diagnostic> {
    let mut roles = Vec::with_capacity(resolved.relates().len());
    for (effective_id, effective) in resolved.relates() {
        let registered_role = registered
            .roles
            .iter()
            .find(|role| role.role_name == effective_id.label().as_str())
            .ok_or_else(|| {
                integrity(
                    "query_adapter_registry_authority_mismatch",
                    "registered relation omits a resolved effective role",
                )
            })?;
        if registered_role_cardinality(registered_role)? != effective.cardinality() {
            return Err(integrity(
                "query_adapter_registry_authority_mismatch",
                "registered role cardinality contradicts resolved schema",
            ));
        }
        let accepted = schema
            .roles()
            .get(effective_id)
            .ok_or_else(|| {
                integrity(
                    "query_adapter_schema_mismatch",
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
                Ok(HydrationPlayer::new(declared, concrete))
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
                "query_adapter_registry_authority_mismatch",
                "registered role-player declarations do not cover resolved accepted players",
            ));
        }
        roles.push(
            HydrationRole::new(
                RoleId::new(
                    concrete.label().as_str().to_owned(),
                    effective_id.label().as_str().to_owned(),
                )?,
                role_reference_ids(schema, resolved, effective_id),
                players,
                effective.cardinality(),
                registered_role.ordered,
                registered_role.distinct,
            )
            .with_specialized_roles(effective.replaced_roles().iter().cloned().collect()),
        );
    }
    roles.sort_by(|left, right| left.role().cmp(right.role()));
    Ok(roles)
}

pub(crate) fn adapt_field(
    field: &BoundFieldId,
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<ModelField, Diagnostic> {
    request
        .plan
        .bindings
        .get(usize::from(field.binding.get()))
        .ok_or_else(|| {
            integrity(
                "query_adapter_unknown_binding",
                "validated field references no declared binding",
            )
        })?;
    let attribute = registered_attribute(registry, &field.field)?;
    Ok(ModelField::new(
        BindingId::new(field.binding.get())?,
        descriptor_id_type(field.field.owner.as_str())?,
        AttributeId::new(attribute.attr_name.clone())?,
        ValueTypeTag::from(attribute.value_type),
    ))
}

/// Return the byte length of the first literal larger than a whole artifact.
pub(crate) fn artifact_sized_literal_bytes(
    expression: &MatchExpr,
    maximum: usize,
) -> Option<usize> {
    match expression {
        MatchExpr::FieldValue { value, .. } => textual_value(value)
            .map(str::len)
            .filter(|bytes| *bytes > maximum),
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => expressions
            .iter()
            .find_map(|child| artifact_sized_literal_bytes(child, maximum)),
        MatchExpr::Not { expression } => artifact_sized_literal_bytes(expression, maximum),
        MatchExpr::FieldComparison { .. }
        | MatchExpr::ScalarComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::RoleEdge { .. }
        | MatchExpr::Reachable { .. } => None,
    }
}

pub(crate) fn byte_count(bytes: usize) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

pub(crate) fn textual_value(value: &AttributeValue) -> Option<&str> {
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

pub(crate) fn constructible_closure(
    schema: &ResolvedSchema,
    declared: &TypeId,
    include_subtypes: bool,
) -> Result<Vec<TypeId>, Diagnostic> {
    let resolved = schema.types().get(declared).ok_or_else(|| {
        integrity(
            "query_adapter_schema_mismatch",
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

pub(crate) fn registered_descriptor(
    registry: &DescriptorRegistry,
    id: &TypeId,
) -> Result<TypeDescriptorRef, Diagnostic> {
    let descriptor = registry.get(id.label().as_str()).ok_or_else(|| {
        integrity(
            "query_adapter_registry_authority_mismatch",
            "resolved hydration descriptor has no registered model descriptor",
        )
    })?;
    if registered_type_id(id.label().as_str(), &descriptor)? == *id {
        Ok(descriptor)
    } else {
        Err(integrity(
            "query_adapter_registry_authority_mismatch",
            "registered hydration descriptor kind contradicts resolved schema",
        ))
    }
}

pub(crate) fn registered_type_id(
    name: &str,
    descriptor: &TypeDescriptorRef,
) -> Result<TypeId, Diagnostic> {
    TypeId::new(
        match descriptor {
            TypeDescriptorRef::Entity(_) => TypeKind::Entity,
            TypeDescriptorRef::Relation(_) => TypeKind::Relation,
        },
        name.to_owned(),
    )
}

pub(crate) fn descriptor_attributes(descriptor: &TypeDescriptorRef) -> &[OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

pub(crate) fn registered_attribute(
    registry: &DescriptorRegistry,
    field: &crate::match_request::FieldId,
) -> Result<OwnedAttributeDescriptor, Diagnostic> {
    let owner = registry.descriptor_type_name(&field.owner).ok_or_else(|| {
        integrity(
            "query_adapter_registry_authority_mismatch",
            "validated field owner is no longer registered",
        )
    })?;
    let descriptor = registry.get(&owner).ok_or_else(|| {
        integrity(
            "query_adapter_registry_authority_mismatch",
            "validated field owner is no longer registered",
        )
    })?;
    descriptor_attributes(&descriptor)
        .iter()
        .find(|attribute| attribute.field_name == field.name)
        .cloned()
        .ok_or_else(|| {
            integrity(
                "query_adapter_registry_authority_mismatch",
                "validated field is absent from its registered owner",
            )
        })
}

pub(crate) fn field_reference_owners(
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

pub(crate) fn role_reference_ids(
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

pub(crate) fn registered_cardinality(
    attribute: &OwnedAttributeDescriptor,
) -> Result<Cardinality, Diagnostic> {
    let (minimum, maximum) = attribute.cardinality().unwrap_or(if attribute.is_optional {
        (0, Some(1))
    } else {
        (1, Some(1))
    });
    Cardinality::new(u64::from(minimum), maximum.map(u64::from))
}

pub(crate) fn registered_role_cardinality(
    role: &RoleDescriptor,
) -> Result<Cardinality, Diagnostic> {
    let (minimum, maximum) = role.cardinality.unwrap_or((0, Some(1)));
    Cardinality::new(u64::from(minimum), maximum.map(u64::from))
}

pub(crate) fn output_slots(output: &FetchShape) -> impl Iterator<Item = &FetchSlot> {
    let slots: Vec<_> = match output {
        FetchShape::Positional { slots } => slots.iter().collect(),
        FetchShape::Named { slots } => slots.iter().map(|named| &named.slot).collect(),
    };
    slots.into_iter()
}

pub(crate) fn request_binding_type(
    request: &MatchRequest,
    binding: u16,
) -> Result<TypeId, Diagnostic> {
    let binding = request
        .plan
        .bindings
        .get(usize::from(binding))
        .ok_or_else(|| {
            integrity(
                "query_adapter_unknown_binding",
                "validated output references no declared binding",
            )
        })?;
    descriptor_type(binding.descriptor.as_str(), binding.thing_kind)
}

pub(crate) fn descriptor_role_id(owner: &str, role: &str) -> Result<RoleId, Diagnostic> {
    let owner = descriptor_id_type(owner)?;
    if owner.kind() != TypeKind::Relation {
        return Err(integrity(
            "query_adapter_role_owner",
            "validated role owner is not relation-kind",
        ));
    }
    RoleId::new(owner.label().as_str().to_owned(), role.to_owned())
}

pub(crate) fn descriptor_id_type(descriptor: &str) -> Result<TypeId, Diagnostic> {
    let (prefix, label) = descriptor.split_once(':').ok_or_else(|| {
        integrity(
            "query_adapter_descriptor_malformed",
            "validated descriptor identity is not kind-qualified",
        )
    })?;
    let kind = match prefix {
        "entity" => TypeKind::Entity,
        "relation" => TypeKind::Relation,
        _ => {
            return Err(integrity(
                "query_adapter_descriptor_malformed",
                "validated descriptor identity has an unsupported kind",
            ));
        }
    };
    TypeId::new(kind, label.to_owned())
}

pub(crate) fn descriptor_type(descriptor: &str, kind: ThingKind) -> Result<TypeId, Diagnostic> {
    let id = descriptor_id_type(descriptor)?;
    let expected = match kind {
        ThingKind::Entity => TypeKind::Entity,
        ThingKind::Relation => TypeKind::Relation,
    };
    if id.kind() == expected {
        Ok(id)
    } else {
        Err(integrity(
            "query_adapter_descriptor_malformed",
            "validated descriptor kind disagrees with the binding thing kind",
        ))
    }
}

pub(crate) fn integrity(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static adapter diagnostic code"),
        message,
    )
}

pub(crate) fn adapt_output(
    output: &FetchShape,
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<ModelOutput, Diagnostic> {
    let adapt_slot = |slot: &FetchSlot| -> Result<ModelOutputSlot, Diagnostic> {
        let binding = BindingId::new(slot.binding().get())?;
        let declared = request_binding_type(request, slot.binding().get())?;
        Ok(match slot {
            FetchSlot::One { .. } => ModelOutputSlot::One { binding, declared },
            FetchSlot::Collect { distinct, .. } => {
                let proof = validated.collection_order(slot.binding()).ok_or_else(|| {
                    integrity(
                        "query_adapter_collection_order_proof",
                        "validated collection output lacks its stable-order proof",
                    )
                })?;
                ModelOutputSlot::Collect {
                    binding,
                    declared,
                    distinct: *distinct,
                    order: adapt_stable_order(proof, vec![binding], registry, request)?,
                }
            }
        })
    };
    Ok(match output {
        FetchShape::Positional { slots } => ModelOutput::Positional {
            slots: slots
                .iter()
                .map(adapt_slot)
                .collect::<Result<Vec<_>, _>>()?,
        },
        FetchShape::Named { slots } => ModelOutput::Named {
            slots: slots
                .iter()
                .map(|named| {
                    Ok(ModelNamedOutputSlot::new(
                        named.name.clone(),
                        adapt_slot(&named.slot)?,
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?,
        },
    })
}

pub(crate) fn adapt_stable_order(
    order: &StableOrderSpec,
    identity_tiebreakers: Vec<BindingId>,
    registry: &DescriptorRegistry,
    request: &MatchRequest,
) -> Result<ModelStableOrder, Diagnostic> {
    let terms = order
        .terms()
        .iter()
        .map(|term| {
            let order = term.order();
            Ok(ModelOrderTerm::new(
                adapt_field(&order.field, request, registry)?,
                match order.direction {
                    SortDirection::Ascending => ModelOrderDirection::Ascending,
                    SortDirection::Descending => ModelOrderDirection::Descending,
                },
                match order.missing {
                    MissingOrder::Reject => ModelMissingOrder::Reject,
                    MissingOrder::First => ModelMissingOrder::First,
                    MissingOrder::Last => ModelMissingOrder::Last,
                },
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(ModelStableOrder::new(terms, identity_tiebreakers))
}

pub(crate) fn adapt_expression(
    expression: &MatchExpr,
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<ModelPredicate, Diagnostic> {
    Ok(match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => ModelPredicate::FieldValue {
            field: adapt_field(field, request, registry)?,
            comparator: adapt_comparator(*operator),
            value: adapt_value(value)?,
        },
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => ModelPredicate::FieldComparison {
            left: adapt_field(left, request, registry)?,
            comparator: adapt_comparator(*operator),
            right: adapt_field(right, request, registry)?,
        },
        MatchExpr::ScalarComparison { .. } => {
            return Err(integrity(
                "query_adapter_function_state",
                "schema-function predicates require native plan adaptation",
            ));
        }
        MatchExpr::FieldPresence { field, present } => ModelPredicate::FieldPresence {
            field: adapt_field(field, request, registry)?,
            present: *present,
        },
        MatchExpr::BindingIid { binding, iid } => ModelPredicate::BindingIid {
            binding: BindingId::new(binding.get())?,
            iid: iid.clone(),
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
                        "query_adapter_unknown_binding",
                        "validated role edge references no relation binding",
                    )
                })?;
            ModelPredicate::RoleEdge {
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
            ModelPredicate::Reachable {
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
        MatchExpr::And { expressions } => ModelPredicate::And {
            patterns: expressions
                .iter()
                .map(|child| adapt_expression(child, request, registry))
                .collect::<Result<Vec<_>, _>>()?,
        },
        MatchExpr::Or { expressions } => ModelPredicate::Or {
            patterns: expressions
                .iter()
                .map(|child| adapt_expression(child, request, registry))
                .collect::<Result<Vec<_>, _>>()?,
        },
        MatchExpr::Not { expression } => ModelPredicate::Not {
            pattern: Box::new(adapt_expression(expression, request, registry)?),
        },
    })
}

pub(crate) fn adapt_compatibility_expression(
    expression: &MatchExpr,
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<Option<ModelPredicate>, Diagnostic> {
    match expression {
        MatchExpr::ScalarComparison { .. } => Ok(None),
        MatchExpr::And { expressions } => {
            let patterns = expressions
                .iter()
                .map(|child| adapt_compatibility_expression(child, request, registry))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            Ok(match patterns.len() {
                0 => None,
                1 => patterns.into_iter().next(),
                _ => Some(ModelPredicate::And { patterns }),
            })
        }
        MatchExpr::Or { .. } | MatchExpr::Not { .. }
            if predicate_has_schema_function(expression) =>
        {
            Err(integrity(
                "query_adapter_function_scope",
                "schema-function predicates must remain in the root conjunction",
            ))
        }
        _ => adapt_expression(expression, request, registry).map(Some),
    }
}

pub(crate) fn predicate_has_schema_function(expression: &MatchExpr) -> bool {
    match expression {
        MatchExpr::ScalarComparison { .. } => true,
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            expressions.iter().any(predicate_has_schema_function)
        }
        MatchExpr::Not { expression } => predicate_has_schema_function(expression),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::RoleEdge { .. }
        | MatchExpr::Reachable { .. } => false,
    }
}

pub(crate) fn adapt_comparator(operator: ComparisonOp) -> ModelComparator {
    match operator {
        ComparisonOp::Equal => ModelComparator::Equal,
        ComparisonOp::NotEqual => ModelComparator::NotEqual,
        ComparisonOp::LessThan => ModelComparator::Less,
        ComparisonOp::LessThanOrEqual => ModelComparator::LessOrEqual,
        ComparisonOp::GreaterThan => ModelComparator::Greater,
        ComparisonOp::GreaterThanOrEqual => ModelComparator::GreaterOrEqual,
        ComparisonOp::Contains => ModelComparator::Contains,
        ComparisonOp::StartsWith => ModelComparator::StartsWith,
        ComparisonOp::EndsWith => ModelComparator::EndsWith,
        ComparisonOp::Regex => ModelComparator::Regex,
    }
}

pub(crate) fn adapt_value(value: &AttributeValue) -> Result<ModelValue, Diagnostic> {
    let malformed = || {
        integrity(
            "query_adapter_validated_literal_malformed",
            "a validated V1 literal no longer satisfies its released scalar domain",
        )
    };
    let canonical = match value {
        AttributeValue::String(value) => match CanonicalString::new(value.as_str()) {
            Ok(value) => Some(CanonicalValue::String(value)),
            Err(_) => return ModelValue::released_string(value.clone()),
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
            Err(_) => return ModelValue::released_datetime(value.clone()),
        },
        AttributeValue::DateTimeTZ(value) => match value.parse::<CanonicalDateTimeTz>() {
            Ok(value) => Some(CanonicalValue::DateTimeTz(value)),
            Err(_) => return ModelValue::released_datetime_tz(value.clone()),
        },
        AttributeValue::Decimal(value) => {
            let canonical = DecimalValue::new(value.as_str()).map_err(|_| malformed())?;
            if canonical.as_str() != value {
                return ModelValue::released_decimal(value.clone());
            }
            Some(CanonicalValue::Decimal(canonical))
        }
        AttributeValue::Duration(value) => match value.parse::<CanonicalDuration>() {
            Ok(value) => Some(CanonicalValue::Duration(value)),
            Err(_) => return ModelValue::released_duration(value.clone()),
        },
    };
    Ok(ModelValue::canonical(canonical.ok_or_else(|| {
        integrity(
            "query_adapter_literal_state",
            "validated literal adaptation produced no canonical or released value",
        )
    })?))
}

pub(crate) fn adapt_reducers(
    reducers: &[crate::match_request::ReduceTerm],
    request: &MatchRequest,
    registry: &DescriptorRegistry,
) -> Result<Vec<type_bridge_contract::query_plan::ModelReductionTerm>, Diagnostic> {
    reducers
        .iter()
        .map(|term| {
            let reduction = match term.reduction {
                crate::match_request::Reduction::Count => {
                    type_bridge_contract::query_plan::ModelReductionKind::Count
                }
                crate::match_request::Reduction::Sum => {
                    type_bridge_contract::query_plan::ModelReductionKind::Sum
                }
                crate::match_request::Reduction::Min => {
                    type_bridge_contract::query_plan::ModelReductionKind::Min
                }
                crate::match_request::Reduction::Max => {
                    type_bridge_contract::query_plan::ModelReductionKind::Max
                }
                crate::match_request::Reduction::Mean => {
                    type_bridge_contract::query_plan::ModelReductionKind::Mean
                }
                crate::match_request::Reduction::Median => {
                    type_bridge_contract::query_plan::ModelReductionKind::Median
                }
                crate::match_request::Reduction::Std => {
                    type_bridge_contract::query_plan::ModelReductionKind::Std
                }
            };
            Ok(type_bridge_contract::query_plan::ModelReductionTerm::new(
                reduction,
                term.input
                    .as_ref()
                    .map(|field| adapt_field(field, request, registry))
                    .transpose()?,
            ))
        })
        .collect()
}

pub(crate) fn document_roles(
    descriptor: &type_bridge_contract::query_plan::HydrationDescriptor,
    authority: &MatchRequestAdapterAuthority,
) -> Result<Vec<type_bridge_contract::query_plan::DocumentRole>, Diagnostic> {
    use type_bridge_contract::schema::{RelatesFactId, SchemaFactId};
    descriptor
        .roles()
        .iter()
        .map(|role| {
            let declared = role
                .reference_roles()
                .iter()
                .find(|candidate| {
                    TypeId::new(
                        type_bridge_contract::id::TypeKind::Relation,
                        candidate.declaring_relation().as_str(),
                    )
                    .ok()
                    .and_then(|owner| RelatesFactId::new(owner, (*candidate).clone()).ok())
                    .is_some_and(|id| {
                        authority
                            .declared()
                            .fact(&SchemaFactId::Relates(id))
                            .is_some()
                    })
                })
                .ok_or_else(|| {
                    integrity(
                        "query_plan_adapter_role",
                        "effective role has no declared schema token",
                    )
                })?;
            Ok(type_bridge_contract::query_plan::DocumentRole::new(
                declared.clone(),
                false,
            ))
        })
        .collect()
}
