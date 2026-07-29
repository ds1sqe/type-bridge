//! Private fail-closed wire DTOs for reusable query plans.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::CapabilitySet;
use crate::codec::from_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::id::{AttributeId, FunctionId, Label};
use crate::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use crate::migration_assertion_wire::{
    AssertionRolePlayerWire, FingerprintWire, RoleIdWire, TypeIdWire, ValueComparatorWire,
};
use crate::query_plan::{
    DocumentField, DocumentSource, InputColumn, InputColumnId, LocalFunction, LocalReturn,
    OrderDirection, OrderTerm, QUERY_PLAN_FORMAT_V1, QUERY_PLAN_FORMAT_V2, QueryOperand,
    QueryOutput, QueryPattern, QueryPlan, QueryPlanV2Compatibility, ReadStage, ReduceAssignment,
    Reducer, failure,
};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use crate::value::{CanonicalValue, ValueTypeTag};

pub(crate) fn decode_query_plan(bytes: &[u8]) -> Result<QueryPlan, Diagnostic> {
    // Inspect only canonical JSON and the discriminator before deserializing
    // any version-specific DTO. A V2 payload can never partially reconstruct
    // through V1 types, and an unknown version cannot select a permissive
    // fallback.
    let value = from_canonical_json::<Value>(bytes)?;
    let format = value
        .as_object()
        .and_then(|object| object.get("format"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "invalid_canonical_value",
                "canonical JSON does not satisfy the requested contract type",
            )
        })?;
    let trusted = match format {
        QUERY_PLAN_FORMAT_V1 => from_canonical_json::<QueryPlanWire>(bytes)?.rebuild()?,
        QUERY_PLAN_FORMAT_V2 => from_canonical_json::<QueryPlanWireV2>(bytes)?.rebuild()?,
        _ => {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_format_unsupported",
                "query plan wire format is unsupported",
            ));
        }
    };
    if trusted.canonical_bytes()? != bytes {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_plan_wire_mismatch",
            "query plan bytes normalize after trusted reconstruction",
        ));
    }
    Ok(trusted)
}

/// The frozen V1 DTO. Do not add fields: its denied-field set is part of the
/// released compatibility boundary.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryPlanWire {
    bindings: Vec<QueryBindingWire>,
    format: String,
    functions: Vec<LocalFunctionWire>,
    inputs: Vec<InputColumnWire>,
    managed_semantics: FingerprintWire,
    output: QueryOutputWire,
    pipeline: Vec<ReadStageWire>,
    required_capabilities: CapabilitySet,
}

