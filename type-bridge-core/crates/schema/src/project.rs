use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, CompleteReadProjection, CreateFieldProjection,
    CreateProjection, CreateRoleProjection, DeclarationProjection, DeclaredRoleProjection,
    DirectSubProjection, EmissionPlan, FieldTokenProjection, FunctionParameterProjection,
    FunctionProjection, FunctionReturnElementProjection, FunctionReturnProjection, ModelProjection,
    PlayingProjection, ProjectedAnnotation, ProjectedModelForm, ProjectedModelUse,
    ProjectedMultiplicity, ProjectedTypeRef, ProjectionConfig, ProjectionHandler,
    QueryTokenProjection, ReadFieldProjection, ReadRoleProjection, ReferenceReadProjection,
    RoleTokenProjection, RuntimeProjection, RustCreatePolicy, StructFieldProjection,
    StructProjection, TargetIdentifier,
};
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, FunctionReturnMode, OwnsFactId,
    RelatesFactId, SchemaAnnotationValue, SchemaDiagnostic, SchemaDiagnostics, TypeReference,
    ValueFactId,
};

use crate::resolve::{EffectiveRelates, ResolvedSchema, ResolvedType};

fn no_source(error: Diagnostic) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(error, None))
}

fn projection_error(code: &'static str, message: impl Into<String>) -> SchemaDiagnostics {
    crate::diagnostic::diagnostic(DiagnosticCategory::InvalidContract, code, message, None)
}

fn python_class_name(label: &str) -> String {
    label
        .replace('_', "-")
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut output = first.to_uppercase().collect::<String>();
                    let rest = chars.as_str();
                    let uniform = part.chars().all(char::is_uppercase)
                        || part.chars().all(char::is_lowercase);
                    if uniform {
                        output.push_str(&rest.to_lowercase());
                    } else {
                        output.push_str(rest);
                    }
                    output
                }
                None => String::new(),
            }
        })
        .collect()
}

fn python_member_name(label: &str) -> String {
    label.replace('-', "_")
}

