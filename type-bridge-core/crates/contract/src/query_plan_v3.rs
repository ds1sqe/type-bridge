//! Additive canonical read/write plans. V1/V2 representations remain frozen.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::*;
use crate::codec::from_canonical_json_with_limits;
use crate::id::TypeKind;
use crate::migration_assertion_wire::{FingerprintWire, RoleIdWire, TypeIdWire};
use crate::query_plan_wire::{
    InputColumnWire, LocalFunctionWire, QueryBindingWire, QueryOperandWire, rebuild_output,
    rebuild_read_stage_v2,
};
use crate::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema, OwnsFactId,
    PlaysFactId, RelatesFactId, SchemaFact, SchemaFactId,
};

/// Exact discriminator of the additive write-plan format.
pub const QUERY_PLAN_FORMAT_V3: &str = "typebridge.query-plan/v3";
/// Canonicalization version separating V3 fingerprints from frozen V2.
pub const QUERY_PLAN_CANONICALIZATION_V3: &str = "typebridge.query-plan-c14n/v3";

/// A thing created or matched by insert/put. Field and player order is retained.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InsertThing {
    /// Fresh dense binding assigned by this stage.
    pub binding: BindingId,
    /// Concrete entity or relation type.
    pub type_id: TypeId,
    /// Declared ownerships and their scalar operands.
    pub fields: Vec<(OwnsFactId, QueryOperand)>,
    /// Declared roles and player bindings.
    pub players: Vec<(RoleId, BindingId)>,
}

/// One update assignment, evaluated inside the same pipeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Set {
    /// Replace an ownership value.
    Field {
        /// Bound owner.
        owner: BindingId,
        /// Declared ownership; key fields cannot be replaced.
        field: OwnsFactId,
        /// New scalar value.
        operand: QueryOperand,
    },
    /// Assign a player to a declared role.
    Link {
        /// Bound relation.
        relation: BindingId,
        /// Declared role.
        role: RoleId,
        /// Bound player.
        player: BindingId,
    },
}

/// One deletion within a pipeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeleteItem {
    /// Delete a thing and end its binding's lifetime.
    Thing {
        /// Thing to delete.
        binding: BindingId,
    },
    /// Delete an ownership without deleting the owner.
    Has {
        /// Bound owner.
        owner: BindingId,
        /// Declared ownership.
        field: OwnsFactId,
    },
    /// Remove one role player.
    Links {
        /// Bound relation.
        relation: BindingId,
        /// Declared role.
        role: RoleId,
        /// Bound player.
        player: BindingId,
    },
}

/// Ordered V3 pipeline vocabulary. Read tails retain the V2 order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stage {
    /// Initial closed match conjunction.
    Match {
        /// Typed read patterns.
        patterns: Vec<QueryPattern>,
    },
    /// Create fresh things.
    Insert {
        /// Things created together, in source order.
        things: Vec<InsertThing>,
    },
    /// Replace fields or role players.
    Update {
        /// Assignments in source order.
        sets: Vec<Set>,
    },
    /// Delete things, ownerships or role players.
    Delete {
        /// Deletions in source order.
        items: Vec<DeleteItem>,
    },
    /// Match or create things.
    Put {
        /// Things matched or created together.
        things: Vec<InsertThing>,
    },
    /// Restrict the visible row environment.
    Select {
        /// Strictly ascending binding set.
        bindings: Vec<BindingId>,
    },
    /// Require visible bindings.
    Require {
        /// Strictly ascending binding set.
        bindings: Vec<BindingId>,
    },
    /// Deduplicate visible rows.
    Distinct,
    /// Group and reduce rows.
    Reduce {
        /// Fresh reducer assignments.
        assignments: Vec<ReduceAssignment>,
        /// Strictly ascending group keys.
        groups: Vec<BindingId>,
    },
    /// Impose a total order.
    Sort {
        /// Ordered sort terms.
        terms: Vec<OrderTerm>,
    },
    /// Skip sorted rows.
    Offset {
        /// Number of rows to skip.
        rows: u64,
    },
    /// Limit sorted rows.
    Limit {
        /// Maximum number of rows.
        rows: u64,
    },
}

