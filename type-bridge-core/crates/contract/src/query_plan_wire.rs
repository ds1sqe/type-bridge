//! Private fail-closed wire DTOs for reusable query plans.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::id::{AttributeId, FunctionId, Label};
use crate::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use crate::migration_assertion_wire::{
    AssertionRolePlayerWire, RoleIdWire, TypeIdWire, ValueComparatorWire,
};
use crate::query_plan::{
    DocumentField, DocumentSource, InputColumn, InputColumnId, LocalFunction, LocalReturn,
    OrderDirection, OrderTerm, QueryOperand, QueryOutput, QueryPattern, ReadStage,
    ReduceAssignment, Reducer, failure,
};
use crate::value::{CanonicalValue, ValueTypeTag};

// Decode scalar values under the native plan's configured limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct PlanScalarWire(Value);

pub(crate) trait WireScalar {
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<CanonicalValue, Diagnostic>;
}
impl WireScalar for CanonicalValue {
    fn rebuild(self, _limits: crate::limits::CodecLimits) -> Result<CanonicalValue, Diagnostic> {
        Ok(self)
    }
}
impl WireScalar for PlanScalarWire {
    fn rebuild(self, limits: crate::limits::CodecLimits) -> Result<CanonicalValue, Diagnostic> {
        CanonicalValue::from_json_with_limits(self.0, limits)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryBindingWire {
    id: u16,
    variable: String,
}

impl QueryBindingWire {
    pub(crate) fn rebuild(self) -> Result<AssertionBinding, Diagnostic> {
        Ok(AssertionBinding::new(
            BindingId::new(self.id)?,
            QueryVariable::new(self.variable)?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputColumnWire {
    id: u16,
    optional: bool,
    public_name: String,
    value_type: ValueTypeTag,
}

impl InputColumnWire {
    pub(crate) fn rebuild(self) -> Result<InputColumn, Diagnostic> {
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
pub(crate) struct LocalFunctionWire<S = CanonicalValue> {
    bindings: Vec<QueryBindingWire>,
    body: Vec<QueryPatternWire<S>>,
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

impl<S: WireScalar> LocalFunctionWire<S> {
    pub(crate) fn rebuild_with_limits(
        self,
        limits: crate::limits::CodecLimits,
    ) -> Result<LocalFunction, Diagnostic> {
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
                .map(|value| value.rebuild_with_limits(limits))
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
        patterns: Vec<ReadPatternWire>,
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
                    .map(ReadPatternWire::rebuild)
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
enum QueryPatternWire<S = CanonicalValue> {
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
    HasType {
        attribute: u16,
        attribute_id: String,
        owner: u16,
        mode: Option<String>,
    },
    Links {
        players: Vec<AssertionRolePlayerWire>,
        relation: u16,
        relation_id: TypeIdWire,
    },
    Value {
        comparator: ValueComparatorWire,
        left: QueryOperandWire<S>,
        right: QueryOperandWire<S>,
    },
    Not {
        patterns: Vec<QueryPatternWire<S>>,
    },
    Try {
        patterns: Vec<QueryPatternWire<S>>,
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
        arguments: Vec<QueryOperandWire<S>>,
        assigned: u16,
        function: String,
    },
}

impl<S: WireScalar> QueryPatternWire<S> {
    fn rebuild_with_limits(
        self,
        limits: crate::limits::CodecLimits,
    ) -> Result<QueryPattern, Diagnostic> {
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
            Self::HasType {
                attribute,
                attribute_id,
                owner,
                mode,
            } => QueryPattern::HasType {
                attribute: BindingId::new(attribute)?,
                attribute_id: AttributeId::new(attribute_id)?,
                owner: BindingId::new(owner)?,
                mode: crate::query_plan::TypeMatchMode::from_typeql(mode.as_deref().unwrap_or(""))?,
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
                left: left.rebuild_with_limits(limits)?,
                right: right.rebuild_with_limits(limits)?,
            },
            Self::Not { patterns } => QueryPattern::Not {
                patterns: patterns
                    .into_iter()
                    .map(|value| value.rebuild_with_limits(limits))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Try { patterns } => QueryPattern::Try {
                patterns: patterns
                    .into_iter()
                    .map(|value| value.rebuild_with_limits(limits))
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
                    .map(|value| value.rebuild_with_limits(limits))
                    .collect::<Result<Vec<_>, _>>()?,
                assigned: BindingId::new(assigned)?,
                function: FunctionId::new(function)?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReadPatternWire<S = CanonicalValue> {
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
    HasType {
        attribute: u16,
        attribute_id: String,
        owner: u16,
        mode: Option<String>,
    },
    Links {
        players: Vec<AssertionRolePlayerWire>,
        relation: u16,
        relation_id: TypeIdWire,
    },
    Value {
        comparator: ValueComparatorWire,
        left: QueryOperandWire<S>,
        right: QueryOperandWire<S>,
    },
    Or {
        branches: Vec<Vec<ReadPatternWire<S>>>,
    },
    Not {
        patterns: Vec<ReadPatternWire<S>>,
    },
    Try {
        patterns: Vec<ReadPatternWire<S>>,
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
        arguments: Vec<QueryOperandWire<S>>,
        assigned: u16,
        function: String,
    },
}

impl<S: WireScalar> ReadPatternWire<S> {
    fn rebuild(self) -> Result<QueryPattern, Diagnostic> {
        self.rebuild_with_limits(crate::limits::CodecLimits::CANONICAL)
    }

    fn rebuild_with_limits(
        self,
        limits: crate::limits::CodecLimits,
    ) -> Result<QueryPattern, Diagnostic> {
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
            Self::HasType {
                attribute,
                attribute_id,
                owner,
                mode,
            } => QueryPattern::HasType {
                attribute: BindingId::new(attribute)?,
                attribute_id: AttributeId::new(attribute_id)?,
                owner: BindingId::new(owner)?,
                mode: crate::query_plan::TypeMatchMode::from_typeql(mode.as_deref().unwrap_or(""))?,
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
                left: left.rebuild_with_limits(limits)?,
                right: right.rebuild_with_limits(limits)?,
            },
            Self::Or { branches } => QueryPattern::Or {
                branches: branches
                    .into_iter()
                    .map(|branch| {
                        branch
                            .into_iter()
                            .map(|value| value.rebuild_with_limits(limits))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Not { patterns } => QueryPattern::Not {
                patterns: patterns
                    .into_iter()
                    .map(|value| value.rebuild_with_limits(limits))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Try { patterns } => QueryPattern::Try {
                patterns: patterns
                    .into_iter()
                    .map(|value| value.rebuild_with_limits(limits))
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
                    .map(|value| value.rebuild_with_limits(limits))
                    .collect::<Result<Vec<_>, _>>()?,
                assigned: BindingId::new(assigned)?,
                function: FunctionId::new(function)?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum QueryOperandWire<S = CanonicalValue> {
    Binding { binding: u16 },
    Literal { value: S },
    Input { column: u16 },
}

impl<S: WireScalar> QueryOperandWire<S> {
    pub(crate) fn rebuild_with_limits(
        self,
        limits: crate::limits::CodecLimits,
    ) -> Result<QueryOperand, Diagnostic> {
        Ok(match self {
            Self::Binding { binding } => QueryOperand::Binding {
                binding: BindingId::new(binding)?,
            },
            Self::Literal { value } => QueryOperand::Literal {
                value: value.rebuild(limits)?,
            },
            Self::Input { column } => QueryOperand::Input {
                column: InputColumnId::new(column),
            },
        })
    }
}

fn rebuild_bindings(values: Vec<u16>) -> Result<Vec<BindingId>, Diagnostic> {
    values.into_iter().map(BindingId::new).collect()
}

// V3 shares the released closed read vocabulary without exposing its DTOs.
pub(crate) fn rebuild_read_stage(value: Value) -> Result<ReadStage, Diagnostic> {
    serde_json::from_value::<ReadStageWire>(value)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "invalid_canonical_value",
                "invalid read stage",
            )
        })?
        .rebuild()
}
pub(crate) fn rebuild_output(value: Value) -> Result<QueryOutput, Diagnostic> {
    serde_json::from_value::<QueryOutputWire>(value)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "invalid_canonical_value",
                "invalid output projection",
            )
        })?
        .rebuild()
}

pub(crate) fn rebuild_pattern(
    value: Value,
    limits: crate::limits::CodecLimits,
) -> Result<QueryPattern, Diagnostic> {
    match value.get("kind").and_then(Value::as_str) {
        Some("iid") => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct IidWire {
                kind: String,
                binding: u16,
                iid: QueryOperandWire<PlanScalarWire>,
            }
            let wire: IidWire = serde_json::from_value(value).map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "invalid_canonical_value",
                    "invalid nested IID pattern",
                )
            })?;
            debug_assert_eq!(wire.kind, "iid");
            return Ok(QueryPattern::Iid {
                binding: BindingId::new(wire.binding)?,
                iid: wire.iid.rebuild_with_limits(limits)?,
            });
        }
        Some("or") => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct OrWire {
                kind: String,
                branches: Vec<Vec<Value>>,
            }
            let wire: OrWire = serde_json::from_value(value).map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "invalid_canonical_value",
                    "invalid disjunction pattern",
                )
            })?;
            debug_assert_eq!(wire.kind, "or");
            return Ok(QueryPattern::Or {
                branches: wire
                    .branches
                    .into_iter()
                    .map(|branch| {
                        branch
                            .into_iter()
                            .map(|value| rebuild_pattern(value, limits))
                            .collect()
                    })
                    .collect::<Result<_, Diagnostic>>()?,
            });
        }
        Some("not" | "try") => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ScopeWire {
                kind: String,
                patterns: Vec<Value>,
            }
            let wire: ScopeWire = serde_json::from_value(value).map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "invalid_canonical_value",
                    "invalid scoped pattern",
                )
            })?;
            let patterns = wire
                .patterns
                .into_iter()
                .map(|value| rebuild_pattern(value, limits))
                .collect::<Result<_, _>>()?;
            return Ok(if wire.kind == "not" {
                QueryPattern::Not { patterns }
            } else {
                QueryPattern::Try { patterns }
            });
        }
        _ => {}
    }
    serde_json::from_value::<ReadPatternWire<PlanScalarWire>>(value)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "invalid_canonical_value",
                "invalid V3 read pattern",
            )
        })?
        .rebuild_with_limits(limits)
}