fn typescript_member_name(label: &str) -> String {
    let class_name = python_class_name(label);
    let mut chars = class_name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

fn rust_member_name(label: &str) -> String {
    label.replace('-', "_").to_ascii_lowercase()
}

fn python_identifier(value: String) -> Result<TargetIdentifier, SchemaDiagnostics> {
    if matches!(value.as_str(), "iid" | "model_config" | "model_fields")
        || value.starts_with("__") && value.ends_with("__")
    {
        return Err(projection_error(
            "reserved_python_projection_identifier",
            "projected Python name collides with generated runtime state",
        ));
    }
    TargetIdentifier::python(value).map_err(no_source)
}

fn typescript_identifier(value: String) -> Result<TargetIdentifier, SchemaDiagnostics> {
    if matches!(
        value.as_str(),
        "iid"
            | "typeToken"
            | "fields"
            | "roles"
            | "plays"
            | "create"
            | "reference"
            | "prototype"
            | "__proto__"
    ) {
        return Err(projection_error(
            "reserved_typescript_projection_identifier",
            "projected TypeScript name collides with generated runtime state",
        ));
    }
    TargetIdentifier::typescript(value).map_err(no_source)
}

fn rust_identifier(value: String) -> Result<TargetIdentifier, SchemaDiagnostics> {
    if matches!(
        value.as_str(),
        "iid"
            | "type_token"
            | "fields"
            | "roles"
            | "plays"
            | "create"
            | "reference"
            | "try_new"
            | "insert"
    ) {
        return Err(projection_error(
            "reserved_rust_projection_identifier",
            "projected Rust name collides with generated runtime state",
        ));
    }
    TargetIdentifier::rust(value).map_err(no_source)
}

fn class_identifier(
    target: BindingTarget,
    label: &str,
) -> Result<TargetIdentifier, SchemaDiagnostics> {
    let value = python_class_name(label);
    match target {
        BindingTarget::Python => python_identifier(value),
        BindingTarget::TypeScript => typescript_identifier(value),
        BindingTarget::Rust => rust_identifier(value),
    }
}

fn member_identifier(
    target: BindingTarget,
    label: &str,
) -> Result<TargetIdentifier, SchemaDiagnostics> {
    match target {
        BindingTarget::Python => python_identifier(python_member_name(label)),
        BindingTarget::TypeScript => typescript_identifier(typescript_member_name(label)),
        BindingTarget::Rust => rust_identifier(rust_member_name(label)),
    }
}

fn reference_identifier(
    target: BindingTarget,
    name: &TargetIdentifier,
) -> Result<TargetIdentifier, SchemaDiagnostics> {
    match target {
        BindingTarget::Python => python_identifier(format!("{}Ref", name.as_str())),
        BindingTarget::TypeScript => typescript_identifier(format!("{}Ref", name.as_str())),
        BindingTarget::Rust => rust_identifier(format!("{}Ref", name.as_str())),
    }
}

#[derive(Default)]
struct NameRegistry {
    names: BTreeMap<(String, String), String>,
}

impl NameRegistry {
    fn insert(
        &mut self,
        namespace: impl Into<String>,
        name: &TargetIdentifier,
        identity: impl Into<String>,
    ) -> Result<(), SchemaDiagnostics> {
        let key = (namespace.into(), name.as_str().to_owned());
        let identity = identity.into();
        if let Some(previous) = self.names.get(&key) {
            if previous != &identity {
                return Err(projection_error(
                    "projection_name_collision",
                    format!(
                        "projected name `{}` in namespace `{}` collides between `{}` and `{}`",
                        key.1, key.0, previous, identity
                    ),
                ));
            }
        } else {
            self.names.insert(key, identity);
        }
        Ok(())
    }
}

fn annotations(
    subject: AnnotationSubjectId,
    values: &BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
) -> Result<BTreeMap<AnnotationFactId, ProjectedAnnotation>, SchemaDiagnostics> {
    values
        .iter()
        .map(|(kind, value)| {
            let id = AnnotationFactId::new(subject.clone(), kind.clone());
            ProjectedAnnotation::new(id.clone(), value.clone())
                .map(|annotation| (id, annotation))
                .map_err(no_source)
        })
        .collect()
}

fn effective_relates_annotation_subject(
    relates: &EffectiveRelates,
) -> Result<AnnotationSubjectId, SchemaDiagnostics> {
    let role = RoleId::new(
        relates.id().relation().label().as_str(),
        relates.id().role().label().as_str(),
    )
    .map_err(no_source)?;
    RelatesFactId::new(relates.id().relation().clone(), role)
        .map(AnnotationSubjectId::Relates)
        .map_err(no_source)
}

fn ordered_owns(resolved: &ResolvedType) -> Vec<OwnsFactId> {
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for attribute in resolved.owned_attribute_order() {
        if let Some(owns) = resolved.owns().get(attribute) {
            seen.insert(attribute.clone());
            ordered.push(owns.id().clone());
        }
    }
    ordered.extend(
        resolved
            .owns()
            .iter()
            .filter(|(attribute, _)| !seen.contains(*attribute))
            .map(|(_, owns)| owns.id().clone()),
    );
    ordered
}

fn read_model_use(id: &TypeId) -> Result<ProjectedModelUse, SchemaDiagnostics> {
    let form = match id.kind() {
        TypeKind::Entity => ProjectedModelForm::Complete,
        TypeKind::Relation => ProjectedModelForm::Reference,
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(projection_error(
                "invalid_projection_reference",
                "only entity and relation types can be projected as role players",
            ));
        }
    };
    Ok(ProjectedModelUse::new(id.clone(), form))
}