impl From<ReadStage> for Stage {
    fn from(stage: ReadStage) -> Self {
        match stage {
            ReadStage::Match { patterns } => Self::Match { patterns },
            ReadStage::Select { bindings } => Self::Select { bindings },
            ReadStage::Require { bindings } => Self::Require { bindings },
            ReadStage::Distinct => Self::Distinct,
            ReadStage::Reduce {
                assignments,
                groups,
            } => Self::Reduce {
                assignments,
                groups,
            },
            ReadStage::Sort { terms } => Self::Sort { terms },
            ReadStage::Offset { rows } => Self::Offset { rows },
            ReadStage::Limit { rows } => Self::Limit { rows },
        }
    }
}

impl Stage {
    /// Whether this stage mutates provider data.
    #[must_use]
    pub const fn is_write(&self) -> bool {
        matches!(
            self,
            Self::Insert { .. } | Self::Update { .. } | Self::Delete { .. } | Self::Put { .. }
        )
    }

    fn read_stage(&self) -> Option<ReadStage> {
        Some(match self {
            Self::Match { patterns } => ReadStage::Match {
                patterns: patterns.clone(),
            },
            Self::Select { bindings } => ReadStage::Select {
                bindings: bindings.clone(),
            },
            Self::Require { bindings } => ReadStage::Require {
                bindings: bindings.clone(),
            },
            Self::Distinct => ReadStage::Distinct,
            Self::Reduce {
                assignments,
                groups,
            } => ReadStage::Reduce {
                assignments: assignments.clone(),
                groups: groups.clone(),
            },
            Self::Sort { terms } => ReadStage::Sort {
                terms: terms.clone(),
            },
            Self::Offset { rows } => ReadStage::Offset { rows: *rows },
            Self::Limit { rows } => ReadStage::Limit { rows: *rows },
            _ => return None,
        })
    }
}

/// Untrusted constructor inputs; QueryPlanV3 validates them against authority.
#[derive(Clone, Debug)]
pub struct PlanSpec {
    /// Dense ordered binding declarations.
    pub bindings: Vec<AssertionBinding>,
    /// Typed invocation columns.
    pub inputs: Vec<InputColumn>,
    /// Ordered read/write pipeline.
    pub pipeline: Vec<Stage>,
    /// Final projection. A write without a projection uses empty row columns.
    pub output: QueryOutput,
    /// Expected managed schema semantics.
    pub managed_semantics: ManagedSemanticSchemaFingerprint,
    /// Retained V2 read semantics when lifting a model-query plan.
    pub compatibility: Option<QueryPlanV2Compatibility>,
    /// Plan-local read functions.
    pub functions: Vec<LocalFunction>,
}

/// A trusted canonical V3 plan. Decoding always rebuilds through validation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueryPlanV3 {
    bindings: Vec<AssertionBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<QueryPlanV2Compatibility>,
    format: String,
    functions: Vec<LocalFunction>,
    inputs: Vec<InputColumn>,
    #[serde(skip)]
    limits: QueryLimits,
    managed_semantics: ManagedSemanticSchemaFingerprint,
    output: QueryOutput,
    pipeline: Vec<Stage>,
    required_capabilities: CapabilitySet,
}

impl QueryPlanV3 {
    /// Validate one program against declared schema authority and fixed limits.
    pub fn new(spec: PlanSpec, schema: &DeclaredSchema) -> Result<Self, Diagnostic> {
        Self::new_with_limits(spec, schema, QueryLimits::CANONICAL)
    }

