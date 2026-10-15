//! Additive canonical read/write plans. V1/V2 representations remain frozen.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::*;
use crate::codec::from_canonical_json_with_limits;
use crate::id::TypeKind;
use crate::migration_assertion_wire::{FingerprintWire, RoleIdWire, TypeIdWire};
use crate::query_plan_wire::rebuild_pattern_v2;
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

#[path = "query_plan_v3_expr.rs"]
mod expression;
pub use expression::{ArithOp, Builtin, Expr};
use expression::{ExprWire, operand_type};

/// A closed match pattern, including a fresh scalar expression assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Pattern {
    /// A released closed read pattern.
    Read(QueryPattern),
    /// Assign a fresh value binding in source order.
    Let {
        /// Fresh declared scalar binding.
        binding: BindingId,
        /// Typed expression over present bindings, literals and inputs.
        expr: Expr,
    },
}
impl From<QueryPattern> for Pattern {
    fn from(pattern: QueryPattern) -> Self {
        Self::Read(pattern)
    }
}
impl Serialize for Pattern {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Read(pattern) => pattern.serialize(serializer),
            Self::Let { binding, expr } => {
                #[derive(Serialize)]
                struct View<'a> {
                    kind: &'static str,
                    binding: BindingId,
                    expr: &'a Expr,
                }
                View {
                    kind: "let",
                    binding: *binding,
                    expr,
                }
                .serialize(serializer)
            }
        }
    }
}

/// The terminal operation is part of V3 plan identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Project read rows.
    Rows,
    /// Count matched read rows.
    Count,
    /// Whether any read row matched.
    Exists,
    /// Execute at least one write stage.
    Write {
        /// Resolve a match count or the same pipeline's projection.
        returns: WriteReturns,
    },
}

/// A write plan's result category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteReturns {
    /// Number of rows matched by the write pipeline.
    Matched,
    /// Project rows after the writes, in the same pipeline.
    Rows,
}