fn create_model_uses(id: &TypeId) -> Result<BTreeSet<ProjectedModelUse>, SchemaDiagnostics> {
    match id.kind() {
        TypeKind::Entity => Ok(BTreeSet::from([ProjectedModelUse::new(
            id.clone(),
            ProjectedModelForm::Complete,
        )])),
        TypeKind::Relation => Ok(BTreeSet::from([
            ProjectedModelUse::new(id.clone(), ProjectedModelForm::Complete),
            ProjectedModelUse::new(id.clone(), ProjectedModelForm::Reference),
        ])),
        TypeKind::Attribute | TypeKind::Struct => Err(projection_error(
            "invalid_projection_reference",
            "only entity and relation types can be projected as role players",
        )),
    }
}

fn immediate_specialization(
    resolved: &ResolvedType,
    relates: &EffectiveRelates,
) -> Result<Option<RoleId>, SchemaDiagnostics> {
    if relates.replaced_roles().is_empty() {
        return Ok(None);
    }
    let mut ranked = relates
        .replaced_roles()
        .iter()
        .filter_map(|role| {
            resolved
                .supertypes()
                .iter()
                .position(|parent| parent.label() == role.declaring_relation())
                .map(|distance| (distance, role.clone()))
        })
        .collect::<Vec<_>>();
    ranked.sort();
    let Some((distance, role)) = ranked.first().cloned() else {
        return Err(projection_error(
            "invalid_projection_reference",
            "specialized role has no declaring relation in the resolved ancestor chain",
        ));
    };
    if ranked.get(1).is_some_and(|next| next.0 == distance) {
        return Err(projection_error(
            "ambiguous_role_specialization_projection",
            "more than one replaced role is nearest to the specializing relation",
        ));
    }
    Ok(Some(role))
}

fn resolve_type_reference(
    reference: &TypeReference,
    resolved: &ResolvedSchema,
) -> Result<ProjectedTypeRef, SchemaDiagnostics> {
    match reference {
        TypeReference::Value(value) => Ok(ProjectedTypeRef::Scalar(*value)),
        TypeReference::Schema(label) => {
            let models = resolved
                .types()
                .keys()
                .filter(|id| id.label() == label)
                .cloned()
                .collect::<Vec<_>>();
            let structures = resolved
                .structs()
                .keys()
                .filter(|id| id.label() == label)
                .cloned()
                .collect::<Vec<_>>();
            match (models.as_slice(), structures.as_slice()) {
                ([model], []) => Ok(ProjectedTypeRef::Model(ProjectedModelUse::new(
                    model.clone(),
                    ProjectedModelForm::Complete,
                ))),
                ([], [structure]) => Ok(ProjectedTypeRef::Struct(structure.clone())),
                ([], []) => Err(projection_error(
                    "unknown_projection_type_reference",
                    "function signature references an unknown projected type",
                )),
                _ => Err(projection_error(
                    "ambiguous_projection_type_reference",
                    "function signature label resolves to more than one projected type",
                )),
            }
        }
    }
}

fn function_return(
    returns: &FunctionReturnMode,
    resolved: &ResolvedSchema,
) -> Result<FunctionReturnProjection, SchemaDiagnostics> {
    let project = |element: &type_bridge_contract::schema::FunctionReturnElement| {
        Ok(FunctionReturnElementProjection::new(
            resolve_type_reference(element.type_ref(), resolved)?,
            element.optional(),
        ))
    };
    match returns {
        FunctionReturnMode::Scalar(element) => {
            Ok(FunctionReturnProjection::Scalar(project(element)?))
        }
        FunctionReturnMode::Tuple(elements) => elements
            .iter()
            .map(project)
            .collect::<Result<Vec<_>, _>>()
            .map(FunctionReturnProjection::Tuple),
        FunctionReturnMode::Stream(elements) => elements
            .iter()
            .map(project)
            .collect::<Result<Vec<_>, _>>()
            .map(FunctionReturnProjection::Stream),
    }
}