    /// Validate with caller-selected limits, which never enter canonical bytes.
    pub fn new_with_limits(
        spec: PlanSpec,
        schema: &DeclaredSchema,
        limits: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        validate_declarations(&spec.bindings, &spec.inputs, limits.structural)?;
        let mut nodes = 0;
        validate_local_functions(&spec.functions, limits.structural, &mut nodes)?;
        validate_stages(&spec, schema, limits, &mut nodes)?;
        if let Some(compatibility) = &spec.compatibility {
            compatibility.validate(spec.bindings.len(), limits)?;
        }
        let reads: Vec<_> = spec.pipeline.iter().filter_map(Stage::read_stage).collect();
        let mut required_capabilities = derive_capabilities(
            &reads,
            &spec.functions,
            &spec.inputs,
            &spec.output,
            spec.compatibility.as_ref(),
        )?;
        insert_capability(&mut required_capabilities, "query.plan.v3")?;
        for stage in &spec.pipeline {
            let capability = match stage {
                Stage::Insert { .. } => "query.write.insert",
                Stage::Update { .. } => "query.write.update",
                Stage::Delete { .. } => "query.write.delete",
                Stage::Put { .. } => "query.write.put",
                _ => continue,
            };
            insert_capability(&mut required_capabilities, capability)?;
        }
        let plan = Self {
            bindings: spec.bindings,
            compatibility: spec.compatibility,
            format: QUERY_PLAN_FORMAT_V3.into(),
            functions: spec.functions,
            inputs: spec.inputs,
            limits,
            managed_semantics: spec.managed_semantics,
            output: spec.output,
            pipeline: spec.pipeline,
            required_capabilities,
        };
        plan.canonical_bytes()?;
        Ok(plan)
    }

    pub(super) fn from_read_plan(
        plan: &QueryPlan,
        schema: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_limits(
            PlanSpec {
                bindings: plan.bindings.clone(),
                compatibility: plan.compatibility.clone(),
                functions: plan.functions.clone(),
                inputs: plan.inputs.clone(),
                managed_semantics: plan.managed_semantics.clone(),
                output: plan.output.clone(),
                pipeline: plan.pipeline.iter().cloned().map(Stage::from).collect(),
            },
            schema,
            plan.limits,
        )
    }

    /// Exact format discriminator.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }
    /// Dense binding declarations.
    #[must_use]
    pub fn bindings(&self) -> &[AssertionBinding] {
        &self.bindings
    }
    /// Typed invocation columns.
    #[must_use]
    pub fn inputs(&self) -> &[InputColumn] {
        &self.inputs
    }
    /// Ordered pipeline.
    #[must_use]
    pub fn pipeline(&self) -> &[Stage] {
        &self.pipeline
    }
    /// Plan-local read functions.
    #[must_use]
    pub fn functions(&self) -> &[LocalFunction] {
        &self.functions
    }
    /// Frozen V2 model-query evidence retained by a lifted plan.
    #[must_use]
    pub fn compatibility(&self) -> Option<&QueryPlanV2Compatibility> {
        self.compatibility.as_ref()
    }
    /// Final projection.
    #[must_use]
    pub const fn output(&self) -> &QueryOutput {
        &self.output
    }
    /// Managed schema semantics.
    #[must_use]
    pub const fn managed_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantics
    }
    /// Capabilities derived exclusively from syntax.
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }
    /// Encode with the bounded common canonical JSON codec.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, self.limits.codec)
    }
    /// Fingerprint under the V3 canonicalization version and common plan domain.
    pub fn fingerprint(&self) -> Result<QueryPlanFingerprint, Diagnostic> {
        Ok(QueryPlanFingerprint(Fingerprint::compute(
            FingerprintDomain::new(QUERY_PLAN_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_PLAN_CANONICALIZATION_V3)?,
            None,
            &self.canonical_bytes()?,
        )))
    }
}

fn invalid(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::InvalidContract, code, message)
}