/// Every capability derivable from closed V3 syntax.
#[must_use]
pub fn query_plan_v3_capability_vocabulary() -> CapabilitySet {
    query_plan_v2_capability_vocabulary()
        .into_iter()
        .chain(
            [
                "query.plan.v3",
                "query.write.insert",
                "query.write.update",
                "query.write.delete",
                "query.write.put",
                "query.let",
                "query.expr.add",
                "query.expr.sub",
                "query.expr.mul",
                "query.expr.div",
                "query.expr.mod",
                "query.expr.pow",
                "query.expr.abs",
                "query.expr.ceil",
                "query.expr.floor",
                "query.expr.round",
                "query.expr.min",
                "query.expr.max",
                "query.expr.length",
            ]
            .map(|name| CapabilityId::new(name).expect("static capability")),
        )
        .collect()
}

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
        patterns: Vec<Pattern>,
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
            ReadStage::Match { patterns } => Self::Match {
                patterns: patterns.into_iter().map(Pattern::Read).collect(),
            },
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
                patterns: patterns
                    .iter()
                    .filter_map(|pattern| match pattern {
                        Pattern::Read(pattern) => Some(pattern.clone()),
                        Pattern::Let { .. } => None,
                    })
                    .collect(),
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
    /// Bound rows, or None for an invocation-free reusable template.
    pub given_rows: Option<Vec<InputRow>>,
    /// Terminal read or write operation.
    pub operation: Operation,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    given_rows: Option<Vec<InputRow>>,
    operation: Operation,
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
        limits.validate().map_err(|_| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_invalid_limits",
                "plan limits are outside the supported protocol range",
            )
        })?;
        Self::build(spec, schema, limits).map_err(|diagnostic| limits.annotate(diagnostic))
    }

    fn build(
        spec: PlanSpec,
        schema: &DeclaredSchema,
        limits: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        validate_declarations(&spec.bindings, &spec.inputs, limits.structural)?;
        validate_operation(&spec)?;
        if let Some(rows) = &spec.given_rows {
            if spec.inputs.is_empty() {
                return Err(invalid(
                    "query_invocation_unexpected_inputs",
                    "given rows require declared input columns",
                ));
            }
            validate_input_rows(&spec.inputs, rows, limits.structural)?;
        }
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
        add_read_given_capability(
            &spec.pipeline,
            &spec.functions,
            &spec.inputs,
            &mut required_capabilities,
        )?;
        for stage in &spec.pipeline {
            if let Stage::Match { patterns } = stage {
                for pattern in patterns {
                    if let Pattern::Let { expr, .. } = pattern {
                        insert_capability(&mut required_capabilities, "query.let")?;
                        expr.capabilities(&mut required_capabilities)?;
                    }
                }
            }
            let operands: Vec<&QueryOperand> = match stage {
                Stage::Insert { things } | Stage::Put { things } => things
                    .iter()
                    .flat_map(|thing| thing.fields.iter().map(|(_, operand)| operand))
                    .collect(),
                Stage::Update { sets } => sets
                    .iter()
                    .filter_map(|set| match set {
                        Set::Field { operand, .. } => Some(operand),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };
            if operands.iter().any(|operand| {
                matches!(
                    operand,
                    QueryOperand::Literal { .. } | QueryOperand::Input { .. }
                )
            }) {
                insert_capability(&mut required_capabilities, "query.input.given-rows")?;
            }
        }
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
            given_rows: spec.given_rows,
            operation: spec.operation,
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
                given_rows: None,
                operation: Operation::Rows,
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
    /// Bound given rows, absent for a reusable template.
    #[must_use]
    pub fn given_rows(&self) -> Option<&[InputRow]> {
        self.given_rows.as_deref()
    }
    /// Terminal operation.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }
    /// Bind values to a reusable plan; rows participate in its V3 fingerprint.
    pub fn with_given_rows(
        &self,
        rows: Vec<InputRow>,
        schema: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        let mut spec = self.spec();
        spec.given_rows = Some(rows);
        Self::new_with_limits(spec, schema, self.limits)
    }
    /// Select an operation when lifting an invocation from V2.
    pub fn with_operation(
        &self,
        operation: Operation,
        schema: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        let mut spec = self.spec();
        spec.operation = operation;
        Self::new_with_limits(spec, schema, self.limits)
    }
    fn spec(&self) -> PlanSpec {
        PlanSpec {
            bindings: self.bindings.clone(),
            inputs: self.inputs.clone(),
            given_rows: self.given_rows.clone(),
            operation: self.operation,
            pipeline: self.pipeline.clone(),
            output: self.output.clone(),
            managed_semantics: self.managed_semantics.clone(),
            compatibility: self.compatibility.clone(),
            functions: self.functions.clone(),
        }
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
    let mut scalar_types = BTreeMap::new();
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
                nonempty(patterns.len(), limits, nodes)?;
                let reads: Vec<_> = patterns
                    .iter()
                    .filter_map(|pattern| match pattern {
                        Pattern::Read(pattern) => Some(pattern.clone()),
                        _ => None,
                    })
                    .collect();
                *nodes -= reads.len();
                if !reads.is_empty() {
                    (mandatory, optional, _) = validate_pipeline(
                        &[ReadStage::Match {
                            patterns: reads.clone(),
                        }],
                        spec.bindings.len(),
                        spec.inputs.len(),
                        limits.structural,
                        nodes,
                    )?;
                }
                for pattern in &reads {
                    collect_pattern_bindings(pattern, &mut referenced);
                    index.pattern_types(pattern, &mut types)?;
                }
                for (binding, domains) in &types {
                    if mandatory.contains(binding)
                        && let Some(domain) = index.scalar_domain(domains)
                    {
                        scalar_types.insert(*binding, domain);
                    }
                }
                index.function_values(&reads, &spec.functions, &mandatory, &mut scalar_types)?;
                let mut produced = BTreeSet::new();
                for pattern in &reads {
                    collect_produced_bindings(pattern, &mut produced);
                }
                for pattern in patterns {
                    if let Pattern::Let { binding, expr } = pattern {
                        check_binding(*binding, spec.bindings.len())?;
                        if produced.contains(binding)
                            || mandatory.contains(binding)
                            || optional.contains(binding)
                        {
                            return Err(invalid(
                                "query_plan_let_binding_not_fresh",
                                "let requires a fresh value binding",
                            ));
                        }
                        let domain = expr.infer(&scalar_types, &spec.inputs, limits, 1, nodes)?;
                        mandatory.insert(*binding);
                        referenced.insert(*binding);
                        scalar_types.insert(*binding, domain);
                    }
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
                        index.field_value(field, operand, &scalar_types, &spec.inputs)?;
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
                            index.field_value(field, operand, &scalar_types, &spec.inputs)?;
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
    if !matches!(&spec.output, QueryOutput::Rows { columns } if columns.is_empty() && matches!(spec.operation,Operation::Count|Operation::Exists|Operation::Write{returns:WriteReturns::Matched}))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    given_rows: Option<Vec<Vec<Option<CanonicalValue>>>>,
    operation: Operation,
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
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("match") {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MatchWire {
            kind: String,
            patterns: Vec<serde_json::Value>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LetWire {
            kind: String,
            binding: u16,
            expr: ExprWire,
        }
        let wire: MatchWire = serde_json::from_value(value)
            .map_err(|_| invalid("invalid_canonical_value", "invalid match stage"))?;
        debug_assert_eq!(wire.kind, "match");
        return Ok(Stage::Match {
            patterns: wire
                .patterns
                .into_iter()
                .map(|value| {
                    if value.get("kind").and_then(serde_json::Value::as_str) == Some("let") {
                        let wire: LetWire = serde_json::from_value(value).map_err(|_| {
                            invalid("invalid_canonical_value", "invalid let pattern")
                        })?;
                        debug_assert_eq!(wire.kind, "let");
                        Ok(Pattern::Let {
                            binding: BindingId::new(wire.binding)?,
                            expr: wire.expr.rebuild()?,
                        })
                    } else {
                        Ok(Pattern::Read(rebuild_pattern_v2(value)?))
                    }
                })
                .collect::<Result<_, Diagnostic>>()?,
        });
    }

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
            given_rows: wire
                .given_rows
                .map(|rows| rows.into_iter().map(InputRow::new).collect()),
            operation: wire.operation,
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

fn validate_operation(spec: &PlanSpec) -> Result<(), Diagnostic> {
    let writes = spec.pipeline.iter().any(Stage::is_write);
    if writes != matches!(spec.operation, Operation::Write { .. }) {
        return Err(invalid(
            "query_plan_operation_write_mismatch",
            "read operations require read stages; write requires a write stage",
        ));
    }
    if matches!(
        spec.operation,
        Operation::Write {
            returns: WriteReturns::Matched
        }
    ) && !matches!(&spec.output,QueryOutput::Rows{columns} if columns.is_empty())
    {
        return Err(invalid(
            "query_plan_matched_projection",
            "matched writes have no projected output",
        ));
    }
    Ok(())
}

fn read_uses_given(pattern: &QueryPattern) -> bool {
    match pattern {
        QueryPattern::Value { left, right, .. } => [left, right].iter().any(|operand| {
            matches!(
                operand,
                QueryOperand::Literal { .. } | QueryOperand::Input { .. }
            )
        }),
        QueryPattern::FunctionCall { .. } => true,
        QueryPattern::Or { branches } => branches.iter().flatten().any(read_uses_given),
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            patterns.iter().any(read_uses_given)
        }
        _ => false,
    }
}
fn add_read_given_capability(
    pipeline: &[Stage],
    functions: &[LocalFunction],
    inputs: &[InputColumn],
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    if !inputs.is_empty() || pipeline.iter().any(|stage|matches!(stage,Stage::Match{patterns} if patterns.iter().any(|pattern|matches!(pattern,Pattern::Read(pattern) if read_uses_given(pattern))))) || functions.iter().any(|function|function.body().iter().any(read_uses_given)) {
        insert_capability(capabilities,"query.input.given-rows")?;
    }
    Ok(())
}

impl SchemaIndex<'_> {
    fn attribute_domain(&self, attribute: &AttributeId) -> Option<ValueTypeTag> {
        let type_id = TypeId::new(TypeKind::Attribute, attribute.label().as_str()).ok()?;
        self.ancestors(&type_id).iter().find_map(|id| {
            let attribute = AttributeId::new(id.label().as_str()).ok()?;
            match self
                .schema
                .fact(&SchemaFactId::Value(crate::schema::ValueFactId::new(
                    attribute,
                ))) {
                Some(SchemaFact::Value(fact)) => Some(fact.value_type()),
                _ => None,
            }
        })
    }
    fn scalar_domain(&self, types: &BTreeSet<TypeId>) -> Option<ValueTypeTag> {
        let mut domain = None;
        for type_id in types {
            if type_id.kind() != TypeKind::Attribute {
                return None;
            }
            let found = self.attribute_domain(&AttributeId::new(type_id.label().as_str()).ok()?)?;
            if domain.is_some_and(|domain| domain != found) {
                return None;
            }
            domain = Some(found);
        }
        domain
    }
    fn field_value(
        &self,
        field: &OwnsFactId,
        operand: &QueryOperand,
        bindings: &BTreeMap<BindingId, ValueTypeTag>,
        inputs: &[InputColumn],
    ) -> Result<(), Diagnostic> {
        let expected = self.attribute_domain(field.attribute()).ok_or_else(|| {
            invalid(
                "query_plan_write_value_type",
                "ownership has no unique scalar domain",
            )
        })?;
        let actual = operand_type(operand, bindings, inputs)?;
        if actual != expected {
            return Err(invalid(
                "query_plan_write_value_type",
                "write operand differs from the ownership scalar domain",
            ));
        }
        Ok(())
    }
    fn function_values(
        &self,
        patterns: &[QueryPattern],
        functions: &[LocalFunction],
        mandatory: &BTreeSet<BindingId>,
        values: &mut BTreeMap<BindingId, ValueTypeTag>,
    ) -> Result<(), Diagnostic> {
        use crate::schema::{FunctionReturnMode, TypeReference};
        for pattern in patterns {
            match pattern {
                QueryPattern::FunctionCall {
                    assigned, function, ..
                } if mandatory.contains(assigned) => {
                    let domain = if let Some(local) =
                        functions.iter().find(|local| local.name() == function)
                    {
                        Some(local.returns().value_type())
                    } else if let Some(SchemaFact::Function(function)) =
                        self.schema.fact(&SchemaFactId::Function(function.clone()))
                    {
                        match function.signature().returns() {
                            FunctionReturnMode::Scalar(element) if !element.optional() => {
                                match element.type_ref() {
                                    TypeReference::Value(domain) => Some(*domain),
                                    TypeReference::Schema(label) => {
                                        self.attribute_domain(&AttributeId::new(label.as_str())?)
                                    }
                                }
                            }
                            _ => None,
                        }
                    } else {
                        return Err(invalid(
                            "query_plan_unknown_function",
                            "expression references an undeclared function",
                        ));
                    };
                    if let Some(domain) = domain
                        && values
                            .insert(*assigned, domain)
                            .is_some_and(|previous| previous != domain)
                    {
                        return Err(invalid(
                            "query_plan_expression_binding",
                            "function binding has inconsistent scalar domains",
                        ));
                    }
                }
                QueryPattern::Or { branches } => {
                    for branch in branches {
                        self.function_values(branch, functions, mandatory, values)?;
                    }
                }
                QueryPattern::Try { patterns } => {
                    self.function_values(patterns, functions, mandatory, values)?
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn collect_produced_bindings(pattern: &QueryPattern, bindings: &mut BTreeSet<BindingId>) {
    match pattern {
        QueryPattern::Or { branches } => {
            for branch in branches {
                for pattern in branch {
                    collect_produced_bindings(pattern, bindings);
                }
            }
        }
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            for pattern in patterns {
                collect_produced_bindings(pattern, bindings);
            }
        }
        _ => collect_direct_positive_bindings(pattern, bindings),
    }
}