fn link_components(resolved: &ResolvedSchema) -> Result<Vec<BTreeSet<TypeId>>, SchemaDiagnostics> {
    let components = resolved.dependency_graph().strongly_connected_components();
    let mut membership = BTreeMap::new();
    for (index, component) in components.iter().enumerate() {
        for member in component {
            if membership.insert(member.clone(), index).is_some() {
                return Err(projection_error(
                    "invalid_projection_emission_plan",
                    "resolver SCC partition repeats a model identity",
                ));
            }
        }
    }
    if membership.len() != resolved.types().len()
        || resolved
            .types()
            .keys()
            .any(|id| !membership.contains_key(id))
    {
        return Err(projection_error(
            "invalid_projection_emission_plan",
            "resolver SCC partition does not cover every model",
        ));
    }
    let mut dependents = vec![BTreeSet::<usize>::new(); components.len()];
    let mut indegree = vec![0usize; components.len()];
    for id in resolved.types().keys() {
        let source = membership[id];
        for dependency in resolved
            .dependency_graph()
            .dependencies(id)
            .into_iter()
            .flatten()
        {
            let target = membership[dependency];
            if source != target && dependents[target].insert(source) {
                indegree[source] += 1;
            }
        }
    }
    let minimum = components
        .iter()
        .map(|component| {
            component
                .first()
                .cloned()
                .expect("resolver SCCs are non-empty")
        })
        .collect::<Vec<_>>();
    let mut ready = indegree
        .iter()
        .enumerate()
        .filter(|(_, degree)| **degree == 0)
        .map(|(index, _)| (minimum[index].clone(), index))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(components.len());
    while let Some((_, index)) = ready.pop_first() {
        ordered.push(components[index].clone());
        for dependent in &dependents[index] {
            indegree[*dependent] -= 1;
            if indegree[*dependent] == 0 {
                ready.insert((minimum[*dependent].clone(), *dependent));
            }
        }
    }
    if ordered.len() != components.len() {
        return Err(projection_error(
            "invalid_projection_emission_plan",
            "resolver SCC condensation graph is cyclic",
        ));
    }
    Ok(ordered)
}