fn validate_stages(
    spec: &PlanSpec,
    schema: &DeclaredSchema,
    limits: QueryLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    let Some(first) = spec.pipeline.first() else {
        return Err(invalid(
            "query_plan_empty_pipeline",
            "a plan requires at least one stage",
        ));
    };
    let index = SchemaIndex::new(schema);
    let mut mandatory = BTreeSet::new();
    let mut optional = BTreeSet::new();
    let mut referenced = BTreeSet::new();
    let mut types = BTreeMap::<BindingId, BTreeSet<TypeId>>::new();
    let mut read_tail = Vec::new();
    let mut reading = false;
    if !matches!(
        first,
        Stage::Match { .. } | Stage::Insert { .. } | Stage::Put { .. }
    ) {
        return Err(invalid(
            "query_plan_match_not_first",
            "a plan starts with match, insert or put",
        ));
    }
    for (stage_index, stage) in spec.pipeline.iter().enumerate() {
        *nodes += 1;
        if *nodes > limits.structural.predicate_nodes {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_stage_limit",
                "plan stages exceed the structural ceiling",
            ));
        }
        match stage {
            Stage::Match { patterns } => {
                if stage_index != 0 {
                    return Err(invalid(
                        "query_plan_stage_order",
                        "match occurs only at the start",
                    ));
                }
                (mandatory, optional, _) = validate_pipeline(
                    &[ReadStage::Match {
                        patterns: patterns.clone(),
                    }],
                    spec.bindings.len(),
                    spec.inputs.len(),
                    limits.structural,
                    nodes,
                )?;
                for pattern in patterns {
                    collect_pattern_bindings(pattern, &mut referenced);
                    index.pattern_types(pattern, &mut types)?;
                }
            }
            Stage::Insert { things } | Stage::Put { things } => {
                if reading {
                    return Err(invalid(
                        "query_plan_stage_order",
                        "write stages precede the read tail",
                    ));
                }
                nonempty(things.len(), limits, nodes)?;
                for thing in things {
                    check_binding(thing.binding, spec.bindings.len())?;
                    if !mandatory.insert(thing.binding)
                        || optional.contains(&thing.binding)
                        || referenced.contains(&thing.binding)
                    {
                        return Err(invalid(
                            "query_plan_write_binding_not_fresh",
                            "insert and put assign fresh declared bindings",
                        ));
                    }
                    if thing.type_id.kind() == TypeKind::Attribute {
                        return Err(invalid(
                            "query_plan_insert_attribute_type",
                            "insert and put require an entity or relation type",
                        ));
                    }
                    if thing.type_id.kind() == TypeKind::Relation && thing.players.is_empty() {
                        return Err(invalid(
                            "query_plan_insert_relation_players",
                            "relation insert and put require at least one player",
                        ));
                    }
                    index.concrete(&thing.type_id)?;
                    types.insert(thing.binding, BTreeSet::from([thing.type_id.clone()]));
                    referenced.insert(thing.binding);
                }
                for thing in things {
                    for (field, operand) in &thing.fields {
                        index.ownership(thing.binding, field, &types, false)?;
                        validate_operand(operand, &mandatory, &optional, spec.inputs.len())?;
                    }
                    for (role, player) in &thing.players {
                        bound(*player, &mandatory)?;
                        index.role(thing.binding, role, *player, &types)?;
                    }
                    nonempty(thing.fields.len() + thing.players.len() + 1, limits, nodes)?;
                }
            }
            Stage::Update { sets } => {
                if reading {
                    return Err(invalid(
                        "query_plan_stage_order",
                        "write stages precede the read tail",
                    ));
                }
                nonempty(sets.len(), limits, nodes)?;
                for set in sets {
                    match set {
                        Set::Field {
                            owner,
                            field,
                            operand,
                        } => {
                            bound(*owner, &mandatory)?;
                            index.ownership(*owner, field, &types, true)?;
                            validate_operand(operand, &mandatory, &optional, spec.inputs.len())?;
                        }
                        Set::Link {
                            relation,
                            role,
                            player,
                        } => {
                            bound(*relation, &mandatory)?;
                            bound(*player, &mandatory)?;
                            index.role(*relation, role, *player, &types)?;
                        }
                    }
                }
            }
            Stage::Delete { items } => {
                if reading {
                    return Err(invalid(
                        "query_plan_stage_order",
                        "write stages precede the read tail",
                    ));
                }
                nonempty(items.len(), limits, nodes)?;
                // Every deletion reads the incoming environment. Lifetimes end
                // together at this stage's boundary, not by item list order.
                for item in items {
                    match item {
                        DeleteItem::Thing { binding } => {
                            bound(*binding, &mandatory)?;
                            if !types.contains_key(binding) {
                                return Err(invalid(
                                    "query_plan_delete_untyped_thing",
                                    "thing deletion requires a typed thing binding",
                                ));
                            }
                        }
                        DeleteItem::Has { owner, field } => {
                            bound(*owner, &mandatory)?;
                            index.ownership(*owner, field, &types, false)?;
                        }
                        DeleteItem::Links {
                            relation,
                            role,
                            player,
                        } => {
                            bound(*relation, &mandatory)?;
                            bound(*player, &mandatory)?;
                            index.role(*relation, role, *player, &types)?;
                        }
                    }
                }
                for item in items {
                    if let DeleteItem::Thing { binding } = item {
                        mandatory.remove(binding);
                    }
                }
            }
            _ => {
                reading = true;
                read_tail.push(stage.read_stage().expect("read stage"));
            }
        }
    }
    let (mandatory, optional, has_sort) = validate_read_stages(
        &read_tail,
        mandatory,
        optional,
        &referenced,
        spec.bindings.len(),
        limits.structural,
    )?;
    if !has_sort
        && read_tail
            .iter()
            .any(|stage| matches!(stage, ReadStage::Offset { .. } | ReadStage::Limit { .. }))
    {
        return Err(invalid(
            "query_plan_unordered_truncation",
            "offset and limit require explicit sort",
        ));
    }
    if !matches!(&spec.output, QueryOutput::Rows { columns } if columns.is_empty() && spec.pipeline.iter().any(Stage::is_write))
    {
        validate_output(&spec.output, &mandatory, &optional, limits.structural)?;
    }
    Ok(())
}