impl QueryPlanWire {
    fn rebuild(self) -> Result<QueryPlan, Diagnostic> {
        let trusted = QueryPlan::new_with_functions(
            self.bindings
                .into_iter()
                .map(QueryBindingWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.functions
                .into_iter()
                .map(LocalFunctionWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.inputs
                .into_iter()
                .map(InputColumnWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.pipeline
                .into_iter()
                .map(ReadStageWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.output.rebuild()?,
            ManagedSemanticSchemaFingerprint::from_wire(self.managed_semantics.rebuild()?)?,
        )?;
        if self.required_capabilities != *trusted.required_capabilities() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_plan_capability_claim_mismatch",
                "query plan required capabilities are not syntax-derived",
            ));
        }
        Ok(trusted)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryPlanWireV2 {
    bindings: Vec<QueryBindingWire>,
    compatibility: QueryPlanV2Compatibility,
    format: String,
    functions: Vec<LocalFunctionWire>,
    inputs: Vec<InputColumnWire>,
    managed_semantics: FingerprintWire,
    output: QueryOutputWire,
    pipeline: Vec<ReadStageWireV2>,
    required_capabilities: CapabilitySet,
}

impl QueryPlanWireV2 {
    fn rebuild(self) -> Result<QueryPlan, Diagnostic> {
        let trusted = QueryPlan::new_v2_with_functions(
            self.bindings
                .into_iter()
                .map(QueryBindingWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.functions
                .into_iter()
                .map(LocalFunctionWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.inputs
                .into_iter()
                .map(InputColumnWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.pipeline
                .into_iter()
                .map(ReadStageWireV2::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.output.rebuild()?,
            self.compatibility,
            ManagedSemanticSchemaFingerprint::from_wire(self.managed_semantics.rebuild()?)?,
        )?;
        if self.format != QUERY_PLAN_FORMAT_V2 {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_format_unsupported",
                "query plan V2 DTO carries a foreign format discriminator",
            ));
        }
        if self.required_capabilities != *trusted.required_capabilities() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_plan_capability_claim_mismatch",
                "query plan required capabilities are not syntax-derived",
            ));
        }
        Ok(trusted)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryBindingWire {
    id: u16,
    variable: String,
}

impl QueryBindingWire {
    fn rebuild(self) -> Result<AssertionBinding, Diagnostic> {
        Ok(AssertionBinding::new(
            BindingId::new(self.id)?,
            QueryVariable::new(self.variable)?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InputColumnWire {
    id: u16,
    optional: bool,
    public_name: String,
    value_type: ValueTypeTag,
}

impl InputColumnWire {
    fn rebuild(self) -> Result<InputColumn, Diagnostic> {
        Ok(InputColumn::new(
            InputColumnId::new(self.id),
            QueryVariable::new(self.public_name)?,
            self.value_type,
            self.optional,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum QueryOutputWire {
    Rows { columns: Vec<u16> },
    Documents { fields: Vec<DocumentFieldWire> },
}

impl QueryOutputWire {
    fn rebuild(self) -> Result<QueryOutput, Diagnostic> {
        Ok(match self {
            Self::Rows { columns } => QueryOutput::Rows {
                columns: columns
                    .into_iter()
                    .map(BindingId::new)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Documents { fields } => QueryOutput::Documents {
                fields: fields
                    .into_iter()
                    .map(DocumentFieldWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalFunctionWire {
    bindings: Vec<QueryBindingWire>,
    body: Vec<QueryPatternWire>,
    name: String,
    parameters: Vec<String>,
    returns: LocalReturnWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalReturnWire {
    input: u16,
    reducer: ReducerWire,
    value_type: ValueTypeTag,
}

impl LocalFunctionWire {
    fn rebuild(self) -> Result<LocalFunction, Diagnostic> {
        Ok(LocalFunction::new(
            FunctionId::new(self.name)?,
            self.bindings
                .into_iter()
                .map(QueryBindingWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.parameters
                .into_iter()
                .map(Label::new)
                .collect::<Result<Vec<_>, _>>()?,
            self.body
                .into_iter()
                .map(QueryPatternWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            LocalReturn::new(
                self.returns.reducer.rebuild(),
                BindingId::new(self.returns.input)?,
                self.returns.value_type,
            ),
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DocumentFieldWire {
    key: String,
    source: DocumentSourceWire,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DocumentSourceWire {
    Binding { binding: u16 },
    AttributeList { attribute: String, owner: u16 },
}

impl DocumentFieldWire {
    fn rebuild(self) -> Result<DocumentField, Diagnostic> {
        Ok(DocumentField::new(
            QueryVariable::new(self.key)?,
            match self.source {
                DocumentSourceWire::Binding { binding } => DocumentSource::Binding {
                    binding: BindingId::new(binding)?,
                },
                DocumentSourceWire::AttributeList { attribute, owner } => {
                    DocumentSource::AttributeList {
                        attribute: AttributeId::new(attribute)?,
                        owner: BindingId::new(owner)?,
                    }
                }
            },
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReadStageWire {
    Match {
        patterns: Vec<QueryPatternWire>,
    },
    Select {
        bindings: Vec<u16>,
    },
    Require {
        bindings: Vec<u16>,
    },
    Distinct,
    Reduce {
        assignments: Vec<ReduceAssignmentWire>,
        groups: Vec<u16>,
    },
    Sort {
        terms: Vec<OrderTermWire>,
    },
    Offset {
        rows: u64,
    },
    Limit {
        rows: u64,
    },
}

impl ReadStageWire {
    fn rebuild(self) -> Result<ReadStage, Diagnostic> {
        Ok(match self {
            Self::Match { patterns } => ReadStage::Match {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Select { bindings } => ReadStage::Select {
                bindings: rebuild_bindings(bindings)?,
            },
            Self::Require { bindings } => ReadStage::Require {
                bindings: rebuild_bindings(bindings)?,
            },
            Self::Distinct => ReadStage::Distinct,
            Self::Reduce {
                assignments,
                groups,
            } => ReadStage::Reduce {
                assignments: assignments
                    .into_iter()
                    .map(ReduceAssignmentWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                groups: rebuild_bindings(groups)?,
            },
            Self::Sort { terms } => ReadStage::Sort {
                terms: terms
                    .into_iter()
                    .map(OrderTermWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Offset { rows } => ReadStage::Offset { rows },
            Self::Limit { rows } => ReadStage::Limit { rows },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReadStageWireV2 {
    Match {
        patterns: Vec<QueryPatternWireV2>,
    },
    Select {
        bindings: Vec<u16>,
    },
    Require {
        bindings: Vec<u16>,
    },
    Distinct,
    Reduce {
        assignments: Vec<ReduceAssignmentWire>,
        groups: Vec<u16>,
    },
    Sort {
        terms: Vec<OrderTermWire>,
    },
    Offset {
        rows: u64,
    },
    Limit {
        rows: u64,
    },
}

impl ReadStageWireV2 {
    fn rebuild(self) -> Result<ReadStage, Diagnostic> {
        Ok(match self {
            Self::Match { patterns } => ReadStage::Match {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWireV2::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Select { bindings } => ReadStage::Select {
                bindings: rebuild_bindings(bindings)?,
            },
            Self::Require { bindings } => ReadStage::Require {
                bindings: rebuild_bindings(bindings)?,
            },
            Self::Distinct => ReadStage::Distinct,
            Self::Reduce {
                assignments,
                groups,
            } => ReadStage::Reduce {
                assignments: assignments
                    .into_iter()
                    .map(ReduceAssignmentWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                groups: rebuild_bindings(groups)?,
            },
            Self::Sort { terms } => ReadStage::Sort {
                terms: terms
                    .into_iter()
                    .map(OrderTermWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Offset { rows } => ReadStage::Offset { rows },
            Self::Limit { rows } => ReadStage::Limit { rows },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReduceAssignmentWire {
    assigned: u16,
    input: Option<u16>,
    reducer: ReducerWire,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReducerWire {
    Count,
    Max,
    Mean,
    Median,
    Min,
    Std,
    Sum,
}

impl ReducerWire {
    const fn rebuild(self) -> Reducer {
        match self {
            Self::Count => Reducer::Count,
            Self::Max => Reducer::Max,
            Self::Mean => Reducer::Mean,
            Self::Median => Reducer::Median,
            Self::Min => Reducer::Min,
            Self::Std => Reducer::Std,
            Self::Sum => Reducer::Sum,
        }
    }
}

impl ReduceAssignmentWire {
    fn rebuild(self) -> Result<ReduceAssignment, Diagnostic> {
        Ok(ReduceAssignment::new(
            BindingId::new(self.assigned)?,
            self.reducer.rebuild(),
            self.input.map(BindingId::new).transpose()?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrderTermWire {
    binding: u16,
    direction: OrderDirectionWire,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum OrderDirectionWire {
    Ascending,
    Descending,
}

impl OrderTermWire {
    fn rebuild(self) -> Result<OrderTerm, Diagnostic> {
        Ok(OrderTerm::new(
            BindingId::new(self.binding)?,
            match self.direction {
                OrderDirectionWire::Ascending => OrderDirection::Ascending,
                OrderDirectionWire::Descending => OrderDirection::Descending,
            },
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum QueryPatternWire {
    Isa {
        binding: u16,
        include_subtypes: bool,
        type_id: TypeIdWire,
    },
    Has {
        attribute: u16,
        attribute_id: String,
        owner: u16,
    },
    Links {
        players: Vec<AssertionRolePlayerWire>,
        relation: u16,
        relation_id: TypeIdWire,
    },
    Value {
        comparator: ValueComparatorWire,
        left: QueryOperandWire,
        right: QueryOperandWire,
    },
    Not {
        patterns: Vec<QueryPatternWire>,
    },
    Try {
        patterns: Vec<QueryPatternWire>,
    },
    Reachable {
        max_depth: u8,
        relation: TypeIdWire,
        role_from: RoleIdWire,
        role_to: RoleIdWire,
        source: u16,
        target: u16,
    },
    FunctionCall {
        arguments: Vec<QueryOperandWire>,
        assigned: u16,
        function: String,
    },
}

impl QueryPatternWire {
    fn rebuild(self) -> Result<QueryPattern, Diagnostic> {
        Ok(match self {
            Self::Isa {
                binding,
                include_subtypes,
                type_id,
            } => QueryPattern::Isa {
                binding: BindingId::new(binding)?,
                include_subtypes,
                type_id: type_id.rebuild()?,
            },
            Self::Has {
                attribute,
                attribute_id,
                owner,
            } => QueryPattern::Has {
                attribute: BindingId::new(attribute)?,
                attribute_id: AttributeId::new(attribute_id)?,
                owner: BindingId::new(owner)?,
            },
            Self::Links {
                players,
                relation,
                relation_id,
            } => QueryPattern::Links {
                players: players
                    .into_iter()
                    .map(AssertionRolePlayerWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                relation: BindingId::new(relation)?,
                relation_id: relation_id.rebuild()?,
            },
            Self::Value {
                comparator,
                left,
                right,
            } => QueryPattern::Value {
                comparator: comparator.rebuild(),
                left: left.rebuild()?,
                right: right.rebuild()?,
            },
            Self::Not { patterns } => QueryPattern::Not {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Try { patterns } => QueryPattern::Try {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Reachable {
                max_depth,
                relation,
                role_from,
                role_to,
                source,
                target,
            } => QueryPattern::Reachable {
                min_depth: 1,
                max_depth,
                relation: relation.rebuild()?,
                role_from: role_from.rebuild()?,
                role_to: role_to.rebuild()?,
                source: BindingId::new(source)?,
                target: BindingId::new(target)?,
            },
            Self::FunctionCall {
                arguments,
                assigned,
                function,
            } => QueryPattern::FunctionCall {
                arguments: arguments
                    .into_iter()
                    .map(QueryOperandWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                assigned: BindingId::new(assigned)?,
                function: FunctionId::new(function)?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum QueryPatternWireV2 {
    Isa {
        binding: u16,
        include_subtypes: bool,
        type_id: TypeIdWire,
    },
    Has {
        attribute: u16,
        attribute_id: String,
        owner: u16,
    },
    Links {
        players: Vec<AssertionRolePlayerWire>,
        relation: u16,
        relation_id: TypeIdWire,
    },
    Value {
        comparator: ValueComparatorWire,
        left: QueryOperandWire,
        right: QueryOperandWire,
    },
    Or {
        branches: Vec<Vec<QueryPatternWireV2>>,
    },
    Not {
        patterns: Vec<QueryPatternWireV2>,
    },
    Try {
        patterns: Vec<QueryPatternWireV2>,
    },
    Reachable {
        min_depth: u8,
        max_depth: u8,
        relation: TypeIdWire,
        role_from: RoleIdWire,
        role_to: RoleIdWire,
        source: u16,
        target: u16,
    },
    FunctionCall {
        arguments: Vec<QueryOperandWire>,
        assigned: u16,
        function: String,
    },
}

impl QueryPatternWireV2 {
    fn rebuild(self) -> Result<QueryPattern, Diagnostic> {
        Ok(match self {
            Self::Isa {
                binding,
                include_subtypes,
                type_id,
            } => QueryPattern::Isa {
                binding: BindingId::new(binding)?,
                include_subtypes,
                type_id: type_id.rebuild()?,
            },
            Self::Has {
                attribute,
                attribute_id,
                owner,
            } => QueryPattern::Has {
                attribute: BindingId::new(attribute)?,
                attribute_id: AttributeId::new(attribute_id)?,
                owner: BindingId::new(owner)?,
            },
            Self::Links {
                players,
                relation,
                relation_id,
            } => QueryPattern::Links {
                players: players
                    .into_iter()
                    .map(AssertionRolePlayerWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                relation: BindingId::new(relation)?,
                relation_id: relation_id.rebuild()?,
            },
            Self::Value {
                comparator,
                left,
                right,
            } => QueryPattern::Value {
                comparator: comparator.rebuild(),
                left: left.rebuild()?,
                right: right.rebuild()?,
            },
            Self::Or { branches } => QueryPattern::Or {
                branches: branches
                    .into_iter()
                    .map(|branch| {
                        branch
                            .into_iter()
                            .map(QueryPatternWireV2::rebuild)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Not { patterns } => QueryPattern::Not {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWireV2::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Try { patterns } => QueryPattern::Try {
                patterns: patterns
                    .into_iter()
                    .map(QueryPatternWireV2::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Reachable {
                min_depth,
                max_depth,
                relation,
                role_from,
                role_to,
                source,
                target,
            } => QueryPattern::Reachable {
                min_depth,
                max_depth,
                relation: relation.rebuild()?,
                role_from: role_from.rebuild()?,
                role_to: role_to.rebuild()?,
                source: BindingId::new(source)?,
                target: BindingId::new(target)?,
            },
            Self::FunctionCall {
                arguments,
                assigned,
                function,
            } => QueryPattern::FunctionCall {
                arguments: arguments
                    .into_iter()
                    .map(QueryOperandWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                assigned: BindingId::new(assigned)?,
                function: FunctionId::new(function)?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum QueryOperandWire {
    Binding { binding: u16 },
    Literal { value: CanonicalValue },
    Input { column: u16 },
}

impl QueryOperandWire {
    fn rebuild(self) -> Result<QueryOperand, Diagnostic> {
        Ok(match self {
            Self::Binding { binding } => QueryOperand::Binding {
                binding: BindingId::new(binding)?,
            },
            Self::Literal { value } => QueryOperand::Literal { value },
            Self::Input { column } => QueryOperand::Input {
                column: InputColumnId::new(column),
            },
        })
    }
}

fn rebuild_bindings(values: Vec<u16>) -> Result<Vec<BindingId>, Diagnostic> {
    values.into_iter().map(BindingId::new).collect()
}