/// Derive a target-specific runtime projection without provider I/O or mutation.
pub fn project(
    resolved: &ResolvedSchema,
    target: BindingTarget,
    config: &ProjectionConfig,
    handlers: &[ProjectionHandler],
    resources: &[CodeResourceDigest],
) -> Result<RuntimeProjection, SchemaDiagnostics> {
    if config.target() != target {
        return Err(projection_error(
            "projection_config_target_mismatch",
            "projection configuration belongs to a different target",
        ));
    }
    let mut names = NameRegistry::default();
    let mut models = BTreeMap::new();
    let mut playing_facts = BTreeMap::new();

    for (id, resolved_type) in resolved.types() {
        let target_name = class_identifier(target, id.label().as_str())?;
        names.insert("root", &target_name, format!("model:{id:?}"))?;
        let reference_name = if matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
            let reference = reference_identifier(target, &target_name)?;
            names.insert("root", &reference, format!("reference:{id:?}"))?;
            Some(reference)
        } else {
            None
        };
        let query_token_name = if target == BindingTarget::Rust {
            let name = rust_identifier(format!("{}Type", target_name.as_str()))?;
            names.insert("root", &name, format!("query-token:{id:?}"))?;
            Some(name)
        } else {
            None
        };
        let type_annotations = annotations(
            AnnotationSubjectId::Type(id.clone()),
            resolved_type.annotations(),
        )?;
        let ordered_field_ids = ordered_owns(resolved_type);
        let mut field_tokens = BTreeMap::new();
        let mut direct_fields = Vec::new();
        let mut create_fields = Vec::new();
        let mut read_fields = Vec::new();
        for owns_id in &ordered_field_ids {
            let owns = resolved_type
                .owns()
                .get(owns_id.attribute())
                .ok_or_else(|| {
                    projection_error(
                        "invalid_projection_reference",
                        "ordered ownership is absent from the effective ownership map",
                    )
                })?;
            let name = member_identifier(target, owns.id().attribute().label().as_str())?;
            names.insert(
                format!("model:{id:?}"),
                &name,
                format!("field:{:?}", owns.id()),
            )?;
            let multiplicity = ProjectedMultiplicity::from_cardinality(owns.cardinality());
            let token = FieldTokenProjection::new(
                owns.id().clone(),
                name,
                multiplicity,
                owns.is_key(),
                owns.is_unique(),
                annotations(
                    AnnotationSubjectId::Owns(owns.id().clone()),
                    owns.annotations(),
                )?,
            )
            .map_err(no_source)?;
            let attribute_model =
                TypeId::new(TypeKind::Attribute, owns.id().attribute().label().as_str())
                    .map_err(no_source)?;
            let value = ProjectedTypeRef::Model(ProjectedModelUse::new(
                attribute_model,
                ProjectedModelForm::Complete,
            ));
            if owns.origin().is_direct() {
                direct_fields.push(owns.id().clone());
            }
            create_fields.push(CreateFieldProjection::new(
                owns.id().clone(),
                value.clone(),
                multiplicity,
            ));
            read_fields.push(ReadFieldProjection::new(
                owns.id().clone(),
                value,
                multiplicity,
            ));
            field_tokens.insert(owns.id().clone(), token);
        }

        let replaced = resolved_type
            .relates()
            .values()
            .flat_map(|relates| relates.replaced_roles().iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut role_tokens = BTreeMap::new();
        let mut direct_roles = BTreeMap::new();
        let mut create_roles = BTreeMap::new();
        let mut read_roles = BTreeMap::new();
        let mut create_enabled = resolved_type.is_constructible() && !resolved_type.is_abstract();
        for (role_id, relates) in resolved_type.relates() {
            if replaced.contains(role_id) {
                continue;
            }
            let name = member_identifier(target, role_id.label().as_str())?;
            names.insert(format!("model:{id:?}"), &name, format!("role:{role_id:?}"))?;
            let resolved_role = resolved.roles().get(role_id).ok_or_else(|| {
                projection_error(
                    "invalid_projection_reference",
                    "effective relates role is absent from the resolved role index",
                )
            })?;
            let specializes = immediate_specialization(resolved_type, relates)?;
            let multiplicity = ProjectedMultiplicity::from_cardinality(relates.cardinality());
            let player_union_name = if target == BindingTarget::Rust {
                let name = rust_identifier(format!(
                    "{}{}Player",
                    target_name.as_str(),
                    python_class_name(role_id.label().as_str()),
                ))?;
                names.insert("root", &name, format!("player-union:{id:?}:{role_id:?}"))?;
                Some(name)
            } else {
                None
            };
            let mut token = RoleTokenProjection::new(
                id.clone(),
                role_id.clone(),
                name,
                resolved_role.accepted_players().clone(),
                specializes.clone(),
                multiplicity,
                relates.is_abstract(),
                annotations(
                    effective_relates_annotation_subject(relates)?,
                    relates.annotations(),
                )?,
            )
            .map_err(no_source)?;
            if let Some(name) = player_union_name {
                token = token.with_player_union_target_name(name);
            }
            let read_players = resolved_role
                .accepted_players()
                .iter()
                .map(read_model_use)
                .collect::<Result<BTreeSet<_>, _>>()?;
            let mut create_players = BTreeSet::new();
            for player in resolved_role.accepted_players() {
                create_players.extend(create_model_uses(player)?);
            }
            if relates.origin().is_direct() {
                direct_roles.insert(
                    role_id.clone(),
                    DeclaredRoleProjection::new(role_id.clone(), specializes),
                );
            }
            if !relates.is_abstract() {
                if create_players.is_empty() && multiplicity.required() {
                    create_enabled = false;
                } else if !create_players.is_empty() {
                    create_roles.insert(
                        role_id.clone(),
                        CreateRoleProjection::new(role_id.clone(), create_players, multiplicity)
                            .map_err(no_source)?,
                    );
                }
            }
            read_roles.insert(
                role_id.clone(),
                ReadRoleProjection::new(role_id.clone(), read_players, multiplicity)
                    .map_err(no_source)?,
            );
            role_tokens.insert(role_id.clone(), token);
        }

        let mut direct_plays = BTreeSet::new();
        for plays in resolved_type.plays().values() {
            if plays.origin().is_direct() {
                direct_plays.insert(plays.id().clone());
            }
            let plays_name = member_identifier(
                target,
                &format!(
                    "plays-{}-{}-{}",
                    id.label(),
                    plays.id().role().declaring_relation(),
                    plays.id().role().label()
                ),
            )?;
            names.insert("root", &plays_name, format!("plays:{:?}", plays.id()))?;
            let projected = PlayingProjection::new(
                plays.id().clone(),
                plays.id().role().clone(),
                ProjectedMultiplicity::from_cardinality(plays.cardinality()),
                annotations(
                    AnnotationSubjectId::Plays(plays.id().clone()),
                    plays.annotations(),
                )?,
            )
            .map_err(no_source)?
            .with_target_name(plays_name);
            if playing_facts
                .insert(plays.id().clone(), projected)
                .is_some()
            {
                return Err(projection_error(
                    "invalid_runtime_projection_map",
                    "an effective playing identity appears more than once",
                ));
            }
        }

        let key_fields = ordered_field_ids
            .iter()
            .filter(|owns| resolved_type.key_attributes().contains(owns.attribute()))
            .cloned()
            .collect();
        let value_annotations = if let Some(value) = resolved_type.value_type() {
            let attribute = AttributeId::new(id.label().as_str()).map_err(no_source)?;
            annotations(
                AnnotationSubjectId::Value(ValueFactId::new(attribute)),
                value.annotations(),
            )?
        } else {
            BTreeMap::new()
        };
        let direct_sub = resolved_type
            .direct_sub()
            .map(|sub| {
                let annotations = annotations(
                    AnnotationSubjectId::Sub(sub.id().clone()),
                    sub.annotations(),
                )?;
                DirectSubProjection::new(
                    sub.id().clone(),
                    sub.origin().declared().clone(),
                    annotations,
                )
                .map_err(no_source)
            })
            .transpose()?;
        let declaration = DeclarationProjection::new(
            resolved_type.supertypes().first().cloned(),
            resolved_type.value_type().map(|value| value.value_type()),
            resolved_type.is_abstract(),
            resolved_type.is_constructible(),
            type_annotations,
            direct_fields,
            direct_roles,
            direct_plays,
        )
        .map_err(no_source)?
        .with_direct_sub(direct_sub)
        .map_err(no_source)?
        .with_value_annotations(value_annotations)
        .map_err(no_source)?;
        let create_target_name = if target == BindingTarget::Rust && create_enabled {
            if config.rust_create_policy() != Some(RustCreatePolicy::ValidatedInputV1) {
                return Err(projection_error(
                    "unsupported_rust_create_policy",
                    "Rust projection does not recognize the configured create policy",
                ));
            }
            let name = rust_identifier(format!("{}Create", target_name.as_str()))?;
            names.insert("root", &name, format!("create:{id:?}"))?;
            Some(name)
        } else {
            None
        };
        let mut create = CreateProjection::new(create_enabled, create_fields, create_roles)
            .map_err(no_source)?;
        if let Some(name) = create_target_name {
            create = create.with_target_name(name);
        }
        let role_upcasts = resolved_type
            .relates()
            .iter()
            .filter_map(|(role, relates)| {
                if relates.replaced_roles().is_empty() {
                    return None;
                }
                let mut ancestors = relates.replaced_roles().iter().cloned().collect::<Vec<_>>();
                ancestors.sort_by_key(|ancestor| {
                    resolved_type
                        .supertypes()
                        .iter()
                        .position(|parent| parent.label() == ancestor.declaring_relation())
                        .unwrap_or(usize::MAX)
                });
                Some((role.clone(), ancestors))
            })
            .collect();
        let complete_read = CompleteReadProjection::new(
            read_fields,
            read_roles,
            resolved_type.supertypes().to_vec(),
        )
        .map_err(no_source)?
        .with_role_upcasts(role_upcasts)
        .map_err(no_source)?;
        let reference_read =
            ReferenceReadProjection::new(reference_name, key_fields).map_err(no_source)?;
        let mut query_tokens =
            QueryTokenProjection::new(id.clone(), field_tokens, role_tokens).map_err(no_source)?;
        if let Some(name) = query_token_name {
            query_tokens = query_tokens.with_target_name(name);
        }
        let model = ModelProjection::new(
            id.clone(),
            target_name,
            declaration,
            create,
            complete_read,
            reference_read,
            query_tokens,
        )
        .map_err(no_source)?;
        models.insert(id.clone(), model);
    }

    let mut structs = BTreeMap::new();
    for (id, structure) in resolved.structs() {
        let target_name = class_identifier(target, id.label().as_str())?;
        names.insert("root", &target_name, format!("struct:{id:?}"))?;
        let mut fields = Vec::new();
        for field in structure.fields() {
            let target_field = member_identifier(target, field.name().as_str())?;
            names.insert(
                format!("struct:{id:?}"),
                &target_field,
                format!("field:{:?}", field.name()),
            )?;
            fields.push(StructFieldProjection::new(
                field.name().clone(),
                target_field,
                field.value_type(),
                field.optional(),
            ));
        }
        structs.insert(
            id.clone(),
            StructProjection::new(id.clone(), target_name, fields).map_err(no_source)?,
        );
    }

    let mut functions = BTreeMap::new();
    for (id, function) in resolved.functions() {
        let target_name = member_identifier(target, id.label().as_str())?;
        names.insert("root", &target_name, format!("function:{id:?}"))?;
        let mut parameters = Vec::new();
        for parameter in function.declaration().signature().parameters() {
            let target_parameter = member_identifier(target, parameter.name().as_str())?;
            names.insert(
                format!("function:{id:?}"),
                &target_parameter,
                format!("parameter:{:?}", parameter.name()),
            )?;
            parameters.push(FunctionParameterProjection::new(
                parameter.name().clone(),
                target_parameter,
                resolve_type_reference(parameter.type_ref(), resolved)?,
            ));
        }
        functions.insert(
            id.clone(),
            FunctionProjection::new(
                id.clone(),
                target_name,
                parameters,
                function_return(function.declaration().signature().returns(), resolved)?,
            )
            .map_err(no_source)?
            .with_annotations(annotations(
                AnnotationSubjectId::Function(id.clone()),
                function.annotations(),
            )?)
            .map_err(no_source)?,
        );
    }

    let mut shells = resolved.types().values().collect::<Vec<_>>();
    shells.sort_by_key(|model| (model.supertypes().len(), model.id().clone()));
    let emission = EmissionPlan::new(
        shells.into_iter().map(|model| model.id().clone()).collect(),
        link_components(resolved)?,
        structs.keys().cloned().collect(),
        functions.keys().cloned().collect(),
    )
    .map_err(no_source)?;
    RuntimeProjection::try_new(
        target,
        config.clone(),
        resolved.semantic_fingerprint().clone(),
        handlers,
        resources,
        models,
        structs,
        functions,
        playing_facts,
        emission,
    )
    .map_err(no_source)
}