fn nonempty(count: usize, limits: QueryLimits, nodes: &mut usize) -> Result<(), Diagnostic> {
    if count == 0 {
        return Err(invalid(
            "query_plan_write_empty",
            "a write stage requires at least one term",
        ));
    }
    *nodes = nodes.saturating_add(count);
    if count > limits.structural.boolean_terms || *nodes > limits.structural.predicate_nodes {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_write_term_limit",
            "write terms exceed the structural ceiling",
        ));
    }
    Ok(())
}

fn bound(binding: BindingId, visible: &BTreeSet<BindingId>) -> Result<(), Diagnostic> {
    if visible.contains(&binding) {
        Ok(())
    } else {
        Err(invalid(
            "query_plan_write_unbound_target",
            "write references an unbound or deleted binding",
        ))
    }
}

fn validate_operand(
    operand: &QueryOperand,
    mandatory: &BTreeSet<BindingId>,
    optional: &BTreeSet<BindingId>,
    inputs: usize,
) -> Result<(), Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => {
            bound(*binding, mandatory)?;
            if optional.contains(binding) {
                return Err(invalid(
                    "query_plan_write_optional_operand",
                    "write operands require present values",
                ));
            }
        }
        QueryOperand::Input { column } if usize::from(column.get()) >= inputs => {
            return Err(invalid(
                "query_plan_unknown_input",
                "write references an undeclared input",
            ));
        }
        _ => {}
    }
    Ok(())
}

struct SchemaIndex<'a> {
    schema: &'a DeclaredSchema,
    parents: BTreeMap<TypeId, TypeId>,
}

