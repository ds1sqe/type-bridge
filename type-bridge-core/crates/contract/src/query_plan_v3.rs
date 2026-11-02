//! Additive canonical read/write plans. V1/V2 representations remain frozen.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::*;
use crate::codec::from_canonical_json_with_limits;
use crate::id::TypeKind;
use crate::migration_assertion_wire::{FingerprintWire, RoleIdWire, TypeIdWire};
use crate::query_plan_wire::{
    InputColumnWire, LocalFunctionWire as CommonLocalFunctionWire, QueryBindingWire,
    QueryOperandWire as CommonOperandWire, rebuild_output, rebuild_read_stage_v2,
};
use crate::query_plan_wire::{PlanScalarWire, WireScalar, rebuild_pattern_v3};
type QueryOperandWire = CommonOperandWire<PlanScalarWire>;
type LocalFunctionWire = CommonLocalFunctionWire<PlanScalarWire>;

use crate::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema, OwnsFactId,
    PlaysFactId, RelatesFactId, SchemaFact, SchemaFactId,
};

/// Exact discriminator of the additive write-plan format.
pub const QUERY_PLAN_FORMAT_V3: &str = "typebridge.query-plan/v3";
/// Canonicalization version separating V3 fingerprints from frozen V2.
pub const QUERY_PLAN_CANONICALIZATION_V3: &str = "typebridge.query-plan-c14n/v3";
/// Domain for diagnostic plan shapes, excluding transported caller values.
pub const QUERY_PLAN_SHAPE_FINGERPRINT_DOMAIN: &str = "typebridge.query.plan-shape";
/// Read-only re-match after a selected window, retaining only visible bindings.
pub const CAP_QUERY_PIPELINE_REMATCH: &str = "query.pipeline.rematch";
/// Nonliteral IID predicates under negation; TypeDB 3.12 cannot safely execute them.
pub const CAP_NEGATED_GIVEN_IID: &str = "query.pattern.negated-given-iid";

/// Whether a frozen semantic profile admits nonliteral IID filters under negation.
#[must_use]
pub fn supports_negated_given_iid(profile: Option<&str>) -> bool {
    profile == Some("typedb-3.13.6/v1")
}

/// String scalar values above the default one-MiB byte ceiling.
pub const CAP_QUERY_INPUT_WIDE_STRINGS: &str = "query.input.wide-strings";

#[path = "query_plan_v3_document.rs"]
mod document;
#[path = "query_plan_v3_expr.rs"]
mod expression;
#[path = "query_plan_v3_seed.rs"]
mod seed;
pub use document::FetchDocumentShape;
pub use expression::{ArithOp, Builtin, Expr};
use expression::{ExprWire, operand_type};

#[path = "query_plan_v3_builder.rs"]
mod builder;
pub use builder::{BuildSelection, PlanBuilder, Site};
#[path = "query_plan_v3_bulk.rs"]
mod bulk;
use bulk::BulkChunk;
pub use bulk::{
    BuiltPlans, BulkCorrelation, BulkRowSlice, BulkRows, group_bulk_columns, group_bulk_rows,
};

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
    /// Identify a thing binding, typed by a positive pattern of the same
    /// match, by its canonical provider IID. Root-match only.
    Iid {
        /// Entity or relation binding established by this match.
        binding: BindingId,
        /// Canonical TypeDB thing IID.
        iid: String,
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
            Self::Iid { binding, iid } => {
                #[derive(Serialize)]
                struct View<'a> {
                    kind: &'static str,
                    binding: BindingId,
                    iid: &'a str,
                }
                View {
                    kind: "iid",
                    binding: *binding,
                    iid,
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

/// Every capability derivable from closed V3 syntax that a staged
/// endpoint advertises.
///
#[must_use]
pub fn query_plan_v3_capability_vocabulary() -> CapabilitySet {
    query_plan_v2_capability_vocabulary()
        .into_iter()
        .chain(
            [
                "query.plan.v3",
                CAP_QUERY_PIPELINE_REMATCH,
                CAP_NEGATED_GIVEN_IID,
                CAP_QUERY_INPUT_WIDE_STRINGS,
                "query.write.insert",
                "query.write.update",
                "query.write.delete",
                "query.write.delete-snapshot",
                "query.write.put",
                "query.fetch.thing",
                "query.fetch.scalar-fields",
                "query.fetch.role-lists",
                "query.pipeline.write-projection",
                "query.write.attach",
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
                "query.expr.iid",
                "query.pattern.nested-iid",
                "query.write.put-subtypes",
                "query.write.optional-attach",
                "query.write.attach-key",
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
#[non_exhaustive]
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
    /// Remove every player of one declared role, leaving the relation alive.
    Players {
        /// Bound relation.
        relation: BindingId,
        /// Declared role.
        role: RoleId,
    },
}

/// One addition to an already bound thing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachItem {
    /// Add one ownership value; existing values stay.
    Has {
        /// Skip this attachment when its operand is absent.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
        /// Bound owner.
        owner: BindingId,
        /// Declared ownership; key additions carry their own capability.
        field: OwnsFactId,
        /// Value to add.
        operand: QueryOperand,
    },
    /// Add one player to a declared role; existing players stay.
    Links {
        /// Skip this attachment when its player is absent.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
        /// Bound relation.
        relation: BindingId,
        /// Declared role.
        role: RoleId,
        /// Bound player.
        player: BindingId,
    },
}

/// One role whose players a thing document lists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct DocumentRole {
    /// Collect unordered players in one list instead of expanding rows.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub collect: bool,
    /// Declared role.
    pub role: RoleId,
    /// Whether the role must be filled; an unfilled required role drops
    /// the row, an unfilled optional one omits its keys.
    pub required: bool,
}

impl DocumentRole {
    /// One listed role.
    #[must_use]
    pub const fn new(role: RoleId, required: bool) -> Self {
        Self {
            role,
            required,
            collect: false,
        }
    }

    /// List unordered players as complete documents without Cartesian expansion.
    #[must_use]
    pub const fn collected(role: RoleId) -> Self {
        Self {
            role,
            required: false,
            collect: true,
        }
    }
}

/// One present typed scalar included in a thing document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DocumentScalar {
    /// Named output field, distinct from the hydration keys.
    pub key: QueryVariable,
    /// Scalar binding, or an entity/relation whose IID is emitted. An
    /// optional thing binding omits this field when absent.
    pub binding: BindingId,
}

/// The hydration document of one thing: its IID, exact type label, every
/// owned attribute and, for a relation, the players of the listed roles.
///
/// The document keys are fixed: `_iid`, `_type`, `attributes`, and for the
/// role at position `N` of `roles`: `_role_N_iid`, `_role_N_type` and
/// `_role_N_attributes`. A role with several players yields one document per
/// player combination.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ThingDocument {
    /// Additional typed scalar fields, such as a batch ordinal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scalars: Vec<DocumentScalar>,
    /// Entity or relation binding to describe.
    pub thing: BindingId,
    /// Listed roles, in key order.
    pub roles: Vec<DocumentRole>,
}