impl<'a> SchemaIndex<'a> {
    fn new(schema: &'a DeclaredSchema) -> Self {
        Self {
            schema,
            parents: schema
                .facts()
                .filter_map(|fact| match fact {
                    SchemaFact::Sub(fact) => {
                        Some((fact.id().subtype().clone(), fact.id().supertype().clone()))
                    }
                    _ => None,
                })
                .collect(),
        }
    }
    fn ancestors(&self, type_id: &TypeId) -> Vec<TypeId> {
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = Some(type_id);
        while let Some(id) = current {
            if !seen.insert(id.clone()) {
                break;
            }
            result.push(id.clone());
            current = self.parents.get(id);
        }
        result
    }
    fn exists(&self, type_id: &TypeId) -> Result<(), Diagnostic> {
        if self
            .schema
            .fact(&SchemaFactId::Type(type_id.clone()))
            .is_none()
        {
            return Err(invalid(
                "query_plan_unknown_type",
                "plan references an undeclared schema type",
            ));
        }
        Ok(())
    }
    fn annotation(&self, subject: AnnotationSubjectId, kind: AnnotationKindId) -> bool {
        self.schema
            .fact(&SchemaFactId::Annotation(AnnotationFactId::new(
                subject, kind,
            )))
            .is_some()
    }
    fn concrete(&self, type_id: &TypeId) -> Result<(), Diagnostic> {
        self.exists(type_id)?;
        if self.annotation(
            AnnotationSubjectId::Type(type_id.clone()),
            AnnotationKindId::Abstract,
        ) {
            return Err(invalid(
                "query_plan_abstract_insert",
                "insert and put require concrete types",
            ));
        }
        Ok(())
    }
    fn pattern_types(
        &self,
        pattern: &QueryPattern,
        types: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    ) -> Result<(), Diagnostic> {
        match pattern {
            QueryPattern::Isa {
                binding,
                type_id,
                include_subtypes,
            } => {
                self.exists(type_id)?;
                let candidates = types.entry(*binding).or_default();
                candidates.insert(type_id.clone());
                if *include_subtypes {
                    for fact in self.schema.facts() {
                        if let SchemaFact::Type(fact) = fact
                            && self.ancestors(fact.id()).contains(type_id)
                        {
                            candidates.insert(fact.id().clone());
                        }
                    }
                }
            }
            QueryPattern::Has {
                attribute,
                attribute_id,
                ..
            } => {
                let id = TypeId::new(TypeKind::Attribute, attribute_id.label().as_str())?;
                self.exists(&id)?;
                types.entry(*attribute).or_default().insert(id);
            }
            QueryPattern::Links {
                relation,
                relation_id,
                ..
            } => {
                self.exists(relation_id)?;
                types
                    .entry(*relation)
                    .or_default()
                    .insert(relation_id.clone());
            }
            QueryPattern::Or { branches } => {
                for branch in branches {
                    for pattern in branch {
                        self.pattern_types(pattern, types)?;
                    }
                }
            }
            QueryPattern::Try { patterns } => {
                for pattern in patterns {
                    self.pattern_types(pattern, types)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn ownership(
        &self,
        owner: BindingId,
        field: &OwnsFactId,
        types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
        update: bool,
    ) -> Result<(), Diagnostic> {
        if self
            .schema
            .fact(&SchemaFactId::Owns(field.clone()))
            .is_none()
        {
            return Err(invalid(
                "query_plan_undeclared_owns",
                "write ownership is not declared",
            ));
        }
        let candidates = types.get(&owner).ok_or_else(|| {
            invalid(
                "query_plan_write_untyped_target",
                "write target has no schema type",
            )
        })?;
        for type_id in candidates {
            let ancestors = self.ancestors(type_id);
            if !ancestors.contains(field.owner()) {
                return Err(invalid(
                    "query_plan_wrong_owns_owner",
                    "ownership does not belong to the bound owner type",
                ));
            }
            if update
                && ancestors.iter().any(|owner| {
                    OwnsFactId::new(owner.clone(), field.attribute().clone())
                        .ok()
                        .is_some_and(|id| {
                            self.annotation(AnnotationSubjectId::Owns(id), AnnotationKindId::Key)
                        })
                })
            {
                return Err(invalid(
                    "query_plan_key_update",
                    "key ownerships cannot be updated",
                ));
            }
        }
        Ok(())
    }
    fn role(
        &self,
        relation: BindingId,
        role: &RoleId,
        player: BindingId,
        types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    ) -> Result<(), Diagnostic> {
        let owner = TypeId::new(TypeKind::Relation, role.declaring_relation().as_str())?;
        let relates = RelatesFactId::new(owner.clone(), role.clone())?;
        if self.schema.fact(&SchemaFactId::Relates(relates)).is_none() {
            return Err(invalid(
                "query_plan_undeclared_relates",
                "write role is not declared",
            ));
        }
        let relations = types.get(&relation).ok_or_else(|| {
            invalid(
                "query_plan_write_untyped_target",
                "relation has no schema type",
            )
        })?;
        if relations
            .iter()
            .any(|id| !self.ancestors(id).contains(&owner))
        {
            return Err(invalid(
                "query_plan_wrong_role_owner",
                "role does not belong to the relation type",
            ));
        }
        let players = types.get(&player).ok_or_else(|| {
            invalid(
                "query_plan_write_untyped_target",
                "player has no schema type",
            )
        })?;
        for player in players {
            let plays = self.ancestors(player).iter().any(|id| {
                PlaysFactId::new(id.clone(), role.clone())
                    .ok()
                    .is_some_and(|id| self.schema.fact(&SchemaFactId::Plays(id)).is_some())
            });
            if !plays {
                return Err(invalid(
                    "query_plan_undeclared_plays",
                    "player type does not declare the written role",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanWire {
    bindings: Vec<QueryBindingWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compatibility: Option<QueryPlanV2Compatibility>,
    format: String,
    functions: Vec<LocalFunctionWire>,
    inputs: Vec<InputColumnWire>,
    managed_semantics: FingerprintWire,
    output: serde_json::Value,
    pipeline: Vec<serde_json::Value>,
    required_capabilities: CapabilitySet,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnsWire {
    owner: TypeIdWire,
    attribute: String,
}
impl OwnsWire {
    fn rebuild(self) -> Result<OwnsFactId, Diagnostic> {
        OwnsFactId::new(self.owner.rebuild()?, AttributeId::new(self.attribute)?)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ThingWire {
    binding: u16,
    type_id: TypeIdWire,
    fields: Vec<(OwnsWire, QueryOperandWire)>,
    players: Vec<(RoleIdWire, u16)>,
}
impl ThingWire {
    fn rebuild(self) -> Result<InsertThing, Diagnostic> {
        Ok(InsertThing {
            binding: BindingId::new(self.binding)?,
            type_id: self.type_id.rebuild()?,
            fields: self
                .fields
                .into_iter()
                .map(|(field, value)| Ok((field.rebuild()?, value.rebuild()?)))
                .collect::<Result<_, Diagnostic>>()?,
            players: self
                .players
                .into_iter()
                .map(|(role, player)| Ok((role.rebuild()?, BindingId::new(player)?)))
                .collect::<Result<_, Diagnostic>>()?,
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SetWire {
    Field {
        owner: u16,
        field: OwnsWire,
        operand: QueryOperandWire,
    },
    Link {
        relation: u16,
        role: RoleIdWire,
        player: u16,
    },
}
impl SetWire {
    fn rebuild(self) -> Result<Set, Diagnostic> {
        Ok(match self {
            Self::Field {
                owner,
                field,
                operand,
            } => Set::Field {
                owner: BindingId::new(owner)?,
                field: field.rebuild()?,
                operand: operand.rebuild()?,
            },
            Self::Link {
                relation,
                role,
                player,
            } => Set::Link {
                relation: BindingId::new(relation)?,
                role: role.rebuild()?,
                player: BindingId::new(player)?,
            },
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeleteWire {
    Thing {
        binding: u16,
    },
    Has {
        owner: u16,
        field: OwnsWire,
    },
    Links {
        relation: u16,
        role: RoleIdWire,
        player: u16,
    },
}
impl DeleteWire {
    fn rebuild(self) -> Result<DeleteItem, Diagnostic> {
        Ok(match self {
            Self::Thing { binding } => DeleteItem::Thing {
                binding: BindingId::new(binding)?,
            },
            Self::Has { owner, field } => DeleteItem::Has {
                owner: BindingId::new(owner)?,
                field: field.rebuild()?,
            },
            Self::Links {
                relation,
                role,
                player,
            } => DeleteItem::Links {
                relation: BindingId::new(relation)?,
                role: role.rebuild()?,
                player: BindingId::new(player)?,
            },
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WriteWire {
    Insert { things: Vec<ThingWire> },
    Update { sets: Vec<SetWire> },
    Delete { items: Vec<DeleteWire> },
    Put { things: Vec<ThingWire> },
}
impl WriteWire {
    fn rebuild(self) -> Result<Stage, Diagnostic> {
        Ok(match self {
            Self::Insert { things } => Stage::Insert {
                things: things
                    .into_iter()
                    .map(ThingWire::rebuild)
                    .collect::<Result<_, _>>()?,
            },
            Self::Put { things } => Stage::Put {
                things: things
                    .into_iter()
                    .map(ThingWire::rebuild)
                    .collect::<Result<_, _>>()?,
            },
            Self::Update { sets } => Stage::Update {
                sets: sets
                    .into_iter()
                    .map(SetWire::rebuild)
                    .collect::<Result<_, _>>()?,
            },
            Self::Delete { items } => Stage::Delete {
                items: items
                    .into_iter()
                    .map(DeleteWire::rebuild)
                    .collect::<Result<_, _>>()?,
            },
        })
    }
}
fn decode_stage(value: serde_json::Value) -> Result<Stage, Diagnostic> {
    if matches!(
        value.get("kind").and_then(serde_json::Value::as_str),
        Some("insert" | "update" | "delete" | "put")
    ) {
        serde_json::from_value::<WriteWire>(value)
            .map_err(|_| invalid("invalid_canonical_value", "invalid write stage"))?
            .rebuild()
    } else {
        Ok(rebuild_read_stage_v2(value)?.into())
    }
}

/// Decode exact canonical V3 bytes using local schema authority.
pub fn decode_query_plan_v3(
    bytes: &[u8],
    schema: &DeclaredSchema,
) -> Result<QueryPlanV3, Diagnostic> {
    decode_query_plan_v3_with_limits(bytes, schema, QueryLimits::CANONICAL)
}

/// Decode under caller-selected codec and structural ceilings.
pub fn decode_query_plan_v3_with_limits(
    bytes: &[u8],
    schema: &DeclaredSchema,
    limits: QueryLimits,
) -> Result<QueryPlanV3, Diagnostic> {
    let wire: PlanWire = from_canonical_json_with_limits(bytes, limits.codec)?;
    if wire.format != QUERY_PLAN_FORMAT_V3 {
        return Err(invalid(
            "query_plan_format_unsupported",
            "V3 decoding requires the exact V3 discriminator",
        ));
    }
    let claims = wire.required_capabilities;
    let plan = QueryPlanV3::new_with_limits(
        PlanSpec {
            bindings: wire
                .bindings
                .into_iter()
                .map(QueryBindingWire::rebuild)
                .collect::<Result<_, _>>()?,
            inputs: wire
                .inputs
                .into_iter()
                .map(InputColumnWire::rebuild)
                .collect::<Result<_, _>>()?,
            pipeline: wire
                .pipeline
                .into_iter()
                .map(decode_stage)
                .collect::<Result<_, _>>()?,
            output: rebuild_output(wire.output)?,
            managed_semantics: ManagedSemanticSchemaFingerprint::from_wire(
                wire.managed_semantics.rebuild()?,
            )?,
            compatibility: wire.compatibility,
            functions: wire
                .functions
                .into_iter()
                .map(LocalFunctionWire::rebuild)
                .collect::<Result<_, _>>()?,
        },
        schema,
        limits,
    )?;
    if claims != plan.required_capabilities {
        return Err(invalid(
            "query_plan_capability_claim_mismatch",
            "V3 capabilities must be syntax-derived",
        ));
    }
    if plan.canonical_bytes()? != bytes {
        return Err(invalid(
            "query_plan_wire_mismatch",
            "V3 reconstruction changes canonical bytes",
        ));
    }
    Ok(plan)
}