impl ThingDocument {
    /// Describe one thing and the players of `roles`.
    #[must_use]
    pub const fn new(thing: BindingId, roles: Vec<DocumentRole>) -> Self {
        Self {
            thing,
            roles,
            scalars: Vec::new(),
        }
    }
}

/// Ordered V3 pipeline vocabulary. Read tails retain the V2 order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// Add ownerships or role players to bound things, keeping existing
    /// ones (for multi-valued fields and roles, where `update` replaces).
    Attach {
        /// Additions in source order.
        items: Vec<AttachItem>,
    },
    /// Match or create things.
    Put {
        /// Reuse matching subtype instances; absent/false keeps exact matching.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        include_subtypes: bool,
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
    /// Fetch one thing document per row. Terminal; the plan's output is then
    /// empty rows and its answer is the documents.
    Fetch(ThingDocument),
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
            Self::Insert { .. }
                | Self::Update { .. }
                | Self::Delete { .. }
                | Self::Put { .. }
                | Self::Attach { .. }
        )
    }

    fn read_stage(&self) -> Option<ReadStage> {
        Some(match self {
            Self::Match { patterns } => ReadStage::Match {
                patterns: patterns
                    .iter()
                    .filter_map(|pattern| match pattern {
                        Pattern::Read(pattern) => Some(pattern.clone()),
                        Pattern::Let { .. } | Pattern::Iid { .. } => None,
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
    #[serde(skip)]
    fetch_document_shape: Option<FetchDocumentShape>,
    #[serde(skip)]
    binding_types: BTreeMap<BindingId, BTreeSet<TypeId>>,
    #[serde(skip)]
    #[expect(
        clippy::box_collection,
        reason = "optional scalar metadata must not enlarge manager futures"
    )]
    scalar_types: Option<Box<BTreeMap<BindingId, ValueTypeTag>>>,
    #[serde(skip)]
    standalone_scalar_entity: bool,
}

impl QueryPlanV3 {
    /// Schema-derived possible types observed for a binding across the pipeline.
    pub fn binding_types(&self, binding: BindingId) -> Option<&BTreeSet<TypeId>> {
        self.binding_types.get(&binding)
    }

    /// Schema-derived evidence for a terminal thing fetch; absent on other plans.
    pub fn fetch_document_shape(&self) -> Option<&FetchDocumentShape> {
        self.fetch_document_shape.as_ref()
    }

    /// Validate one program against declared schema authority and fixed limits.
    pub fn new(spec: PlanSpec, schema: &DeclaredSchema) -> Result<Self, Diagnostic> {
        Self::new_with_limits(spec, schema, QueryLimits::V3)
    }

    /// Validate with caller-selected limits, which never enter canonical bytes.
    pub fn new_with_limits(
        mut spec: PlanSpec,
        schema: &DeclaredSchema,
        limits: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        for stage in &mut spec.pipeline {
            if let Stage::Match { patterns } = stage {
                for pattern in patterns {
                    if let Pattern::Read(QueryPattern::Iid {
                        binding,
                        iid:
                            QueryOperand::Literal {
                                value: crate::value::CanonicalValue::String(iid),
                            },
                    }) = pattern
                    {
                        *pattern = Pattern::Iid {
                            binding: *binding,
                            iid: iid.as_str().to_owned(),
                        };
                    }
                }
            }
        }
        limits.validate_v3().map_err(|error| {
            let mut diagnostic = failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_invalid_limits",
                "plan limits are outside the supported protocol range",
            )
            .with_detail("limit", error.limit())
            .with_detail("source", crate::limits::LIMIT_SOURCE_CONFIGURED);
            if let crate::limits::QueryLimitsError::OutOfRange { maximum, .. } = error {
                diagnostic =
                    diagnostic.with_detail("maximum", i64::try_from(maximum).unwrap_or(i64::MAX));
            }
            diagnostic
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
        let mut binding_types = BTreeMap::new();
        let (fetch_document_shape, scalar_types) =
            validate_stages(&spec, schema, limits, &mut nodes, &mut binding_types)?;
        fn extra_zero_hop_nodes(
            pattern: &QueryPattern,
            domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
        ) -> usize {
            match pattern {
                QueryPattern::Reachable {
                    min_depth: 0,
                    source,
                    ..
                } => domains
                    .get(source)
                    .map_or(0, |types| types.len().saturating_sub(1)),
                QueryPattern::Or { branches } => {
                    branches.iter().flatten().fold(0usize, |total, pattern| {
                        total.saturating_add(extra_zero_hop_nodes(pattern, domains))
                    })
                }
                QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                    patterns.iter().fold(0usize, |total, pattern| {
                        total.saturating_add(extra_zero_hop_nodes(pattern, domains))
                    })
                }
                _ => 0,
            }
        }
        for stage in &spec.pipeline {
            if let Stage::Match { patterns } = stage {
                for pattern in patterns {
                    if let Pattern::Read(pattern) = pattern {
                        nodes = nodes.saturating_add(extra_zero_hop_nodes(pattern, &binding_types));
                        if nodes > limits.structural.predicate_nodes {
                            return Err(failure(
                                DiagnosticCategory::ResourceLimit,
                                "query_plan_reachable_expansion_limit",
                                "schema-derived zero-hop branches exceed the predicate-node ceiling",
                            ));
                        }
                    }
                }
            }
        }
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
        if has_wide_strings(&spec) {
            insert_capability(&mut required_capabilities, CAP_QUERY_INPUT_WIDE_STRINGS)?;
        }
        if spec
            .pipeline
            .iter()
            .skip(1)
            .any(|stage| matches!(stage, Stage::Match { .. }))
        {
            insert_capability(&mut required_capabilities, CAP_QUERY_PIPELINE_REMATCH)?;
        }
        add_read_given_capability(
            &spec.pipeline,
            &spec.functions,
            &spec.inputs,
            &mut required_capabilities,
        )?;
        for stage in &spec.pipeline {
            if let Stage::Match { patterns } = stage {
                for pattern in patterns {
                    match pattern {
                        Pattern::Let { expr, .. } => {
                            insert_capability(&mut required_capabilities, "query.let")?;
                            expr.capabilities(&mut required_capabilities)?;
                        }
                        Pattern::Iid { .. } => {
                            insert_capability(&mut required_capabilities, v2::CAP_IID)?;
                        }
                        Pattern::Read(_) => {}
                    }
                }
            }
            let operands: Vec<&QueryOperand> = match stage {
                Stage::Insert { things } | Stage::Put { things, .. } => things
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
                Stage::Attach { items } => items
                    .iter()
                    .filter_map(|item| match item {
                        AttachItem::Has { operand, .. } => Some(operand),
                        AttachItem::Links { .. } => None,
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
                Stage::Put {
                    include_subtypes, ..
                } => {
                    if *include_subtypes {
                        insert_capability(&mut required_capabilities, "query.write.put-subtypes")?;
                    }
                    "query.write.put"
                }
                Stage::Fetch(document) => {
                    if !document.scalars.is_empty() {
                        insert_capability(&mut required_capabilities, "query.fetch.scalar-fields")?;
                    }
                    if document.roles.iter().any(|role| role.collect) {
                        insert_capability(&mut required_capabilities, "query.fetch.role-lists")?;
                    }
                    "query.fetch.thing"
                }
                Stage::Attach { items } => {
                    for item in items {
                        match item {
                            AttachItem::Has {
                                field, optional, ..
                            } => {
                                if *optional {
                                    insert_capability(
                                        &mut required_capabilities,
                                        "query.write.optional-attach",
                                    )?;
                                }
                                let key = AnnotationFactId::new(
                                    AnnotationSubjectId::Owns(field.clone()),
                                    AnnotationKindId::Key,
                                );
                                if schema.fact(&SchemaFactId::Annotation(key)).is_some() {
                                    insert_capability(
                                        &mut required_capabilities,
                                        "query.write.attach-key",
                                    )?;
                                }
                            }
                            AttachItem::Links { optional: true, .. } => insert_capability(
                                &mut required_capabilities,
                                "query.write.optional-attach",
                            )?,
                            _ => {}
                        }
                    }
                    "query.write.attach"
                }
                _ => continue,
            };
            insert_capability(&mut required_capabilities, capability)?;
        }
        if spec
            .pipeline
            .windows(2)
            .any(|pair| matches!(pair, [Stage::Fetch(_), Stage::Delete { .. }]))
        {
            insert_capability(&mut required_capabilities, "query.write.delete-snapshot")?;
        }
        if spec.pipeline.windows(3).any(|stages| {
            matches!(&stages[..2], [Stage::Select { .. }, Stage::Distinct]) && stages[2].is_write()
        }) {
            insert_capability(
                &mut required_capabilities,
                "query.pipeline.write-projection",
            )?;
        }
        let standalone_scalar_entity = bulk::standalone_scalar_entity(&spec, schema);
        let scalar_types = match &spec.output {
            QueryOutput::Rows { columns } => scalar_types
                .into_iter()
                .filter(|(binding, _)| columns.contains(binding))
                .collect::<BTreeMap<_, _>>(),
            _ => BTreeMap::new(),
        };
        let scalar_types = (!scalar_types.is_empty()).then(|| Box::new(scalar_types));
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
            fetch_document_shape,
            binding_types,
            scalar_types,
            standalone_scalar_entity,
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
            {
                let mut limits = plan.limits;
                limits.structural.selected_slots = limits
                    .structural
                    .selected_slots
                    .min(limits.structural.bindings);
                limits
            },
        )
    }

    /// Exact format discriminator.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }
    /// Provider read/write statements charged to a staged request's budgets.
    /// A deletion snapshot adds a capture read before its write statement.
    #[must_use]
    pub fn statement_counts(&self) -> (usize, usize) {
        let writes = usize::from(self.writes());
        let capture = usize::from(
            self.pipeline
                .windows(2)
                .any(|pair| matches!(pair, [Stage::Fetch(_), Stage::Delete { .. }])),
        );
        (1 - writes + capture, writes)
    }

    /// Receiver-owned limits retained during trusted construction.
    #[must_use]
    pub const fn limits(&self) -> QueryLimits {
        self.limits
    }
    /// Dense binding declarations.
    #[must_use]
    pub fn bindings(&self) -> &[AssertionBinding] {
        &self.bindings
    }

    /// Schema-derived scalar domain for a selected row value binding.
    #[must_use]
    pub fn scalar_value_type(&self, binding: BindingId) -> Option<ValueTypeTag> {
        self.scalar_types.as_deref()?.get(&binding).copied()
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
    /// Whether the fetched root is deleted by this validated plan.
    #[must_use]
    pub fn fetched_thing_is_deleted(&self) -> bool {
        let Some(Stage::Delete { items }) = self.pipeline.last() else {
            return false;
        };
        let Some(Stage::Fetch(document)) = self.pipeline.iter().rev().nth(1) else {
            return false;
        };
        items
            .iter()
            .any(|item| matches!(item, DeleteItem::Thing { binding } if *binding == document.thing))
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
    /// Whether the plan writes, and therefore needs a write transaction.
    #[must_use]
    pub const fn writes(&self) -> bool {
        matches!(self.operation, Operation::Write { .. })
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

    /// Fingerprint the canonical program without `given_rows` for diagnostics.
    ///
    /// This separate domain never identifies an invocation or authenticates a request.
    pub fn shape_fingerprint(&self) -> Result<Fingerprint, Diagnostic> {
        let mut shape = self.clone();
        shape.given_rows = None;
        Ok(Fingerprint::compute(
            FingerprintDomain::new(QUERY_PLAN_SHAPE_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_PLAN_CANONICALIZATION_V3)?,
            None,
            &shape.canonical_bytes()?,
        ))
    }
}

fn has_wide_strings(spec: &PlanSpec) -> bool {
    fn wide(value: &CanonicalValue) -> bool {
        matches!(value, CanonicalValue::String(text) if text.as_str().len() > crate::limits::MAX_CANONICAL_STRING_BYTES)
    }
    fn operand(value: &QueryOperand) -> bool {
        matches!(value, QueryOperand::Literal { value } if wide(value))
    }
    fn read(pattern: &QueryPattern) -> bool {
        match pattern {
            QueryPattern::Value { left, right, .. } => operand(left) || operand(right),
            QueryPattern::FunctionCall { arguments, .. } => arguments.iter().any(operand),
            QueryPattern::Or { branches } => branches.iter().flatten().any(read),
            QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                patterns.iter().any(read)
            }
            _ => false,
        }
    }
    fn expr(value: &Expr) -> bool {
        match value {
            Expr::Operand(value) => operand(value),
            Expr::Arith { lhs, rhs, .. } => expr(lhs) || expr(rhs),
            Expr::Call { args, .. } => args.iter().any(expr),
            Expr::Iid { .. } => false,
        }
    }
    spec.given_rows
        .iter()
        .flatten()
        .flat_map(|row| row.values())
        .flatten()
        .any(wide)
        || spec
            .functions
            .iter()
            .flat_map(|function| function.body())
            .any(read)
        || spec.pipeline.iter().any(|stage| match stage {
            Stage::Match { patterns } => patterns.iter().any(|pattern| match pattern {
                Pattern::Read(pattern) => read(pattern),
                Pattern::Let { expr: value, .. } => expr(value),
                Pattern::Iid { .. } => false,
            }),
            Stage::Insert { things } | Stage::Put { things, .. } => things
                .iter()
                .flat_map(|thing| &thing.fields)
                .any(|(_, value)| operand(value)),
            Stage::Update { sets } => sets
                .iter()
                .any(|set| matches!(set, Set::Field { operand: value, .. } if operand(value))),
            Stage::Attach { items } => items.iter().any(
                |item| matches!(item, AttachItem::Has { operand: value, .. } if operand(value)),
            ),
            _ => false,
        })
}

fn require_unordered_write(mode: crate::schema::CollectionMode) -> Result<(), Diagnostic> {
    if mode.is_unordered() {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::UnsupportedCapability,
            "query_plan_ordered_collection_unsupported",
            "Supported TypeDB providers do not implement ordered list DML",
        ))
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
    binding_types: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
) -> Result<
    (
        Option<FetchDocumentShape>,
        BTreeMap<BindingId, ValueTypeTag>,
    ),
    Diagnostic,
> {
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
    let mut rematched = false;
    let mut fetched: Option<&ThingDocument> = None;
    let mut captured_environment = None;
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
        if reading && stage.is_write() {
            if !matches!(
                read_tail.as_slice(),
                [ReadStage::Select { .. }, ReadStage::Distinct]
            ) || !matches!(spec.operation, Operation::Write { .. })
            {
                return Err(invalid(
                    "query_plan_stage_order",
                    "only select followed by distinct may separate write stages",
                ));
            }
            (mandatory, optional) = validate_v3_read_tail(
                &read_tail,
                mandatory,
                optional,
                &referenced,
                spec.bindings.len(),
                limits.structural,
                &scalar_types,
            )?;
            types.retain(|binding, _| mandatory.contains(binding) || optional.contains(binding));
            scalar_types
                .retain(|binding, _| mandatory.contains(binding) || optional.contains(binding));
            read_tail.clear();
            reading = false;
        }
        match stage {
            Stage::Match { patterns } => {
                let retained = if stage_index == 0 {
                    None
                } else {
                    let writing = matches!(spec.operation, Operation::Write { .. });
                    if (!writing && rematched)
                        || (writing
                            && !matches!(
                                read_tail.as_slice(),
                                [ReadStage::Select { .. }, ReadStage::Distinct]
                            ))
                        || !matches!(read_tail.first(), Some(ReadStage::Select { .. }))
                        || !read_tail
                            .iter()
                            .any(|stage| matches!(stage, ReadStage::Distinct))
                        || read_tail.iter().any(|stage| {
                            !matches!(
                                stage,
                                ReadStage::Select { .. }
                                    | ReadStage::Distinct
                                    | ReadStage::Sort { .. }
                                    | ReadStage::Offset { .. }
                                    | ReadStage::Limit { .. }
                            )
                        })
                    {
                        return Err(invalid(
                            "query_plan_stage_order",
                            "match occurs only at the start or after a read-only selected window",
                        ));
                    }
                    let visible = validate_v3_read_tail(
                        &read_tail,
                        std::mem::take(&mut mandatory),
                        std::mem::take(&mut optional),
                        &referenced,
                        spec.bindings.len(),
                        limits.structural,
                        &scalar_types,
                    )?;
                    read_tail.clear();
                    types.retain(|binding, _| {
                        visible.0.contains(binding) || visible.1.contains(binding)
                    });
                    scalar_types.retain(|binding, _| {
                        visible.0.contains(binding) || visible.1.contains(binding)
                    });
                    rematched = true;
                    Some(visible)
                };
                reading = false;
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
                if let Some((retained_mandatory, retained_optional)) = retained {
                    mandatory.extend(retained_mandatory);
                    optional.extend(retained_optional);
                    optional.retain(|binding| !mandatory.contains(binding));
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
                for (binding, domains) in &types {
                    binding_types
                        .entry(*binding)
                        .or_default()
                        .extend(domains.iter().cloned());
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
                        let things = mandatory
                            .iter()
                            .filter(|binding| {
                                types.get(binding).is_some_and(|domains| {
                                    !domains.is_empty()
                                        && domains.iter().all(|domain| {
                                            matches!(
                                                domain.kind(),
                                                TypeKind::Entity | TypeKind::Relation
                                            )
                                        })
                                })
                            })
                            .copied()
                            .collect();
                        let domain =
                            expr.infer(&scalar_types, &spec.inputs, &things, limits, 1, nodes)?;
                        mandatory.insert(*binding);
                        referenced.insert(*binding);
                        scalar_types.insert(*binding, domain);
                    }
                }
                validate_comparisons(&reads, &scalar_types, &spec.inputs, &index)?;
                if reads.iter().any(contains_nested_iid) {
                    validate_nested_iids(
                        &reads,
                        &types,
                        &mandatory,
                        &index,
                        &spec.inputs,
                        spec.given_rows.as_deref(),
                        &mut BTreeSet::new(),
                    )?;
                }
                for pattern in patterns {
                    if let Pattern::Iid { binding, iid } = pattern {
                        check_binding(*binding, spec.bindings.len())?;
                        if !crate::id::is_canonical_thing_iid(iid) {
                            return Err(invalid(
                                "query_plan_invalid_iid",
                                "IID patterns require a canonical TypeDB thing IID",
                            ));
                        }
                        let thing = mandatory.contains(binding)
                            && types.get(binding).is_some_and(|types| {
                                !types.is_empty()
                                    && types.iter().all(|type_id| {
                                        matches!(
                                            type_id.kind(),
                                            TypeKind::Entity | TypeKind::Relation
                                        )
                                    })
                            });
                        if !thing {
                            return Err(invalid(
                                "query_plan_iid_untyped_thing",
                                "IID patterns require a positively typed thing binding",
                            ));
                        }
                    }
                }
            }
            Stage::Insert { things } | Stage::Put { things, .. } => {
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
                        let deferred_players = seed::players_proven(
                            spec,
                            stage_index,
                            thing.binding,
                            &mandatory,
                            &optional,
                        );
                        let keyed_seed = matches!(stage, Stage::Put { .. })
                            && !thing.fields.is_empty()
                            && thing.fields.iter().all(|(field, _)| {
                                index.annotation(
                                    AnnotationSubjectId::Owns(field.clone()),
                                    AnnotationKindId::Key,
                                )
                            })
                            && deferred_players;
                        let insert_seed = matches!(stage, Stage::Insert { .. }) && deferred_players;
                        if !keyed_seed && !insert_seed {
                            return Err(invalid(
                                "query_plan_insert_relation_players",
                                "relation insert and put require players, or a keyed put followed by player attachment",
                            ));
                        }
                    }
                    let mut candidates = BTreeSet::from([thing.type_id.clone()]);
                    if matches!(
                        stage,
                        Stage::Put {
                            include_subtypes: true,
                            ..
                        }
                    ) {
                        index.exists(&thing.type_id)?;
                        for fact in index.schema.facts() {
                            if let SchemaFact::Type(fact) = fact
                                && index.ancestors(fact.id()).contains(&thing.type_id)
                            {
                                candidates.insert(fact.id().clone());
                            }
                        }
                    } else {
                        index.concrete(&thing.type_id)?;
                    }
                    types.insert(thing.binding, candidates);
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
            Stage::Attach { items } => {
                if reading {
                    return Err(invalid(
                        "query_plan_stage_order",
                        "write stages precede the read tail",
                    ));
                }
                nonempty(items.len(), limits, nodes)?;
                for item in items {
                    match item {
                        AttachItem::Has {
                            owner,
                            field,
                            operand,
                            optional: allow_absent,
                        } => {
                            if !(*allow_absent && optional.contains(owner)) {
                                bound(*owner, &mandatory)?;
                            }
                            index.ownership(*owner, field, &types, false)?;
                            if let QueryOperand::Input { column } = operand
                                && *allow_absent
                            {
                                validate_operand(
                                    operand,
                                    &mandatory,
                                    &optional,
                                    spec.inputs.len(),
                                )?;
                                if index.attribute_domain(field.attribute())
                                    != Some(spec.inputs[usize::from(column.get())].value_type())
                                {
                                    return Err(invalid(
                                        "query_plan_write_value_type",
                                        "write operand differs from the ownership scalar domain",
                                    ));
                                }
                            } else if let QueryOperand::Binding { binding } = operand
                                && *allow_absent
                                && optional.contains(binding)
                            {
                                let domain = types
                                    .get(binding)
                                    .and_then(|types| index.scalar_domain(types))
                                    .ok_or_else(|| {
                                        invalid(
                                            "query_plan_attach_operand_type",
                                            "optional attachment requires a typed scalar",
                                        )
                                    })?;
                                let mut local = scalar_types.clone();
                                local.insert(*binding, domain);
                                index.field_value(field, operand, &local, &spec.inputs)?;
                            } else {
                                validate_operand(
                                    operand,
                                    &mandatory,
                                    &optional,
                                    spec.inputs.len(),
                                )?;
                                index.field_value(field, operand, &scalar_types, &spec.inputs)?;
                            }
                        }
                        AttachItem::Links {
                            relation,
                            role,
                            player,
                            optional: allow_absent,
                        } => {
                            bound(*relation, &mandatory)?;
                            if !(*allow_absent && optional.contains(player)) {
                                bound(*player, &mandatory)?;
                            }
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
                            if !mandatory.contains(owner) {
                                bound(*owner, &optional)?;
                            }
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
                        DeleteItem::Players { relation, role } => {
                            bound(*relation, &mandatory)?;
                            index.relation_role(*relation, role, &types)?;
                        }
                    }
                }
                for item in items {
                    if let DeleteItem::Thing { binding } = item {
                        mandatory.remove(binding);
                    }
                }
            }
            Stage::Fetch(document) => {
                let snapshot = stage_index + 2 == spec.pipeline.len()
                    && matches!(spec.pipeline.last(), Some(Stage::Delete { .. }));
                if stage_index + 1 != spec.pipeline.len() && !snapshot {
                    return Err(invalid(
                        "query_plan_fetch_not_last",
                        "a thing fetch is terminal or immediately precedes a terminal delete",
                    ));
                }
                if snapshot {
                    // Replaying a write prefix would apply mutations twice.
                    if spec.pipeline[..stage_index].iter().any(|stage| {
                        matches!(
                            stage,
                            Stage::Insert { .. }
                                | Stage::Put { .. }
                                | Stage::Update { .. }
                                | Stage::Attach { .. }
                                | Stage::Delete { .. }
                        )
                    }) {
                        return Err(invalid(
                            "query_plan_fetch_delete_prefix",
                            "a deletion snapshot requires a read-only prefix",
                        ));
                    }
                    let Some(Stage::Delete { items }) = spec.pipeline.last() else {
                        unreachable!()
                    };
                    if !matches!(items.as_slice(), [DeleteItem::Thing { binding }] if *binding == document.thing)
                    {
                        return Err(invalid(
                            "query_plan_fetch_delete_target",
                            "a deletion snapshot captures exactly the thing being deleted",
                        ));
                    }
                    // The answer retains authority from before its root is deleted.
                    captured_environment = Some((mandatory.clone(), optional.clone()));
                }
                if !matches!(
                    spec.operation,
                    Operation::Rows
                        | Operation::Write {
                            returns: WriteReturns::Rows
                        }
                ) || !matches!(&spec.output, QueryOutput::Rows { columns } if columns.is_empty())
                {
                    return Err(invalid(
                        "query_plan_fetch_output",
                        "a thing fetch answers rows with an empty declared output",
                    ));
                }
                check_binding(document.thing, spec.bindings.len())?;
                nonempty(
                    document.roles.len() + document.scalars.len() + 1,
                    limits,
                    nodes,
                )?;
                let mut keys = BTreeSet::new();
                for scalar in &document.scalars {
                    check_binding(scalar.binding, spec.bindings.len())?;
                    let key = scalar.key.as_str();
                    if key.starts_with('_') || key == "attributes" || !keys.insert(key) {
                        return Err(invalid(
                            "query_plan_fetch_scalar_key",
                            "scalar fetch keys must be unique and distinct from hydration keys",
                        ));
                    }
                }
                for (index, role) in document.roles.iter().enumerate() {
                    if document.roles[..index]
                        .iter()
                        .any(|earlier| earlier.role == role.role)
                    {
                        return Err(invalid(
                            "query_plan_fetch_duplicate_role",
                            "a thing fetch lists each role once",
                        ));
                    }
                }
                fetched = Some(document);
            }
            _ => {
                reading = true;
                read_tail.push(stage.read_stage().expect("read stage"));
            }
        }
    }
    let (mandatory, optional) = validate_v3_read_tail(
        &read_tail,
        mandatory,
        optional,
        &referenced,
        spec.bindings.len(),
        limits.structural,
        &scalar_types,
    )?;
    for (binding, domains) in &types {
        binding_types
            .entry(*binding)
            .or_default()
            .extend(domains.iter().cloned());
    }
    if let Some(document) = fetched {
        let (answer_mandatory, answer_optional) = captured_environment
            .as_ref()
            .map_or((&mandatory, &optional), |(mandatory, optional)| {
                (mandatory, optional)
            });
        index.thing_document(document, answer_mandatory, answer_optional, &types)?;
        for scalar in &document.scalars {
            let present_scalar = answer_mandatory.contains(&scalar.binding)
                && !answer_optional.contains(&scalar.binding)
                && scalar_types.contains_key(&scalar.binding);
            let thing_iid = (answer_mandatory.contains(&scalar.binding)
                || answer_optional.contains(&scalar.binding))
                && types.get(&scalar.binding).is_some_and(|domains| {
                    !domains.is_empty()
                        && domains.iter().all(|domain| {
                            matches!(domain.kind(), TypeKind::Entity | TypeKind::Relation)
                        })
                });
            if !present_scalar && !thing_iid {
                return Err(invalid(
                    "query_plan_fetch_scalar_binding",
                    "scalar fetch fields require a present scalar or bound entity/relation IID",
                ));
            }
        }
        let profile = crate::semantic_profile::SemanticProfile::resolve(
            spec.managed_semantics
                .as_fingerprint()
                .semantic_profile()
                .expect("managed schema fingerprints carry a profile"),
        )?;
        return Ok((
            Some(FetchDocumentShape::new(
                &index,
                document,
                &types,
                &scalar_types,
                answer_optional,
                &profile,
            )),
            scalar_types,
        ));
    } else if !matches!(&spec.output, QueryOutput::Rows { columns } if columns.is_empty() && matches!(spec.operation,Operation::Count|Operation::Exists|Operation::Write{returns:WriteReturns::Matched}))
    {
        validate_output(&spec.output, &mandatory, &optional, limits.structural)?;
    }
    Ok((None, scalar_types))
}

fn validate_v3_read_tail(
    read_tail: &[ReadStage],
    mandatory: BTreeSet<BindingId>,
    optional: BTreeSet<BindingId>,
    referenced: &BTreeSet<BindingId>,
    binding_count: usize,
    limits: StructuralLimits,
    scalar_types: &BTreeMap<BindingId, ValueTypeTag>,
) -> Result<(BTreeSet<BindingId>, BTreeSet<BindingId>), Diagnostic> {
    let (mandatory, optional, has_sort) = validate_read_stages(
        read_tail,
        mandatory,
        optional,
        referenced,
        binding_count,
        limits,
    )?;
    for stage in read_tail {
        if let ReadStage::Sort { terms } = stage
            && terms
                .iter()
                .any(|term| scalar_types.get(&term.binding()) == Some(&ValueTypeTag::Duration))
        {
            return Err(invalid(
                "query_plan_sort_unordered_value",
                "sort requires an ordered value type; durations have no total order",
            ));
        }
    }
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
    Ok((mandatory, optional))
}

/// TypeDB 3.14 refuses range comparisons of durations (3.13 accepted them);
/// refuse them at build time on every profile so a plan fails the same way
/// before reaching any server. Operands whose domain is unknown here
/// (branch-local bindings) are left to the server.
fn validate_comparisons(
    patterns: &[QueryPattern],
    scalar_types: &BTreeMap<BindingId, ValueTypeTag>,
    inputs: &[InputColumn],
    index: &SchemaIndex<'_>,
) -> Result<(), Diagnostic> {
    use crate::migration_assertion::ValueComparator;
    let mut scalar_types = scalar_types.clone();
    let mut local_types = BTreeMap::new();
    for pattern in patterns {
        index.pattern_types(pattern, &mut local_types)?;
    }
    for (binding, types) in local_types {
        if let Some(domain) = index.scalar_domain(&types) {
            scalar_types.insert(binding, domain);
        }
    }
    let duration = |operand: &QueryOperand| match operand {
        QueryOperand::Literal { value } => value.value_type() == ValueTypeTag::Duration,
        QueryOperand::Binding { binding } => {
            scalar_types.get(binding) == Some(&ValueTypeTag::Duration)
        }
        QueryOperand::Input { column } => inputs
            .get(usize::from(column.get()))
            .is_some_and(|input| input.value_type() == ValueTypeTag::Duration),
    };
    for pattern in patterns {
        if let QueryPattern::Value {
            comparator,
            left,
            right,
        } = pattern
            && matches!(
                comparator,
                ValueComparator::Contains
                    | ValueComparator::StartsWith
                    | ValueComparator::EndsWith
                    | ValueComparator::Like
            )
        {
            if matches!(
                comparator,
                ValueComparator::StartsWith | ValueComparator::EndsWith | ValueComparator::Like
            ) && matches!(right, QueryOperand::Binding { .. })
            {
                return Err(invalid(
                    "query_plan_compare_regex_literal",
                    "like requires a literal or bound string input",
                ));
            }
            for operand in [left, right] {
                if operand_type(operand, &scalar_types, inputs).ok() != Some(ValueTypeTag::String) {
                    return Err(invalid(
                        "query_plan_compare_string_type",
                        "string comparisons require string operands",
                    ));
                }
            }
        }
        match pattern {
            QueryPattern::Value {
                comparator,
                left,
                right,
            } if matches!(
                comparator,
                ValueComparator::Less
                    | ValueComparator::LessOrEqual
                    | ValueComparator::Greater
                    | ValueComparator::GreaterOrEqual
            ) && (duration(left) || duration(right)) =>
            {
                return Err(invalid(
                    "query_plan_duration_range",
                    "durations admit only equality comparison",
                ));
            }
            QueryPattern::Or { branches } => {
                for branch in branches {
                    validate_comparisons(branch, &scalar_types, inputs, index)?;
                }
            }
            QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                validate_comparisons(patterns, &scalar_types, inputs, index)?;
            }
            _ => {}
        }
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
        let constrain = |types: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
                         binding,
                         candidates: BTreeSet<TypeId>| {
            if let Some(previous) = types.get_mut(&binding) {
                previous.retain(|candidate| candidates.contains(candidate));
            } else {
                types.insert(binding, candidates);
            }
        };
        match pattern {
            QueryPattern::Isa {
                binding,
                type_id,
                include_subtypes,
            } => {
                self.exists(type_id)?;
                let mut candidates = BTreeSet::from([type_id.clone()]);
                if *include_subtypes {
                    for fact in self.schema.facts() {
                        if let SchemaFact::Type(fact) = fact
                            && self.ancestors(fact.id()).contains(type_id)
                        {
                            candidates.insert(fact.id().clone());
                        }
                    }
                }
                constrain(types, *binding, candidates);
            }
            QueryPattern::Has {
                attribute,
                attribute_id,
                ..
            } => {
                let id = TypeId::new(TypeKind::Attribute, attribute_id.label().as_str())?;
                self.exists(&id)?;
                constrain(types, *attribute, BTreeSet::from([id]));
            }
            QueryPattern::Links {
                relation,
                relation_id,
                ..
            } => {
                self.exists(relation_id)?;
                constrain(types, *relation, BTreeSet::from([relation_id.clone()]));
            }
            QueryPattern::Or { branches } => {
                let mut alternatives = BTreeMap::<BindingId, BTreeSet<TypeId>>::new();
                for branch in branches {
                    let mut local = types.clone();
                    for pattern in branch {
                        self.pattern_types(pattern, &mut local)?;
                    }
                    for (binding, candidates) in local {
                        alternatives.entry(binding).or_default().extend(candidates);
                    }
                }
                *types = alternatives;
            }
            QueryPattern::Try { patterns } => {
                let mut local = types.clone();
                for pattern in patterns {
                    self.pattern_types(pattern, &mut local)?;
                }
                for (binding, candidates) in local {
                    types.entry(binding).or_insert(candidates);
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
        match self.schema.fact(&SchemaFactId::Owns(field.clone())) {
            Some(SchemaFact::Owns(fact)) => require_unordered_write(fact.collection_mode())?,
            _ => {
                return Err(invalid(
                    "query_plan_undeclared_owns",
                    "write ownership is not declared",
                ));
            }
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
    /// The fetched thing is a present entity or relation; every listed role
    /// is declared by a relation type the thing can be.
    fn thing_document(
        &self,
        document: &ThingDocument,
        mandatory: &BTreeSet<BindingId>,
        optional: &BTreeSet<BindingId>,
        types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    ) -> Result<(), Diagnostic> {
        let candidates = types
            .get(&document.thing)
            .filter(|candidates| {
                mandatory.contains(&document.thing)
                    && !optional.contains(&document.thing)
                    && !candidates.is_empty()
                    && candidates.iter().all(|type_id| {
                        matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation)
                    })
            })
            .ok_or_else(|| {
                invalid(
                    "query_plan_fetch_untyped_thing",
                    "a thing fetch requires a present entity or relation binding",
                )
            })?;
        for listed in &document.roles {
            let owner = TypeId::new(
                TypeKind::Relation,
                listed.role.declaring_relation().as_str(),
            )?;
            let relates = RelatesFactId::new(owner.clone(), listed.role.clone())?;
            if listed.collect
                && matches!(self.schema.fact(&SchemaFactId::Relates(relates.clone())), Some(SchemaFact::Relates(fact)) if fact.collection_mode() == crate::schema::CollectionMode::OrderedList)
            {
                return Err(invalid(
                    "query_plan_fetch_ordered_role",
                    "collected role documents require unordered roles",
                ));
            }
            if self.schema.fact(&SchemaFactId::Relates(relates)).is_none() {
                return Err(invalid(
                    "query_plan_undeclared_relates",
                    "fetched role is not declared",
                ));
            }
            let belongs = |id: &TypeId| self.ancestors(id).contains(&owner);
            if (listed.required && !candidates.iter().all(belongs))
                || !candidates.iter().any(belongs)
            {
                return Err(invalid(
                    "query_plan_wrong_role_owner",
                    "role does not belong to the relation type",
                ));
            }
        }
        Ok(())
    }

    /// The role is declared by a relation type the bound relation can be.
    fn relation_role(
        &self,
        relation: BindingId,
        role: &RoleId,
        types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    ) -> Result<(), Diagnostic> {
        let owner = TypeId::new(TypeKind::Relation, role.declaring_relation().as_str())?;
        let relates = RelatesFactId::new(owner.clone(), role.clone())?;
        match self.schema.fact(&SchemaFactId::Relates(relates)) {
            Some(SchemaFact::Relates(fact)) => require_unordered_write(fact.collection_mode())?,
            _ => {
                return Err(invalid(
                    "query_plan_undeclared_relates",
                    "write role is not declared",
                ));
            }
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
        Ok(())
    }

    fn role(
        &self,
        relation: BindingId,
        role: &RoleId,
        player: BindingId,
        types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    ) -> Result<(), Diagnostic> {
        self.relation_role(relation, role, types)?;
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
    given_rows: Option<Vec<Vec<Option<PlanScalarWire>>>>,
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
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<InsertThing, Diagnostic> {
        Ok(InsertThing {
            binding: BindingId::new(self.binding)?,
            type_id: self.type_id.rebuild()?,
            fields: self
                .fields
                .into_iter()
                .map(|(field, value)| Ok((field.rebuild()?, value.rebuild_with_limits(limits)?)))
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
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<Set, Diagnostic> {
        Ok(match self {
            Self::Field {
                owner,
                field,
                operand,
            } => Set::Field {
                owner: BindingId::new(owner)?,
                field: field.rebuild()?,
                operand: operand.rebuild_with_limits(limits)?,
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
    Players {
        relation: u16,
        role: RoleIdWire,
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
            Self::Players { relation, role } => DeleteItem::Players {
                relation: BindingId::new(relation)?,
                role: role.rebuild()?,
            },
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AttachWire {
    Has {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
        owner: u16,
        field: OwnsWire,
        operand: QueryOperandWire,
    },
    Links {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
        relation: u16,
        role: RoleIdWire,
        player: u16,
    },
}
impl AttachWire {
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<AttachItem, Diagnostic> {
        Ok(match self {
            Self::Has {
                optional,
                owner,
                field,
                operand,
            } => AttachItem::Has {
                optional,
                owner: BindingId::new(owner)?,
                field: field.rebuild()?,
                operand: operand.rebuild_with_limits(limits)?,
            },
            Self::Links {
                optional,
                relation,
                role,
                player,
            } => AttachItem::Links {
                optional,
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
    Attach {
        items: Vec<AttachWire>,
    },
    Insert {
        things: Vec<ThingWire>,
    },
    Update {
        sets: Vec<SetWire>,
    },
    Delete {
        items: Vec<DeleteWire>,
    },
    Put {
        things: Vec<ThingWire>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_subtypes: bool,
    },
}
impl WriteWire {
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<Stage, Diagnostic> {
        Ok(match self {
            Self::Attach { items } => Stage::Attach {
                items: items
                    .into_iter()
                    .map(|value| value.rebuild(limits))
                    .collect::<Result<_, _>>()?,
            },
            Self::Insert { things } => Stage::Insert {
                things: things
                    .into_iter()
                    .map(|value| value.rebuild(limits))
                    .collect::<Result<_, _>>()?,
            },
            Self::Put {
                things,
                include_subtypes,
            } => Stage::Put {
                include_subtypes,
                things: things
                    .into_iter()
                    .map(|value| value.rebuild(limits))
                    .collect::<Result<_, _>>()?,
            },
            Self::Update { sets } => Stage::Update {
                sets: sets
                    .into_iter()
                    .map(|value| value.rebuild(limits))
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
fn decode_stage(
    value: serde_json::Value,
    limits: crate::limits::CodecLimits,
) -> Result<Stage, Diagnostic> {
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
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct IidWire {
            kind: String,
            binding: u16,
            iid: String,
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
                            expr: wire.expr.rebuild(limits)?,
                        })
                    } else if value.get("kind").and_then(serde_json::Value::as_str) == Some("iid")
                        && value.get("iid").is_some_and(serde_json::Value::is_string)
                    {
                        let wire: IidWire = serde_json::from_value(value).map_err(|_| {
                            invalid("invalid_canonical_value", "invalid iid pattern")
                        })?;
                        debug_assert_eq!(wire.kind, "iid");
                        Ok(Pattern::Iid {
                            binding: BindingId::new(wire.binding)?,
                            iid: wire.iid,
                        })
                    } else {
                        Ok(Pattern::Read(rebuild_pattern_v3(value, limits)?))
                    }
                })
                .collect::<Result<_, Diagnostic>>()?,
        });
    }

    if value.get("kind").and_then(serde_json::Value::as_str) == Some("fetch") {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RoleWire {
            #[serde(default)]
            collect: bool,
            required: bool,
            role: RoleIdWire,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ScalarWire {
            key: String,
            binding: u16,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FetchWire {
            #[serde(default)]
            scalars: Vec<ScalarWire>,
            kind: String,
            roles: Vec<RoleWire>,
            thing: u16,
        }
        let wire: FetchWire = serde_json::from_value(value)
            .map_err(|_| invalid("invalid_canonical_value", "invalid fetch stage"))?;
        debug_assert_eq!(wire.kind, "fetch");
        let mut document = ThingDocument::new(
            BindingId::new(wire.thing)?,
            wire.roles
                .into_iter()
                .map(|role| {
                    let mut listed = DocumentRole::new(role.role.rebuild()?, role.required);
                    listed.collect = role.collect;
                    Ok(listed)
                })
                .collect::<Result<_, Diagnostic>>()?,
        );
        document.scalars = wire
            .scalars
            .into_iter()
            .map(|scalar| {
                Ok(DocumentScalar {
                    key: QueryVariable::new(scalar.key)?,
                    binding: BindingId::new(scalar.binding)?,
                })
            })
            .collect::<Result<_, Diagnostic>>()?;
        return Ok(Stage::Fetch(document));
    }
    if matches!(
        value.get("kind").and_then(serde_json::Value::as_str),
        Some("insert" | "update" | "delete" | "put" | "attach")
    ) {
        serde_json::from_value::<WriteWire>(value)
            .map_err(|_| invalid("invalid_canonical_value", "invalid write stage"))?
            .rebuild(limits)
    } else {
        Ok(rebuild_read_stage_v2(value)?.into())
    }
}

/// Decode exact canonical V3 bytes using local schema authority.
pub fn decode_query_plan_v3(
    bytes: &[u8],
    schema: &DeclaredSchema,
) -> Result<QueryPlanV3, Diagnostic> {
    decode_query_plan_v3_with_limits(bytes, schema, QueryLimits::V3)
}

/// Decode under caller-selected codec and structural ceilings.
pub fn decode_query_plan_v3_with_limits(
    bytes: &[u8],
    schema: &DeclaredSchema,
    limits: QueryLimits,
) -> Result<QueryPlanV3, Diagnostic> {
    let wire: PlanWire = from_canonical_json_with_limits(bytes, limits.codec)
        .map_err(|error| limits.annotate(error))?;
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
                .map(|rows| {
                    rows.into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|value| {
                                    value.map(|value| value.rebuild(limits.codec)).transpose()
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .map(InputRow::new)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            operation: wire.operation,
            inputs: wire
                .inputs
                .into_iter()
                .map(InputColumnWire::rebuild)
                .collect::<Result<_, _>>()?,
            pipeline: wire
                .pipeline
                .into_iter()
                .map(|stage| decode_stage(stage, limits.codec))
                .collect::<Result<_, _>>()?,
            output: rebuild_output(wire.output)?,
            managed_semantics: ManagedSemanticSchemaFingerprint::from_wire(
                wire.managed_semantics.rebuild()?,
            )?,
            compatibility: wire.compatibility,
            functions: wire
                .functions
                .into_iter()
                .map(|function| function.rebuild_with_limits(limits.codec))
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
    if matches!(
        spec.pipeline.as_slice(),
        [.., Stage::Fetch(_), Stage::Delete { .. }]
    ) && !matches!(
        spec.operation,
        Operation::Write {
            returns: WriteReturns::Rows
        }
    ) {
        return Err(invalid(
            "query_plan_fetch_delete_operation",
            "a deletion snapshot requires write returning rows",
        ));
    }
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

impl AsRef<[QueryPlanV3]> for QueryPlanV3 {
    fn as_ref(&self) -> &[QueryPlanV3] {
        std::slice::from_ref(self)
    }
}

fn contains_nested_iid(pattern: &QueryPattern) -> bool {
    match pattern {
        QueryPattern::Iid { .. } => true,
        QueryPattern::Or { branches } => branches.iter().flatten().any(contains_nested_iid),
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            patterns.iter().any(contains_nested_iid)
        }
        _ => false,
    }
}

fn validate_nested_iids(
    patterns: &[QueryPattern],
    outer_types: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    outer_bound: &BTreeSet<BindingId>,
    index: &SchemaIndex<'_>,
    inputs: &[InputColumn],
    rows: Option<&[InputRow]>,
    checked_inputs: &mut BTreeSet<InputColumnId>,
) -> Result<(), Diagnostic> {
    let mut types = outer_types.clone();
    let mut bound = outer_bound.clone();
    for pattern in patterns {
        index.pattern_types(pattern, &mut types)?;
        collect_direct_positive_bindings(pattern, &mut bound);
    }
    for pattern in patterns {
        match pattern {
            QueryPattern::Iid { binding, iid } => {
                let domain = match iid {
                    QueryOperand::Literal { value } => Some(value.value_type()),
                    QueryOperand::Input { column } => inputs
                        .get(usize::from(column.get()))
                        .map(InputColumn::value_type),
                    QueryOperand::Binding { binding } if bound.contains(binding) => types
                        .get(binding)
                        .and_then(|types| index.scalar_domain(types)),
                    QueryOperand::Binding { .. } => None,
                };
                if domain != Some(ValueTypeTag::String) {
                    return Err(invalid(
                        "query_plan_iid_operand_type",
                        "IID comparisons require a present string operand",
                    ));
                }
                if let QueryOperand::Input { column } = iid
                    && checked_inputs.insert(*column)
                {
                    for row in rows.unwrap_or_default() {
                        if let Some(value) = &row.values()[usize::from(column.get())]
                            && !matches!(value, crate::value::CanonicalValue::String(iid) if crate::id::is_canonical_thing_iid(iid.as_str()))
                        {
                            return Err(invalid(
                                "query_plan_invalid_iid",
                                "IID inputs require canonical TypeDB thing IIDs",
                            ));
                        }
                    }
                }
                if !bound.contains(binding)
                    || !types.get(binding).is_some_and(|types| {
                        !types.is_empty()
                            && types.iter().all(|ty| {
                                matches!(ty.kind(), TypeKind::Entity | TypeKind::Relation)
                            })
                    })
                {
                    return Err(invalid(
                        "query_plan_iid_untyped_thing",
                        "IID patterns require a positively typed thing in their lexical scope",
                    ));
                }
            }
            QueryPattern::Or { branches } => {
                for branch in branches {
                    validate_nested_iids(
                        branch,
                        &types,
                        &bound,
                        index,
                        inputs,
                        rows,
                        checked_inputs,
                    )?;
                }
            }
            QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                validate_nested_iids(
                    patterns,
                    &types,
                    &bound,
                    index,
                    inputs,
                    rows,
                    checked_inputs,
                )?
            }
            _ => {}
        }
    }
    Ok(())
}
