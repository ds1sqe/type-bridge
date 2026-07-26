//! Shared incremental authoring state machine for canonical V2 query plans.
//!
//! Binding-facing wrappers own only opaque instances of the handles in this
//! module. All authored syntax, scope assignment, dense-ID allocation,
//! canonical construction, schema-aware validation, serialization,
//! fingerprinting, and invocation binding stay in Rust.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticDetailValue,
};
use type_bridge_contract::id::{AttributeId, FunctionId, Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::{
    MAX_BOOLEAN_TERMS, MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES, StructuralLimits,
};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    DocumentField, DocumentSource, InputColumn, InputColumnId, InputRow, LocalFunction,
    LocalReturn, OrderDirection, OrderTerm, QueryInvocation, QueryOperand, QueryOperation,
    QueryOutput, QueryPattern, QueryPlan, QueryPlanFingerprint, QueryPlanV2Compatibility,
    ReadStage, ReduceAssignment, Reducer,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_query::{
    MigrationAssertionValidationContext, ValidatedQuery, validate_query_local_function,
    validate_query_plan,
};

use crate::query_v2_prepared::QueryAuthority;

/// Frozen low-level operation inventory shared by native wrapper parity gates.
pub const QUERY_PLAN_BUILDER_OPERATIONS: &[&str] = &[
    "binding",
    "input",
    "binding_operand",
    "literal_operand",
    "input_operand",
    "isa",
    "has",
    "links",
    "value",
    "not",
    "or",
    "try",
    "reachable",
    "function_call",
    "order",
    "reduce_assignment",
    "local_return",
    "local_function",
    "match",
    "select",
    "require",
    "distinct",
    "reduce",
    "sort",
    "offset",
    "limit",
    "document_binding",
    "document_attribute_list",
    "finalize_rows",
    "finalize_documents",
];

/// Primitive host value accepted by the binding-neutral scalar converter.
///
/// Python and Node inspect only the host primitive kind, then delegate all
/// canonical-domain parsing and validation to this shared conversion.
#[doc(hidden)]
pub enum QueryBuilderScalarInput {
    /// A host string, including canonical temporal/decimal/duration text.
    Text(String),
    /// An exact signed 64-bit integer.
    Long(i64),
    /// A host binary64 number.
    Double(f64),
    /// A host boolean.
    Boolean(bool),
}

macro_rules! define_closed_builder_vocabulary {
    (
        $manifest:ident,
        $name_function:ident,
        $variant_type:ty,
        { $($variant:path => $spelling:literal),+ $(,)? }
    ) => {
        #[doc = "Closed public spellings generated from the exhaustive enum mapping."]
        #[doc(hidden)]
        pub const $manifest: &[($variant_type, &str)] = &[
            $(($variant, $spelling)),+
        ];

        #[doc = "Return the source-owned spelling for one closed enum variant."]
        #[doc(hidden)]
        pub const fn $name_function(value: $variant_type) -> &'static str {
            match value {
                $($variant => $spelling),+
            }
        }
    };
}

define_closed_builder_vocabulary!(
    QUERY_BUILDER_VALUE_TYPES,
    query_builder_value_type_name,
    ValueTypeTag,
    {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "long",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime_tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
);

macro_rules! define_queryable_type_kinds {
    (
        accepted { $($accepted:path => $spelling:literal),+ $(,)? }
        rejected { $($rejected:path),+ $(,)? }
    ) => {
        /// Closed schema-kind subset accepted by `isa` authoring.
        #[doc(hidden)]
        pub const QUERY_BUILDER_TYPE_KINDS: &[(TypeKind, &str)] = &[
            $(($accepted, $spelling)),+
        ];

        /// Return the source-owned `isa` spelling, or `None` for non-queryable kinds.
        #[doc(hidden)]
        pub const fn query_builder_type_kind_name(kind: TypeKind) -> Option<&'static str> {
            match kind {
                $($accepted => Some($spelling)),+,
                $($rejected => None),+
            }
        }
    };
}

define_queryable_type_kinds!(
    accepted {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute => "attribute",
    }
    rejected {
        TypeKind::Struct,
    }
);

define_closed_builder_vocabulary!(
    QUERY_BUILDER_COMPARATORS,
    query_builder_comparator_name,
    ValueComparator,
    {
        ValueComparator::Equal => "equal",
        ValueComparator::NotEqual => "not_equal",
        ValueComparator::Less => "less",
        ValueComparator::LessOrEqual => "less_or_equal",
        ValueComparator::Greater => "greater",
        ValueComparator::GreaterOrEqual => "greater_or_equal",
    }
);

define_closed_builder_vocabulary!(
    QUERY_BUILDER_ORDER_DIRECTIONS,
    query_builder_order_direction_name,
    OrderDirection,
    {
        OrderDirection::Ascending => "ascending",
        OrderDirection::Descending => "descending",
    }
);

define_closed_builder_vocabulary!(
    QUERY_BUILDER_REDUCERS,
    query_builder_reducer_name,
    Reducer,
    {
        Reducer::Count => "count",
        Reducer::Max => "max",
        Reducer::Mean => "mean",
        Reducer::Min => "min",
        Reducer::Sum => "sum",
    }
);

/// Closed reducer/result pairs admitted by plan-local functions.
#[doc(hidden)]
pub const QUERY_BUILDER_LOCAL_RETURNS: &[(Reducer, ValueTypeTag)] = &[
    (Reducer::Count, ValueTypeTag::Long),
    (Reducer::Sum, ValueTypeTag::Long),
    (Reducer::Sum, ValueTypeTag::Double),
];

/// Parse the exact low-level scalar-domain spelling shared by both bindings.
#[doc(hidden)]
pub fn query_builder_value_type(value: &str) -> Result<ValueTypeTag, Diagnostic> {
    QUERY_BUILDER_VALUE_TYPES
        .iter()
        .find_map(|(value_type, spelling)| (*spelling == value).then_some(*value_type))
        .ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_value_type_unknown",
                "value type must use one exact canonical scalar-domain spelling",
            )
        })
}

/// Parse the exact low-level schema-kind spelling shared by both bindings.
#[doc(hidden)]
pub fn query_builder_type_kind(value: &str) -> Result<TypeKind, Diagnostic> {
    QUERY_BUILDER_TYPE_KINDS
        .iter()
        .find_map(|(kind, spelling)| (*spelling == value).then_some(*kind))
        .ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_type_kind_unknown",
                "type kind must be entity, relation, or attribute",
            )
        })
}

/// Parse the exact low-level comparator spelling shared by both bindings.
#[doc(hidden)]
pub fn query_builder_comparator(value: &str) -> Result<ValueComparator, Diagnostic> {
    QUERY_BUILDER_COMPARATORS
        .iter()
        .find_map(|(comparator, spelling)| (*spelling == value).then_some(*comparator))
        .ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_comparator_unknown",
                "comparator must use one exact canonical comparator spelling",
            )
        })
}

/// Parse the exact low-level sort-direction spelling shared by both bindings.
#[doc(hidden)]
pub fn query_builder_order_direction(value: &str) -> Result<OrderDirection, Diagnostic> {
    QUERY_BUILDER_ORDER_DIRECTIONS
        .iter()
        .find_map(|(direction, spelling)| (*spelling == value).then_some(*direction))
        .ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_order_direction_unknown",
                "order direction must be ascending or descending",
            )
        })
}

/// Parse the exact low-level reducer spelling shared by both bindings.
#[doc(hidden)]
pub fn query_builder_reducer(value: &str) -> Result<Reducer, Diagnostic> {
    QUERY_BUILDER_REDUCERS
        .iter()
        .find_map(|(reducer, spelling)| (*spelling == value).then_some(*reducer))
        .ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_reducer_unknown",
                "reducer must use one exact canonical reducer spelling",
            )
        })
}

/// Convert one checked host primitive into its exact canonical scalar.
#[doc(hidden)]
pub fn query_builder_scalar(
    value_type: ValueTypeTag,
    value: QueryBuilderScalarInput,
) -> Result<CanonicalValue, Diagnostic> {
    match (value_type, value) {
        (ValueTypeTag::String, QueryBuilderScalarInput::Text(value)) => {
            CanonicalString::new(value).map(CanonicalValue::String)
        }
        (ValueTypeTag::Long, QueryBuilderScalarInput::Long(value)) => {
            Ok(CanonicalValue::Long(value))
        }
        (ValueTypeTag::Double, QueryBuilderScalarInput::Double(value)) => {
            CanonicalDouble::new(value).map(CanonicalValue::Double)
        }
        (ValueTypeTag::Boolean, QueryBuilderScalarInput::Boolean(value)) => {
            Ok(CanonicalValue::Boolean(value))
        }
        (ValueTypeTag::Date, QueryBuilderScalarInput::Text(value)) => {
            value.parse::<CanonicalDate>().map(CanonicalValue::Date)
        }
        (ValueTypeTag::DateTime, QueryBuilderScalarInput::Text(value)) => value
            .parse::<CanonicalDateTime>()
            .map(CanonicalValue::DateTime),
        (ValueTypeTag::DateTimeTz, QueryBuilderScalarInput::Text(value)) => value
            .parse::<CanonicalDateTimeTz>()
            .map(CanonicalValue::DateTimeTz),
        (ValueTypeTag::Decimal, QueryBuilderScalarInput::Text(value)) => {
            DecimalValue::new(value).map(CanonicalValue::Decimal)
        }
        (ValueTypeTag::Duration, QueryBuilderScalarInput::Text(value)) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration),
        _ => Err(builder_failure(
            DiagnosticCategory::InvalidContract,
            "query_builder_scalar_host_type",
            "host scalar kind does not match the declared canonical value type",
        )),
    }
}

/// Return the stable diagnostic for a host primitive of the wrong kind.
#[doc(hidden)]
#[must_use]
pub fn query_builder_scalar_host_type_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_host_type",
        "host scalar kind does not match the declared canonical value type",
    )
}

/// Return the stable diagnostic for a semantic flag that is not an exact host
/// boolean.
#[doc(hidden)]
#[must_use]
pub fn query_builder_boolean_host_type_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_boolean_host_type",
        "query authoring flags require an exact host boolean",
    )
}

/// Return the stable diagnostic for an integer outside signed 64-bit range.
#[doc(hidden)]
#[must_use]
pub fn query_builder_scalar_integer_range_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_integer_range",
        "long scalar must fit the exact signed 64-bit domain",
    )
}

/// Return the stable diagnostic for host text that cannot be represented as
/// one unchanged Unicode scalar sequence.
#[doc(hidden)]
#[must_use]
pub fn query_builder_scalar_unicode_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_unicode",
        "string scalar must be valid Unicode without surrogate code points",
    )
}

/// Return the stable diagnostic for a scalar or otherwise unsupported host
/// value supplied where query authoring requires a collection.
#[doc(hidden)]
#[must_use]
pub fn query_builder_host_collection_type_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_host_collection_type",
        "host query authoring collection has an unsupported container type",
    )
}

/// Return the canonical invocation diagnostic for a non-rectangular row.
#[doc(hidden)]
#[must_use]
pub fn query_builder_invocation_row_arity_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_invocation_row_arity",
        "input row does not carry exactly the declared column set",
    )
}

/// Return the canonical relation role-player term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_role_player_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_role_player_limit",
        "links pattern has no players or exceeds the term ceiling",
    )
}

/// Return the canonical negation term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_negation_term_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_negation_term_limit",
        "negation is empty or exceeds the boolean-term ceiling",
    )
}

/// Return the canonical disjunction term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_disjunction_term_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_disjunction_term_limit",
        "disjunction branches are empty or exceed the boolean-term ceiling",
    )
}

/// Return the canonical optional-block term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_try_term_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_try_term_limit",
        "optional block is empty or exceeds the boolean-term ceiling",
    )
}

/// Return the canonical function-argument term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_function_argument_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_function_argument_limit",
        "function call arguments exceed the term ceiling",
    )
}

/// Return the canonical plan binding-count ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_binding_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_binding_limit",
        "plan binding count exceeds the structural ceiling",
    )
}

/// Return the canonical root-conjunction term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_root_pattern_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_pattern_limit",
        "plan root conjunction is empty or exceeds the term ceiling",
    )
}

/// Return the canonical local-function body term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_local_body_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_pattern_limit",
        "local function body is empty or exceeds the term ceiling",
    )
}

/// Return the canonical local binding-count ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_local_binding_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_binding_limit",
        "local binding count is empty or exceeds the structural ceiling",
    )
}

/// Return the canonical reduce-assignment term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_reduce_term_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_reduce_term_limit",
        "reduce has no assignments or exceeds the term ceiling",
    )
}

/// Return the canonical sort-term ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_sort_term_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_sort_term_limit",
        "sort has no terms or exceeds the term ceiling",
    )
}

/// Return the canonical row-output slot ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_row_output_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_output_limit",
        "output column count is empty or exceeds the structural ceiling",
    )
}

/// Return the canonical document-output slot ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_document_output_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_plan_output_limit",
        "output field count is empty or exceeds the structural ceiling",
    )
}

/// Return the stable invocation row-count ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_invocation_row_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_invocation_row_limit",
        "invocation input row count exceeds the structural ceiling",
    )
}

/// Return the stable invocation aggregate input-byte ceiling diagnostic.
#[doc(hidden)]
#[must_use]
pub fn query_builder_invocation_input_byte_limit_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::ResourceLimit,
        "query_invocation_input_byte_limit",
        "invocation input rows exceed the structural byte ceiling",
    )
}

/// Convert an exact host integer into a bounded reachability depth.
#[doc(hidden)]
pub fn query_builder_depth(value: i128) -> Result<u8, Diagnostic> {
    u8::try_from(value).map_err(|_| query_builder_depth_error())
}

/// Return the stable diagnostic for a non-integer or out-of-range depth.
#[doc(hidden)]
#[must_use]
pub fn query_builder_depth_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_depth_range",
        "reachability depth must be an exact integer between 0 and 255",
    )
}

/// Convert an exact host integer into an unsigned row-window value.
#[doc(hidden)]
pub fn query_builder_unsigned(value: i128) -> Result<u64, Diagnostic> {
    u64::try_from(value).map_err(|_| query_builder_unsigned_error())
}

/// Return the stable diagnostic for a non-integer or out-of-range row window.
#[doc(hidden)]
#[must_use]
pub fn query_builder_unsigned_error() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_unsigned_integer_range",
        "row offset or limit must be an exact unsigned 64-bit integer",
    )
}

/// Opaque identity of one exact [`QueryAuthority`].
///
/// Equality is allocation identity, not schema-value equality: separately
/// constructed authorities remain distinct even when their canonical schema
/// bytes are equal.
#[derive(Clone)]
pub struct QueryAuthorityIdentity(Arc<()>);

impl QueryAuthorityIdentity {
    pub(crate) fn fresh() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for QueryAuthorityIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for QueryAuthorityIdentity {}

impl fmt::Debug for QueryAuthorityIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QueryAuthorityIdentity([OPAQUE])")
    }
}

#[derive(Clone)]
struct BuilderIdentity(Arc<()>);

impl BuilderIdentity {
    fn fresh() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for BuilderIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for BuilderIdentity {}

#[derive(Clone)]
struct HandleOwner {
    authority: QueryAuthorityIdentity,
    builder: BuilderIdentity,
}

impl HandleOwner {
    fn new(authority: QueryAuthorityIdentity) -> Self {
        Self {
            authority,
            builder: BuilderIdentity::fresh(),
        }
    }
}

impl fmt::Debug for HandleOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandleOwner([OPAQUE])")
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AuthoredBindingId(usize);

/// Opaque builder-owned binding declaration.
#[derive(Clone, Debug)]
pub struct QueryBindingHandle {
    id: AuthoredBindingId,
    owner: HandleOwner,
}

/// Opaque builder-owned input-column declaration.
#[derive(Clone, Debug)]
pub struct QueryInputHandle {
    id: InputColumnId,
    owner: HandleOwner,
}

#[derive(Clone, Debug)]
enum AuthoredOperand {
    Binding(AuthoredBindingId),
    Literal(CanonicalValue),
    Input(InputColumnId),
}

/// Opaque builder-owned typed value operand.
#[derive(Clone, Debug)]
pub struct QueryOperandHandle {
    operand: AuthoredOperand,
    owner: HandleOwner,
}

#[derive(Clone, Debug)]
enum AuthoredPattern {
    Isa {
        binding: AuthoredBindingId,
        include_subtypes: bool,
        type_id: TypeId,
    },
    Has {
        attribute: AuthoredBindingId,
        attribute_id: AttributeId,
        owner: AuthoredBindingId,
    },
    Links {
        players: Vec<(RoleId, AuthoredBindingId)>,
        relation: AuthoredBindingId,
        relation_id: TypeId,
    },
    Value {
        comparator: ValueComparator,
        left: AuthoredOperand,
        right: AuthoredOperand,
    },
    Or {
        branches: Vec<Vec<Arc<AuthoredPattern>>>,
    },
    Not {
        patterns: Vec<Arc<AuthoredPattern>>,
    },
    Try {
        patterns: Vec<Arc<AuthoredPattern>>,
    },
    Reachable {
        min_depth: u8,
        max_depth: u8,
        relation: TypeId,
        role_from: RoleId,
        role_to: RoleId,
        source: AuthoredBindingId,
        target: AuthoredBindingId,
    },
    FunctionCall {
        arguments: Vec<AuthoredOperand>,
        assigned: AuthoredBindingId,
        function: FunctionId,
    },
}

/// Opaque builder-owned query pattern.
#[derive(Clone)]
pub struct QueryPatternHandle {
    metrics: PatternMetrics,
    owner: HandleOwner,
    pattern: Arc<AuthoredPattern>,
}

impl fmt::Debug for QueryPatternHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueryPatternHandle")
            .field("owner", &self.owner)
            .field("depth", &self.metrics.depth)
            .field("nodes", &self.metrics.nodes)
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
struct PatternMetrics {
    depth: usize,
    nodes: usize,
    root_only: Option<RootOnlyPattern>,
}

impl PatternMetrics {
    const LEAF: Self = Self {
        depth: 1,
        nodes: 1,
        root_only: None,
    };
    const FUNCTION: Self = Self {
        depth: 1,
        nodes: 1,
        root_only: Some(RootOnlyPattern::FunctionCall),
    };

    fn composite(children: impl IntoIterator<Item = Self>) -> Result<Self, Diagnostic> {
        let mut depth = 1usize;
        let mut nodes = 1usize;
        for child in children {
            depth = depth.max(child.depth.saturating_add(1));
            nodes = nodes.saturating_add(child.nodes);
        }
        let metrics = Self {
            depth,
            nodes,
            root_only: None,
        };
        metrics.ensure_bounded()?;
        Ok(metrics)
    }

    fn ensure_bounded(self) -> Result<(), Diagnostic> {
        if self.depth > MAX_PREDICATE_DEPTH {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_pattern_depth_limit",
                "plan pattern depth exceeds the structural ceiling",
            ));
        }
        if self.nodes > MAX_PREDICATE_NODES {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_pattern_node_limit",
                "plan pattern count exceeds the structural ceiling",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum RootOnlyPattern {
    Try,
    Reachable,
    FunctionCall,
}

impl RootOnlyPattern {
    fn nested_failure(self) -> Diagnostic {
        match self {
            Self::Try => builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_try_not_root",
                "optional blocks are admitted only in the root conjunction",
            ),
            Self::Reachable => builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_reachable_not_root",
                "bounded reachability is admitted only in the root conjunction",
            ),
            Self::FunctionCall => builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_function_in_negation",
                "function calls are admitted only in the root conjunction",
            ),
        }
    }
}

/// Opaque builder-owned sort term.
#[derive(Clone, Debug)]
pub struct QueryOrderHandle {
    binding: AuthoredBindingId,
    direction: OrderDirection,
    owner: HandleOwner,
}

/// Opaque builder-owned reducer assignment.
#[derive(Clone, Debug)]
pub struct QueryReduceAssignmentHandle {
    assigned: AuthoredBindingId,
    input: Option<AuthoredBindingId>,
    owner: HandleOwner,
    reducer: Reducer,
}

/// Opaque builder-owned local-function return.
#[derive(Clone, Debug)]
pub struct QueryLocalReturnHandle {
    input: AuthoredBindingId,
    owner: HandleOwner,
    reducer: Reducer,
    value_type: ValueTypeTag,
}

/// Opaque builder-owned plan-local function.
#[derive(Clone, Debug)]
pub struct QueryLocalFunctionHandle {
    function: FunctionId,
    owner: HandleOwner,
}

/// One function target admitted by [`QueryPlanBuilder::function_call`].
pub enum QueryFunctionTarget<'a> {
    /// An exact function from the authority's resolved schema.
    Schema(FunctionId),
    /// A plan-local function authored by this builder.
    Local(&'a QueryLocalFunctionHandle),
}

/// Build the closed schema-or-local function target from binding arguments.
#[doc(hidden)]
pub fn query_builder_function_target(
    schema: Option<FunctionId>,
    local: Option<&QueryLocalFunctionHandle>,
) -> Result<QueryFunctionTarget<'_>, Diagnostic> {
    match (schema, local) {
        (Some(function), None) => Ok(QueryFunctionTarget::Schema(function)),
        (None, Some(function)) => Ok(QueryFunctionTarget::Local(function)),
        _ => Err(builder_failure(
            DiagnosticCategory::InvalidContract,
            "query_builder_function_target",
            "exactly one schema function name or local-function handle is required",
        )),
    }
}

/// Qualify parallel role labels and player handles under one relation.
#[doc(hidden)]
pub fn query_builder_role_players(
    declaring_relation: &str,
    roles: Vec<String>,
    players: Vec<QueryBindingHandle>,
) -> Result<Vec<(RoleId, QueryBindingHandle)>, Diagnostic> {
    if roles.len() != players.len() {
        return Err(builder_failure(
            DiagnosticCategory::InvalidContract,
            "query_builder_role_player_arity",
            "role labels and player handles must have equal length",
        ));
    }
    roles
        .into_iter()
        .zip(players)
        .map(|(role, player)| Ok((RoleId::new(declaring_relation, role)?, player)))
        .collect()
}

/// Validate and zip parallel local parameter handles and schema labels.
#[doc(hidden)]
pub fn query_builder_local_parameters(
    bindings: Vec<QueryBindingHandle>,
    labels: Vec<String>,
) -> Result<Vec<(QueryBindingHandle, Label)>, Diagnostic> {
    if bindings.len() != labels.len() {
        return Err(builder_failure(
            DiagnosticCategory::InvalidContract,
            "query_builder_local_parameter_arity",
            "local parameter handles and schema labels must have equal length",
        ));
    }
    bindings
        .into_iter()
        .zip(labels)
        .map(|(binding, label)| Ok((binding, Label::new(label)?)))
        .collect()
}

#[derive(Clone, Debug)]
enum AuthoredDocumentSource {
    Binding(AuthoredBindingId),
    AttributeList {
        attribute: AttributeId,
        owner: AuthoredBindingId,
    },
}

/// Opaque builder-owned document field.
#[derive(Clone, Debug)]
pub struct QueryDocumentFieldHandle {
    key: QueryVariable,
    owner: HandleOwner,
    source: AuthoredDocumentSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BindingScope {
    Root,
    Local(usize),
}

#[derive(Clone, Debug)]
struct BindingDeclaration {
    scope: Option<BindingScope>,
    variable: QueryVariable,
}

#[derive(Clone, Debug)]
struct RootRowEnvironment {
    mandatory: BTreeSet<AuthoredBindingId>,
    optional: BTreeSet<AuthoredBindingId>,
    pattern_bound: BTreeSet<AuthoredBindingId>,
    has_sort: bool,
}

impl RootRowEnvironment {
    fn from_patterns(patterns: &[Arc<AuthoredPattern>]) -> Result<Self, Diagnostic> {
        let mut mandatory = BTreeSet::new();
        let mut scoped_positive = BTreeSet::new();
        for pattern in patterns {
            pattern.collect_direct_positive_bindings(&mut mandatory);
            pattern.collect_negation_positive_bindings(&mut scoped_positive);
        }

        let mut optional = BTreeSet::new();
        for pattern in patterns {
            let AuthoredPattern::Try { patterns } = pattern.as_ref() else {
                continue;
            };
            let mut body_positive = BTreeSet::new();
            for child in patterns {
                child.collect_direct_positive_bindings(&mut body_positive);
            }
            for local in body_positive.difference(&mandatory) {
                if !optional.insert(*local) {
                    return Err(builder_failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_try_binding_shared",
                        "an optional binding belongs to exactly one try body",
                    ));
                }
            }
        }
        if !scoped_positive.is_disjoint(&optional) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_try_binding_shared",
                "an optional binding cannot also be a negation-local witness",
            ));
        }

        Ok(Self {
            mandatory,
            optional,
            pattern_bound: pattern_bindings(patterns),
            has_sort: false,
        })
    }

    fn visible(&self) -> BTreeSet<AuthoredBindingId> {
        self.mandatory.union(&self.optional).copied().collect()
    }

    fn ensure_visible(
        &self,
        bindings: impl IntoIterator<Item = AuthoredBindingId>,
    ) -> Result<(), Diagnostic> {
        let visible = self.visible();
        if bindings
            .into_iter()
            .any(|binding| !visible.contains(&binding))
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_stage_unknown_binding",
                "stage references a binding outside the visible row environment",
            ));
        }
        Ok(())
    }

    fn ensure_mandatory(
        &self,
        bindings: impl IntoIterator<Item = AuthoredBindingId>,
        message: &'static str,
    ) -> Result<(), Diagnostic> {
        if bindings
            .into_iter()
            .any(|binding| !self.mandatory.contains(&binding))
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_stage_unknown_binding",
                message,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum AuthoredStage {
    Match {
        patterns: Vec<Arc<AuthoredPattern>>,
    },
    Select {
        bindings: Vec<AuthoredBindingId>,
    },
    Require {
        bindings: Vec<AuthoredBindingId>,
    },
    Distinct,
    Reduce {
        assignments: Vec<AuthoredReduceAssignment>,
        groups: Vec<AuthoredBindingId>,
    },
    Sort {
        terms: Vec<(AuthoredBindingId, OrderDirection)>,
    },
    Offset {
        rows: u64,
    },
    Limit {
        rows: u64,
    },
}

impl AuthoredStage {
    const fn ordinal(&self) -> u8 {
        match self {
            Self::Match { .. } => 0,
            Self::Select { .. } => 1,
            Self::Require { .. } => 2,
            Self::Distinct => 3,
            Self::Reduce { .. } => 4,
            Self::Sort { .. } => 5,
            Self::Offset { .. } => 6,
            Self::Limit { .. } => 7,
        }
    }
}

#[derive(Clone, Debug)]
struct AuthoredReduceAssignment {
    assigned: AuthoredBindingId,
    input: Option<AuthoredBindingId>,
    reducer: Reducer,
}

#[derive(Clone)]
enum BuilderState {
    Active,
    Finalized(Box<AuthoredQueryPlan>),
}

/// One immutable finalized V2 plan.
#[derive(Clone)]
pub struct AuthoredQueryPlan {
    authority: QueryAuthorityIdentity,
    canonical_bytes: Vec<u8>,
    fingerprint: QueryPlanFingerprint,
    fingerprint_hex: String,
    plan: QueryPlan,
    required_capabilities: Vec<String>,
}

impl fmt::Debug for AuthoredQueryPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthoredQueryPlan")
            .field("format", &self.plan.format())
            .field("fingerprint", &self.fingerprint_hex)
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl AuthoredQueryPlan {
    /// Return an immutable copy of the canonical plan bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_bytes.clone()
    }

    /// Return the exact V2 format discriminator.
    #[must_use]
    pub fn format(&self) -> &str {
        self.plan.format()
    }

    /// Return the domain-separated plan fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &QueryPlanFingerprint {
        &self.fingerprint
    }

    /// Return the lower-hex plan fingerprint.
    #[must_use]
    pub fn fingerprint_hex(&self) -> &str {
        &self.fingerprint_hex
    }

    /// Return lexically sorted required capability identifiers.
    #[must_use]
    pub fn required_capabilities(&self) -> &[String] {
        &self.required_capabilities
    }

    /// Return this plan's opaque authority identity.
    #[must_use]
    pub const fn authority_identity(&self) -> &QueryAuthorityIdentity {
        &self.authority
    }

    /// Create one row-output invocation bound to this exact plan.
    pub fn rows(
        &self,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> Result<AuthoredQueryInvocation, Diagnostic> {
        if !matches!(self.plan.output(), QueryOutput::Rows { .. }) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_output_operation_mismatch",
                "rows invocations require a row-output authored plan",
            ));
        }
        self.invocation(QueryOperation::Rows, rows)
    }

    /// Create one document-output invocation bound to this exact plan.
    pub fn documents(
        &self,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> Result<AuthoredQueryInvocation, Diagnostic> {
        if !matches!(self.plan.output(), QueryOutput::Documents { .. }) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_output_operation_mismatch",
                "documents invocations require a document-output authored plan",
            ));
        }
        self.invocation(QueryOperation::Rows, rows)
    }

    /// Create one count invocation bound to this exact plan.
    pub fn count(
        &self,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> Result<AuthoredQueryInvocation, Diagnostic> {
        self.invocation(QueryOperation::Count, rows)
    }

    /// Create one existence invocation bound to this exact plan.
    pub fn exists(
        &self,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> Result<AuthoredQueryInvocation, Diagnostic> {
        self.invocation(QueryOperation::Exists, rows)
    }

    fn invocation(
        &self,
        operation: QueryOperation,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> Result<AuthoredQueryInvocation, Diagnostic> {
        let invocation = QueryInvocation::new(
            &self.plan,
            operation,
            rows.into_iter().map(InputRow::new).collect(),
        )?;
        let canonical_bytes = to_canonical_json(&invocation)?;
        let required_transport_capabilities = invocation
            .transport_capabilities()
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect();
        Ok(AuthoredQueryInvocation {
            authority: self.authority.clone(),
            canonical_bytes,
            invocation,
            required_transport_capabilities,
        })
    }

    /// Borrow the validated contract plan for Rust execution adapters.
    #[doc(hidden)]
    #[must_use]
    pub const fn contract_plan(&self) -> &QueryPlan {
        &self.plan
    }

    /// Borrow declared input columns for checked native host-value conversion.
    #[doc(hidden)]
    #[must_use]
    pub fn input_columns(&self) -> &[InputColumn] {
        self.plan.inputs()
    }
}

/// One immutable typed invocation bound to an authored plan fingerprint.
#[derive(Clone, Debug)]
pub struct AuthoredQueryInvocation {
    authority: QueryAuthorityIdentity,
    canonical_bytes: Vec<u8>,
    invocation: QueryInvocation,
    required_transport_capabilities: Vec<String>,
}

impl AuthoredQueryInvocation {
    /// Return an immutable copy of the canonical invocation bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_bytes.clone()
    }

    /// Return the closed invocation operation.
    #[must_use]
    pub const fn operation(&self) -> QueryOperation {
        self.invocation.operation()
    }

    /// Return the stable operation spelling.
    #[must_use]
    pub const fn operation_name(&self) -> &'static str {
        match self.invocation.operation() {
            QueryOperation::Rows => "rows",
            QueryOperation::Count => "count",
            QueryOperation::Exists => "exists",
        }
    }

    /// Return the exact plan fingerprint this invocation binds.
    #[must_use]
    pub const fn plan_fingerprint(&self) -> &QueryPlanFingerprint {
        self.invocation.plan_fingerprint()
    }

    /// Return the lower-hex fingerprint of the exact bound plan.
    #[must_use]
    pub fn plan_fingerprint_hex(&self) -> String {
        self.plan_fingerprint().as_fingerprint().digest().to_hex()
    }

    /// Return this invocation's opaque authority identity.
    #[must_use]
    pub const fn authority_identity(&self) -> &QueryAuthorityIdentity {
        &self.authority
    }

    /// Return lexically sorted invocation-derived transport capabilities.
    #[must_use]
    pub fn required_transport_capabilities(&self) -> &[String] {
        &self.required_transport_capabilities
    }

    /// Borrow the validated contract invocation for Rust execution adapters.
    #[doc(hidden)]
    #[must_use]
    pub const fn contract_invocation(&self) -> &QueryInvocation {
        &self.invocation
    }
}

/// The one shared incremental Rust builder for low-level V2 plan authoring.
pub struct QueryPlanBuilder {
    authority: Arc<QueryAuthority>,
    bindings: Vec<BindingDeclaration>,
    functions: Vec<LocalFunction>,
    inputs: Vec<InputColumn>,
    owner: HandleOwner,
    pattern_nodes: usize,
    pipeline: Vec<AuthoredStage>,
    root_environment: Option<RootRowEnvironment>,
    state: BuilderState,
}

/// Closed adapter-owned components accepted by the builder's compatibility
/// finalization gate.
///
/// The V1 adapter owns translation into this already-typed compatibility
/// algebra. It does not construct a `QueryPlan` directly: this value enters
/// the same component constructor and schema-aware validator used by ordinary
/// incremental authoring.
pub(crate) struct QueryCompatibilityPlanInput {
    bindings: Vec<AssertionBinding>,
    pipeline: Vec<ReadStage>,
    output: QueryOutput,
    compatibility: QueryPlanV2Compatibility,
}

impl QueryCompatibilityPlanInput {
    /// Package one closed compatibility projection for builder-owned
    /// finalization.
    pub(crate) const fn new(
        bindings: Vec<AssertionBinding>,
        pipeline: Vec<ReadStage>,
        output: QueryOutput,
        compatibility: QueryPlanV2Compatibility,
    ) -> Self {
        Self {
            bindings,
            pipeline,
            output,
            compatibility,
        }
    }
}

struct QueryPlanComponents {
    bindings: Vec<AssertionBinding>,
    functions: Vec<LocalFunction>,
    inputs: Vec<InputColumn>,
    pipeline: Vec<ReadStage>,
    output: QueryOutput,
    compatibility: QueryPlanV2Compatibility,
}

impl fmt::Debug for QueryPlanBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueryPlanBuilder")
            .field("authority", &self.owner.authority)
            .field("bindings", &self.bindings.len())
            .field("inputs", &self.inputs.len())
            .field("functions", &self.functions.len())
            .field("pattern_nodes", &self.pattern_nodes)
            .field("stages", &self.pipeline.len())
            .field(
                "finalized",
                &matches!(self.state, BuilderState::Finalized(_)),
            )
            .finish()
    }
}

impl QueryPlanBuilder {
    /// Start one builder under exactly one schema authority.
    #[must_use]
    pub fn new(authority: Arc<QueryAuthority>) -> Self {
        let owner = HandleOwner::new(authority.identity());
        Self {
            authority,
            bindings: Vec::new(),
            functions: Vec::new(),
            inputs: Vec::new(),
            owner,
            pattern_nodes: 0,
            pipeline: Vec::new(),
            root_environment: None,
            state: BuilderState::Active,
        }
    }

    /// Finalize one adapter-authored closed compatibility projection through
    /// the same V2 construction and schema-validation gate as incremental
    /// plans.
    pub(crate) fn finalize_compatibility(
        input: QueryCompatibilityPlanInput,
        context: &MigrationAssertionValidationContext<'_>,
        limits: StructuralLimits,
    ) -> Result<ValidatedQuery, Diagnostic> {
        validate_components(
            QueryPlanComponents {
                bindings: input.bindings,
                functions: Vec::new(),
                inputs: Vec::new(),
                pipeline: input.pipeline,
                output: input.output,
                compatibility: input.compatibility,
            },
            context,
            limits,
        )
    }

    /// Return the first finalized value, if finalization succeeded.
    #[must_use]
    pub fn finalized_plan(&self) -> Option<&AuthoredQueryPlan> {
        match &self.state {
            BuilderState::Active => None,
            BuilderState::Finalized(plan) => Some(plan.as_ref()),
        }
    }

    /// Declare one builder-owned binding symbol.
    pub fn binding(
        &mut self,
        variable: impl Into<String>,
    ) -> Result<QueryBindingHandle, Diagnostic> {
        self.ensure_active()?;
        if !StructuralLimits::CANONICAL.allows_bindings(self.bindings.len() + 1) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_builder_authored_binding_limit",
                "authored binding symbols exceed the canonical binding ceiling",
            ));
        }
        let variable = QueryVariable::new(variable)?;
        let id = AuthoredBindingId(self.bindings.len());
        self.bindings.push(BindingDeclaration {
            scope: None,
            variable,
        });
        Ok(QueryBindingHandle {
            id,
            owner: self.owner.clone(),
        })
    }

    /// Declare one dense typed invocation input.
    pub fn input(
        &mut self,
        public_name: impl Into<String>,
        value_type: ValueTypeTag,
        optional: bool,
    ) -> Result<QueryInputHandle, Diagnostic> {
        self.ensure_active()?;
        if !StructuralLimits::CANONICAL.allows_bindings(self.inputs.len() + 1) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_input_limit",
                "plan input column count exceeds the structural ceiling",
            ));
        }
        let public_name = QueryVariable::new(public_name)?;
        if self
            .inputs
            .iter()
            .any(|input| input.public_name() == &public_name)
            || self.bindings.iter().any(|binding| {
                binding.scope == Some(BindingScope::Root) && binding.variable == public_name
            })
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_duplicate_variable",
                "input column names must not collide with root query variables",
            ));
        }
        let ordinal = u16::try_from(self.inputs.len()).map_err(|_| {
            builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_input_limit",
                "plan input column count exceeds the structural ceiling",
            )
        })?;
        let id = InputColumnId::new(ordinal);
        self.inputs
            .push(InputColumn::new(id, public_name, value_type, optional));
        Ok(QueryInputHandle {
            id,
            owner: self.owner.clone(),
        })
    }

    /// Create one operand reading a scalar binding.
    pub fn binding_operand(
        &self,
        binding: &QueryBindingHandle,
    ) -> Result<QueryOperandHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&binding.owner)?;
        Ok(QueryOperandHandle {
            operand: AuthoredOperand::Binding(binding.id),
            owner: self.owner.clone(),
        })
    }

    /// Create one exact canonical literal operand.
    pub fn literal_operand(&self, value: CanonicalValue) -> Result<QueryOperandHandle, Diagnostic> {
        self.ensure_active()?;
        Ok(QueryOperandHandle {
            operand: AuthoredOperand::Literal(value),
            owner: self.owner.clone(),
        })
    }

    /// Create one operand reading a declared invocation input.
    pub fn input_operand(
        &self,
        input: &QueryInputHandle,
    ) -> Result<QueryOperandHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&input.owner)?;
        Ok(QueryOperandHandle {
            operand: AuthoredOperand::Input(input.id),
            owner: self.owner.clone(),
        })
    }

    /// Create one exact/subtype type pattern.
    pub fn isa(
        &self,
        binding: &QueryBindingHandle,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&binding.owner)?;
        self.pattern(AuthoredPattern::Isa {
            binding: binding.id,
            include_subtypes,
            type_id,
        })
    }

    /// Create one effective ownership pattern.
    pub fn has(
        &self,
        owner: &QueryBindingHandle,
        attribute: &QueryBindingHandle,
        attribute_id: AttributeId,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&owner.owner)?;
        self.ensure_owner(&attribute.owner)?;
        self.pattern(AuthoredPattern::Has {
            attribute: attribute.id,
            attribute_id,
            owner: owner.id,
        })
    }

    /// Create one relation and descriptor-qualified role-player pattern.
    pub fn links(
        &self,
        relation: &QueryBindingHandle,
        relation_id: TypeId,
        players: Vec<(RoleId, QueryBindingHandle)>,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&relation.owner)?;
        if players.is_empty() || players.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_role_player_limit_error());
        }
        let mut authored = Vec::with_capacity(players.len());
        for (role, player) in players {
            self.ensure_owner(&player.owner)?;
            authored.push((role, player.id));
        }
        self.pattern(AuthoredPattern::Links {
            players: authored,
            relation: relation.id,
            relation_id,
        })
    }

    /// Create one typed scalar comparison.
    pub fn value(
        &self,
        comparator: ValueComparator,
        left: &QueryOperandHandle,
        right: &QueryOperandHandle,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&left.owner)?;
        self.ensure_owner(&right.owner)?;
        self.pattern(AuthoredPattern::Value {
            comparator,
            left: left.operand.clone(),
            right: right.operand.clone(),
        })
    }

    /// Create one nested negated conjunction.
    pub fn not(&self, patterns: Vec<QueryPatternHandle>) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_pattern_owners(&patterns)?;
        if patterns.is_empty() || patterns.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_negation_term_limit_error());
        }
        self.ensure_patterns_nestable(&patterns)?;
        let metrics = PatternMetrics::composite(patterns.iter().map(|pattern| pattern.metrics))?;
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.pattern)
            .collect();
        self.pattern_with_metrics(AuthoredPattern::Not { patterns }, metrics)
    }

    /// Create one closed disjunction of non-empty conjunction branches.
    pub fn or(
        &self,
        branches: Vec<Vec<QueryPatternHandle>>,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        if branches.is_empty() || branches.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_disjunction_term_limit_error());
        }
        let mut authored = Vec::with_capacity(branches.len());
        let mut child_metrics = Vec::new();
        for branch in branches {
            self.ensure_pattern_owners(&branch)?;
            if branch.is_empty() || branch.len() > MAX_BOOLEAN_TERMS {
                return Err(query_builder_disjunction_term_limit_error());
            }
            self.ensure_patterns_nestable(&branch)?;
            child_metrics.extend(branch.iter().map(|pattern| pattern.metrics));
            authored.push(branch.into_iter().map(|pattern| pattern.pattern).collect());
        }
        let metrics = PatternMetrics::composite(child_metrics)?;
        self.pattern_with_metrics(AuthoredPattern::Or { branches: authored }, metrics)
    }

    /// Create one root-only optional conjunction.
    pub fn r#try(
        &self,
        patterns: Vec<QueryPatternHandle>,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_pattern_owners(&patterns)?;
        if patterns.is_empty() || patterns.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_try_term_limit_error());
        }
        if patterns.iter().any(|pattern| !pattern.pattern.is_flat()) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_try_body_unsupported",
                "the first optional vocabulary admits only isa, has, links, and value patterns",
            ));
        }
        let mut metrics =
            PatternMetrics::composite(patterns.iter().map(|pattern| pattern.metrics))?;
        metrics.root_only = Some(RootOnlyPattern::Try);
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.pattern)
            .collect();
        self.pattern_with_metrics(AuthoredPattern::Try { patterns }, metrics)
    }

    /// Create one root-only finite directed reachability predicate.
    #[expect(
        clippy::too_many_arguments,
        reason = "the operation mirrors the closed reachability contract"
    )]
    pub fn reachable(
        &self,
        source: &QueryBindingHandle,
        target: &QueryBindingHandle,
        relation: TypeId,
        role_from: RoleId,
        role_to: RoleId,
        min_depth: u8,
        max_depth: u8,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&source.owner)?;
        self.ensure_owner(&target.owner)?;
        if min_depth > max_depth {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_reachable_bounds",
                "reachability minimum depth must not exceed its maximum depth",
            ));
        }
        if usize::from(max_depth) > MAX_PREDICATE_DEPTH {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_reachable_depth",
                "reachability requires a finite hop bound within the depth ceiling",
            ));
        }
        let expanded = reachability_nodes(min_depth, max_depth);
        if expanded > MAX_PREDICATE_NODES {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_reachable_expansion_limit",
                "reachability expansion exceeds the plan pattern-node ceiling",
            ));
        }
        self.pattern_with_metrics(
            AuthoredPattern::Reachable {
                min_depth,
                max_depth,
                relation,
                role_from,
                role_to,
                source: source.id,
                target: target.id,
            },
            PatternMetrics {
                depth: 1,
                nodes: expanded,
                root_only: Some(RootOnlyPattern::Reachable),
            },
        )
    }

    /// Create one root-only scalar function assignment.
    pub fn function_call(
        &self,
        assigned: &QueryBindingHandle,
        function: QueryFunctionTarget<'_>,
        arguments: Vec<QueryOperandHandle>,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&assigned.owner)?;
        if arguments.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_function_argument_limit_error());
        }
        let function = match function {
            QueryFunctionTarget::Schema(function) => function,
            QueryFunctionTarget::Local(function) => {
                self.ensure_owner(&function.owner)?;
                function.function.clone()
            }
        };
        let mut authored = Vec::with_capacity(arguments.len());
        for argument in arguments {
            self.ensure_owner(&argument.owner)?;
            authored.push(argument.operand);
        }
        self.pattern_with_metrics(
            AuthoredPattern::FunctionCall {
                arguments: authored,
                assigned: assigned.id,
                function,
            },
            PatternMetrics::FUNCTION,
        )
    }

    /// Create one typed sort term.
    pub fn order(
        &self,
        binding: &QueryBindingHandle,
        direction: OrderDirection,
    ) -> Result<QueryOrderHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&binding.owner)?;
        Ok(QueryOrderHandle {
            binding: binding.id,
            direction,
            owner: self.owner.clone(),
        })
    }

    /// Create one reducer assignment to a fresh binding.
    pub fn reduce_assignment(
        &self,
        assigned: &QueryBindingHandle,
        reducer: Reducer,
        input: Option<&QueryBindingHandle>,
    ) -> Result<QueryReduceAssignmentHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&assigned.owner)?;
        if let Some(input) = input {
            self.ensure_owner(&input.owner)?;
        }
        if reducer.requires_input() != input.is_some() {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_reduce_missing_input",
                "count takes no input and every other reducer requires one",
            ));
        }
        Ok(QueryReduceAssignmentHandle {
            assigned: assigned.id,
            input: input.map(|binding| binding.id),
            owner: self.owner.clone(),
            reducer,
        })
    }

    /// Create one total scalar local-function return.
    pub fn local_return(
        &self,
        reducer: Reducer,
        input: &QueryBindingHandle,
        value_type: ValueTypeTag,
    ) -> Result<QueryLocalReturnHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&input.owner)?;
        if !QUERY_BUILDER_LOCAL_RETURNS.contains(&(reducer, value_type)) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_return_type",
                "local returns admit count as long or sum as long/double",
            ));
        }
        Ok(QueryLocalReturnHandle {
            input: input.id,
            owner: self.owner.clone(),
            reducer,
            value_type,
        })
    }

    /// Consume one exact private binding set into a plan-local function.
    ///
    /// Parameter handles are remapped first in explicit parameter order.
    /// Every remaining consumed handle is remapped by immutable declaration
    /// ordinal. The claim is atomic and no consumed symbol can also belong to
    /// the root or another function.
    pub fn local_function(
        &mut self,
        name: FunctionId,
        bindings: Vec<QueryBindingHandle>,
        parameters: Vec<(QueryBindingHandle, Label)>,
        body: Vec<QueryPatternHandle>,
        returns: &QueryLocalReturnHandle,
    ) -> Result<QueryLocalFunctionHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&returns.owner)?;
        for binding in &bindings {
            self.ensure_owner(&binding.owner)?;
        }
        for (binding, _) in &parameters {
            self.ensure_owner(&binding.owner)?;
        }
        self.ensure_pattern_owners(&body)?;
        if self
            .functions
            .iter()
            .any(|function| function.name() == &name)
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_duplicate_local_function",
                "plan-local function names must be unique",
            ));
        }
        if self
            .authority
            .context()
            .resolved_schema()
            .functions()
            .contains_key(&name)
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_shadows_schema",
                "a plan-local function cannot shadow a schema function",
            ));
        }
        if !StructuralLimits::CANONICAL.allows_bindings(self.functions.len() + 1) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_local_function_limit",
                "plan-local function count exceeds the structural ceiling",
            ));
        }
        if bindings.is_empty() || !StructuralLimits::CANONICAL.allows_bindings(bindings.len()) {
            return Err(query_builder_local_binding_limit_error());
        }
        if parameters.is_empty() || parameters.len() > bindings.len() {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_parameters",
                "parameters must be a non-empty subset of local bindings",
            ));
        }
        if body.is_empty() || body.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_local_body_limit_error());
        }

        let consumed = self.binding_handle_set(&bindings)?;
        let mut local_names = BTreeSet::new();
        for binding in &consumed {
            let declaration = self.binding_declaration(*binding)?;
            if !local_names.insert(declaration.variable.clone()) {
                return Err(scope_failure(
                    "query_plan_duplicate_variable",
                    "local query variables must be unique",
                    &declaration.variable,
                ));
            }
        }
        let mut parameter_ids = Vec::with_capacity(parameters.len());
        let mut parameter_labels = Vec::with_capacity(parameters.len());
        let mut seen_parameters = BTreeSet::new();
        for (binding, label) in parameters {
            self.ensure_owner(&binding.owner)?;
            if !consumed.contains(&binding.id) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_builder_local_parameter_not_consumed",
                    "every local parameter must belong to the consumed binding set",
                ));
            }
            if !seen_parameters.insert(binding.id) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_builder_duplicate_local_parameter",
                    "a local binding cannot appear in the parameter list twice",
                ));
            }
            parameter_ids.push(binding.id);
            parameter_labels.push(label);
        }

        let body = self.pattern_values(body)?;
        if body.iter().any(|pattern| !pattern.is_flat()) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_body_unsupported",
                "local bodies admit only isa, has, links, and value patterns",
            ));
        }
        if body.iter().any(|pattern| pattern.references_input()) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_local_function_input",
                "plan-local functions cannot reference invocation input columns",
            ));
        }
        let aggregate_nodes = self.pattern_nodes.saturating_add(body.len());
        if !StructuralLimits::CANONICAL.allows_predicate_nodes(aggregate_nodes) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_pattern_node_limit",
                "aggregate local and root pattern count exceeds the structural ceiling",
            ));
        }

        let mut referenced = BTreeSet::new();
        for pattern in &body {
            pattern.collect_bindings(&mut referenced);
        }
        referenced.insert(returns.input);
        if !referenced.is_subset(&consumed) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_local_binding_omitted",
                "the consumed local binding set omits a body or return binding",
            ));
        }
        if consumed != referenced {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_unused_local_binding",
                "every consumed local binding must be referenced by its body or return",
            ));
        }

        let local_ordinal = self.functions.len();
        self.preflight_scope_claim(&consumed, BindingScope::Local(local_ordinal))?;

        let mut dense_order = parameter_ids;
        dense_order.extend(
            consumed
                .iter()
                .copied()
                .filter(|id| !seen_parameters.contains(id)),
        );
        let map = dense_binding_map(&dense_order, self.bindings.len())?;
        let local_bindings = dense_order
            .iter()
            .map(|id| {
                Ok(AssertionBinding::new(
                    mapped_binding(&map, *id)?,
                    self.binding_declaration(*id)?.variable.clone(),
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let body = body
            .iter()
            .map(|pattern| pattern.to_contract(&map))
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let returns = LocalReturn::new(
            returns.reducer,
            mapped_binding(&map, returns.input)?,
            returns.value_type,
        );
        let function = LocalFunction::new(
            name.clone(),
            local_bindings,
            parameter_labels,
            body,
            returns,
        );
        validate_query_local_function(&function, &self.authority.context())?;

        self.commit_scope_claim(&consumed, BindingScope::Local(local_ordinal));
        self.functions.push(function);
        self.pattern_nodes = aggregate_nodes;
        Ok(QueryLocalFunctionHandle {
            function: name,
            owner: self.owner.clone(),
        })
    }

    /// Attach the one root match conjunction.
    pub fn r#match(&mut self, patterns: Vec<QueryPatternHandle>) -> Result<(), Diagnostic> {
        self.ensure_active()?;
        if !self.pipeline.is_empty() {
            return Err(stage_order_failure());
        }
        let patterns = self.pattern_values(patterns)?;
        let root_nodes = validate_root_patterns(&patterns)?;
        let aggregate_nodes = self.pattern_nodes.saturating_add(root_nodes);
        if !StructuralLimits::CANONICAL.allows_predicate_nodes(aggregate_nodes) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_pattern_node_limit",
                "aggregate local and root pattern count exceeds the structural ceiling",
            ));
        }
        let symbols = pattern_bindings(&patterns);
        self.preflight_scope_claim(&symbols, BindingScope::Root)?;
        let environment = RootRowEnvironment::from_patterns(&patterns)?;
        self.validate_root_prefix(
            AuthoredStage::Match {
                patterns: patterns.clone(),
            },
            &environment,
            &symbols,
        )?;
        self.commit_scope_claim(&symbols, BindingScope::Root);
        self.pipeline.push(AuthoredStage::Match { patterns });
        self.pattern_nodes = aggregate_nodes;
        self.root_environment = Some(environment);
        Ok(())
    }

    /// Attach one canonical visible-binding selection.
    pub fn select(&mut self, bindings: Vec<QueryBindingHandle>) -> Result<(), Diagnostic> {
        self.preflight_stage(1)?;
        let bindings = self.sorted_binding_handles(bindings, true)?;
        let mut environment = self.root_environment()?.clone();
        environment.ensure_visible(bindings.iter().copied())?;
        let selected = bindings.iter().copied().collect::<BTreeSet<_>>();
        environment
            .mandatory
            .retain(|binding| selected.contains(binding));
        environment
            .optional
            .retain(|binding| selected.contains(binding));
        let stage = AuthoredStage::Select { bindings };
        self.validate_root_prefix(stage.clone(), &environment, &BTreeSet::new())?;
        self.pipeline.push(stage);
        self.root_environment = Some(environment);
        Ok(())
    }

    /// Attach one canonical mandatory-binding requirement.
    pub fn require(&mut self, bindings: Vec<QueryBindingHandle>) -> Result<(), Diagnostic> {
        self.preflight_stage(2)?;
        let bindings = self.sorted_binding_handles(bindings, true)?;
        let environment = self.root_environment()?;
        environment.ensure_visible(bindings.iter().copied())?;
        if bindings
            .iter()
            .any(|binding| environment.optional.contains(binding))
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_require_optional_reserved",
                "requiring an optional binding is reserved in this vocabulary",
            ));
        }
        let stage = AuthoredStage::Require { bindings };
        self.validate_root_prefix(stage.clone(), environment, &BTreeSet::new())?;
        self.pipeline.push(stage);
        Ok(())
    }

    /// Attach row-environment deduplication.
    pub fn distinct(&mut self) -> Result<(), Diagnostic> {
        self.preflight_stage(3)?;
        let stage = AuthoredStage::Distinct;
        self.validate_root_prefix(stage.clone(), self.root_environment()?, &BTreeSet::new())?;
        self.push_stage(stage)
    }

    /// Attach one grouped or global reduction stage.
    pub fn reduce(
        &mut self,
        assignments: Vec<QueryReduceAssignmentHandle>,
        groups: Vec<QueryBindingHandle>,
    ) -> Result<(), Diagnostic> {
        self.preflight_stage(4)?;
        if assignments.is_empty() || assignments.len() > MAX_BOOLEAN_TERMS {
            return Err(query_builder_reduce_term_limit_error());
        }
        let groups = self.sorted_binding_handles(groups, false)?;
        for assignment in &assignments {
            self.ensure_owner(&assignment.owner)?;
        }
        if groups.is_empty()
            && assignments
                .iter()
                .any(|assignment| !assignment.reducer.total_without_groups())
        {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_reduce_requires_groups",
                "reducers undefined on empty streams require group bindings",
            ));
        }
        let environment = self.root_environment()?.clone();
        environment.ensure_mandatory(
            groups.iter().copied(),
            "reduce groups a binding outside the mandatory row environment",
        )?;
        let mut symbols = groups.iter().copied().collect::<BTreeSet<_>>();
        let mut next_mandatory = symbols.clone();
        let mut authored = Vec::with_capacity(assignments.len());
        let mut assigned = BTreeSet::new();
        for assignment in assignments {
            if environment.pattern_bound.contains(&assignment.assigned)
                || !assigned.insert(assignment.assigned)
                || !next_mandatory.insert(assignment.assigned)
            {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_reduce_assigned_bound",
                    "reduce must assign a fresh binding free of patterns and groups",
                ));
            }
            symbols.insert(assignment.assigned);
            if let Some(input) = assignment.input {
                environment.ensure_visible([input])?;
                if environment.optional.contains(&input)
                    && !assignment.reducer.total_without_groups()
                {
                    return Err(builder_failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_reduce_optional_input",
                        "this reducer is undefined over an optional input",
                    ));
                }
                symbols.insert(input);
            }
            authored.push(AuthoredReduceAssignment {
                assigned: assignment.assigned,
                input: assignment.input,
                reducer: assignment.reducer,
            });
        }
        self.preflight_scope_claim(&symbols, BindingScope::Root)?;
        let next_environment = RootRowEnvironment {
            mandatory: next_mandatory,
            optional: BTreeSet::new(),
            pattern_bound: environment.pattern_bound,
            has_sort: false,
        };
        let stage = AuthoredStage::Reduce {
            assignments: authored,
            groups,
        };
        self.validate_root_prefix(stage.clone(), &next_environment, &symbols)?;
        self.commit_scope_claim(&symbols, BindingScope::Root);
        self.pipeline.push(stage);
        self.root_environment = Some(next_environment);
        Ok(())
    }

    /// Attach one ordered total-sort stage.
    pub fn sort(&mut self, terms: Vec<QueryOrderHandle>) -> Result<(), Diagnostic> {
        self.preflight_stage(5)?;
        if terms.is_empty() || !StructuralLimits::CANONICAL.allows_order_terms(terms.len()) {
            return Err(query_builder_sort_term_limit_error());
        }
        let mut symbols = BTreeSet::new();
        let mut authored = Vec::with_capacity(terms.len());
        for term in terms {
            self.ensure_owner(&term.owner)?;
            if !symbols.insert(term.binding) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_sort_binding",
                    "sort references one binding twice",
                ));
            }
            authored.push((term.binding, term.direction));
        }
        self.root_environment()?.ensure_mandatory(
            symbols.iter().copied(),
            "sort references a binding outside the mandatory row environment",
        )?;
        let mut environment = self.root_environment()?.clone();
        environment.has_sort = true;
        let stage = AuthoredStage::Sort { terms: authored };
        self.validate_root_prefix(stage.clone(), &environment, &BTreeSet::new())?;
        self.pipeline.push(stage);
        self.root_environment = Some(environment);
        Ok(())
    }

    /// Attach one exact ordered row offset.
    pub fn offset(&mut self, rows: u64) -> Result<(), Diagnostic> {
        self.preflight_stage(6)?;
        self.ensure_ordered_truncation()?;
        let stage = AuthoredStage::Offset { rows };
        self.validate_root_prefix(stage.clone(), self.root_environment()?, &BTreeSet::new())?;
        self.push_stage(stage)
    }

    /// Attach one exact ordered row limit.
    pub fn limit(&mut self, rows: u64) -> Result<(), Diagnostic> {
        self.preflight_stage(7)?;
        self.ensure_ordered_truncation()?;
        let stage = AuthoredStage::Limit { rows };
        self.validate_root_prefix(stage.clone(), self.root_environment()?, &BTreeSet::new())?;
        self.push_stage(stage)
    }

    /// Create one document field from a visible scalar binding.
    pub fn document_binding(
        &self,
        key: impl Into<String>,
        binding: &QueryBindingHandle,
    ) -> Result<QueryDocumentFieldHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&binding.owner)?;
        Ok(QueryDocumentFieldHandle {
            key: QueryVariable::new(key)?,
            owner: self.owner.clone(),
            source: AuthoredDocumentSource::Binding(binding.id),
        })
    }

    /// Create one document field listing an exact attribute on a mandatory owner.
    pub fn document_attribute_list(
        &self,
        key: impl Into<String>,
        owner: &QueryBindingHandle,
        attribute: AttributeId,
    ) -> Result<QueryDocumentFieldHandle, Diagnostic> {
        self.ensure_active()?;
        self.ensure_owner(&owner.owner)?;
        Ok(QueryDocumentFieldHandle {
            key: QueryVariable::new(key)?,
            owner: self.owner.clone(),
            source: AuthoredDocumentSource::AttributeList {
                attribute,
                owner: owner.id,
            },
        })
    }

    /// Finalize one row-output V2 plan.
    pub fn finalize_rows(
        &mut self,
        columns: Vec<QueryBindingHandle>,
    ) -> Result<AuthoredQueryPlan, Diagnostic> {
        self.ensure_active()?;
        self.ensure_match_attached()?;
        if columns.is_empty() || !StructuralLimits::CANONICAL.allows_selected_slots(columns.len()) {
            return Err(query_builder_row_output_limit_error());
        }
        let mut seen = BTreeSet::new();
        let mut authored = Vec::with_capacity(columns.len());
        for column in columns {
            self.ensure_owner(&column.owner)?;
            self.ensure_root_scoped(column.id)?;
            if !self.root_environment()?.visible().contains(&column.id) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_output_not_visible",
                    "output projects a binding outside the visible row environment",
                ));
            }
            if !seen.insert(column.id) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_output_column",
                    "output projects one binding twice",
                ));
            }
            authored.push(column.id);
        }
        self.finalize(AuthoredOutput::Rows(authored))
    }

    /// Finalize one flat-document-output V2 plan.
    pub fn finalize_documents(
        &mut self,
        fields: Vec<QueryDocumentFieldHandle>,
    ) -> Result<AuthoredQueryPlan, Diagnostic> {
        self.ensure_active()?;
        self.ensure_match_attached()?;
        if fields.is_empty() || !StructuralLimits::CANONICAL.allows_selected_slots(fields.len()) {
            return Err(query_builder_document_output_limit_error());
        }
        let mut keys = BTreeSet::new();
        let mut authored = Vec::with_capacity(fields.len());
        for field in fields {
            self.ensure_owner(&field.owner)?;
            if !keys.insert(field.key.clone()) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_duplicate_output_column",
                    "documents fetch one key twice",
                ));
            }
            match field.source {
                AuthoredDocumentSource::Binding(binding) => {
                    self.ensure_root_scoped(binding)?;
                    if !self.root_environment()?.visible().contains(&binding) {
                        return Err(builder_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_output_not_visible",
                            "output projects a binding outside the visible row environment",
                        ));
                    }
                }
                AuthoredDocumentSource::AttributeList { owner, .. } => {
                    self.ensure_root_scoped(owner)?;
                    if !self.root_environment()?.mandatory.contains(&owner) {
                        return Err(builder_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_output_not_visible",
                            "attribute lists require a mandatory owner binding",
                        ));
                    }
                }
            }
            authored.push((field.key, field.source));
        }
        self.finalize(AuthoredOutput::Documents(authored))
    }

    fn finalize(&mut self, output: AuthoredOutput) -> Result<AuthoredQueryPlan, Diagnostic> {
        self.ensure_match_attached()?;
        let root_order = self
            .bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                (binding.scope == Some(BindingScope::Root)).then_some(AuthoredBindingId(index))
            })
            .collect::<Vec<_>>();
        if root_order.is_empty() || !StructuralLimits::CANONICAL.allows_bindings(root_order.len()) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_binding_limit",
                "plan binding count is empty or exceeds the structural ceiling",
            ));
        }
        let map = dense_binding_map(&root_order, self.bindings.len())?;
        let bindings = root_order
            .iter()
            .map(|id| {
                Ok(AssertionBinding::new(
                    mapped_binding(&map, *id)?,
                    self.binding_declaration(*id)?.variable.clone(),
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let pipeline = self
            .pipeline
            .iter()
            .map(|stage| stage.to_contract(&map))
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let output = output.to_contract(&map)?;
        let validated = validate_components(
            QueryPlanComponents {
                bindings,
                functions: self.functions.clone(),
                inputs: self.inputs.clone(),
                pipeline,
                output,
                compatibility: QueryPlanV2Compatibility::native(),
            },
            &self.authority.context(),
            StructuralLimits::CANONICAL,
        )?;
        let plan = validated.plan().clone();
        let canonical_bytes = plan.canonical_bytes()?;
        let fingerprint = plan.fingerprint()?;
        let fingerprint_hex = fingerprint.as_fingerprint().digest().to_hex();
        let required_capabilities = plan
            .required_capabilities()
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect();
        let authored = AuthoredQueryPlan {
            authority: self.owner.authority.clone(),
            canonical_bytes,
            fingerprint,
            fingerprint_hex,
            plan,
            required_capabilities,
        };
        self.state = BuilderState::Finalized(Box::new(authored.clone()));
        Ok(authored)
    }

    fn ensure_match_attached(&self) -> Result<(), Diagnostic> {
        if !matches!(self.pipeline.first(), Some(AuthoredStage::Match { .. })) {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_match_required",
                "a builder can finalize only after one root match stage",
            ));
        }
        Ok(())
    }

    fn pattern(&self, pattern: AuthoredPattern) -> Result<QueryPatternHandle, Diagnostic> {
        self.pattern_with_metrics(pattern, PatternMetrics::LEAF)
    }

    fn pattern_with_metrics(
        &self,
        pattern: AuthoredPattern,
        metrics: PatternMetrics,
    ) -> Result<QueryPatternHandle, Diagnostic> {
        metrics.ensure_bounded()?;
        Ok(QueryPatternHandle {
            metrics,
            owner: self.owner.clone(),
            pattern: Arc::new(pattern),
        })
    }

    fn pattern_values(
        &self,
        patterns: Vec<QueryPatternHandle>,
    ) -> Result<Vec<Arc<AuthoredPattern>>, Diagnostic> {
        patterns
            .into_iter()
            .map(|pattern| {
                self.ensure_owner(&pattern.owner)?;
                Ok(pattern.pattern)
            })
            .collect()
    }

    fn ensure_pattern_owners(&self, patterns: &[QueryPatternHandle]) -> Result<(), Diagnostic> {
        for pattern in patterns {
            self.ensure_owner(&pattern.owner)?;
        }
        Ok(())
    }

    fn ensure_patterns_nestable(&self, patterns: &[QueryPatternHandle]) -> Result<(), Diagnostic> {
        for pattern in patterns {
            if let Some(root_only) = pattern.metrics.root_only {
                return Err(root_only.nested_failure());
            }
        }
        Ok(())
    }

    fn binding_handle_set(
        &self,
        bindings: &[QueryBindingHandle],
    ) -> Result<BTreeSet<AuthoredBindingId>, Diagnostic> {
        let mut set = BTreeSet::new();
        for binding in bindings {
            self.ensure_owner(&binding.owner)?;
            if !set.insert(binding.id) {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_builder_duplicate_binding_handle",
                    "one binding handle cannot appear twice in this binding set",
                ));
            }
        }
        Ok(set)
    }

    fn sorted_binding_handles(
        &self,
        bindings: Vec<QueryBindingHandle>,
        require_nonempty: bool,
    ) -> Result<Vec<AuthoredBindingId>, Diagnostic> {
        if require_nonempty && bindings.is_empty() {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_empty_stage_set",
                "stage binding sets must be non-empty",
            ));
        }
        let set = self.binding_handle_set(&bindings)?;
        Ok(set.into_iter().collect())
    }

    fn push_stage(&mut self, stage: AuthoredStage) -> Result<(), Diagnostic> {
        self.preflight_stage(stage.ordinal())?;
        self.pipeline.push(stage);
        Ok(())
    }

    fn validate_root_prefix(
        &self,
        stage: AuthoredStage,
        environment: &RootRowEnvironment,
        additional_symbols: &BTreeSet<AuthoredBindingId>,
    ) -> Result<(), Diagnostic> {
        let root_order = self
            .bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                let id = AuthoredBindingId(index);
                (binding.scope == Some(BindingScope::Root) || additional_symbols.contains(&id))
                    .then_some(id)
            })
            .collect::<Vec<_>>();
        if root_order.is_empty() || !StructuralLimits::CANONICAL.allows_bindings(root_order.len()) {
            return Err(builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_binding_limit",
                "plan binding count is empty or exceeds the structural ceiling",
            ));
        }
        let map = dense_binding_map(&root_order, self.bindings.len())?;
        let bindings = root_order
            .iter()
            .map(|id| {
                Ok(AssertionBinding::new(
                    mapped_binding(&map, *id)?,
                    self.binding_declaration(*id)?.variable.clone(),
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let mut pipeline = self.pipeline.clone();
        pipeline.push(stage);
        let pipeline = pipeline
            .iter()
            .map(|stage| stage.to_contract(&map))
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let output = environment.visible().into_iter().next().ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_output_not_visible",
                "a root prefix must establish at least one visible binding",
            )
        })?;
        validate_components(
            QueryPlanComponents {
                bindings,
                functions: self.functions.clone(),
                inputs: self.inputs.clone(),
                pipeline,
                output: QueryOutput::Rows {
                    columns: vec![mapped_binding(&map, output)?],
                },
                compatibility: QueryPlanV2Compatibility::native(),
            },
            &self.authority.context(),
            StructuralLimits::CANONICAL,
        )?;
        Ok(())
    }

    fn root_environment(&self) -> Result<&RootRowEnvironment, Diagnostic> {
        self.root_environment.as_ref().ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::Integrity,
                "query_builder_root_environment_missing",
                "an attached root match has no row-environment state",
            )
        })
    }

    fn ensure_ordered_truncation(&self) -> Result<(), Diagnostic> {
        if self.root_environment()?.has_sort {
            Ok(())
        } else {
            Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_unordered_truncation",
                "offset and limit require an explicit total sort order",
            ))
        }
    }

    fn preflight_stage(&self, ordinal: u8) -> Result<(), Diagnostic> {
        self.ensure_active()?;
        let Some(previous) = self.pipeline.last() else {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_match_required",
                "one root match stage must precede every later stage",
            ));
        };
        if ordinal <= previous.ordinal() {
            return Err(stage_order_failure());
        }
        Ok(())
    }

    fn preflight_scope_claim(
        &self,
        symbols: &BTreeSet<AuthoredBindingId>,
        scope: BindingScope,
    ) -> Result<(), Diagnostic> {
        let mut prospective_names = BTreeSet::new();
        let mut prospective_count = 0usize;
        if scope == BindingScope::Root {
            prospective_names.extend(self.inputs.iter().map(|input| input.public_name().clone()));
            for declaration in &self.bindings {
                if declaration.scope == Some(BindingScope::Root) {
                    prospective_count += 1;
                    prospective_names.insert(declaration.variable.clone());
                }
            }
        }
        for symbol in symbols {
            let declaration = self.binding_declaration(*symbol)?;
            match declaration.scope {
                None => {}
                Some(existing) if existing == scope && scope == BindingScope::Root => continue,
                Some(BindingScope::Root) => {
                    return Err(scope_failure(
                        "query_builder_binding_already_root",
                        "a root binding cannot be consumed by a local function",
                        &declaration.variable,
                    ));
                }
                Some(BindingScope::Local(_)) => {
                    return Err(scope_failure(
                        "query_builder_binding_already_local",
                        "a local-function binding cannot be reused by root or another function",
                        &declaration.variable,
                    ));
                }
            }
            if scope == BindingScope::Root {
                prospective_count += 1;
                if !prospective_names.insert(declaration.variable.clone()) {
                    return Err(scope_failure(
                        "query_plan_duplicate_variable",
                        "root query variables must be unique",
                        &declaration.variable,
                    ));
                }
            }
        }
        if scope == BindingScope::Root
            && !StructuralLimits::CANONICAL.allows_bindings(prospective_count)
        {
            return Err(query_builder_binding_limit_error());
        }
        Ok(())
    }

    fn commit_scope_claim(&mut self, symbols: &BTreeSet<AuthoredBindingId>, scope: BindingScope) {
        for symbol in symbols {
            if self.bindings[symbol.0].scope.is_none() {
                self.bindings[symbol.0].scope = Some(scope);
            }
        }
    }

    fn ensure_root_scoped(&self, symbol: AuthoredBindingId) -> Result<(), Diagnostic> {
        let declaration = self.binding_declaration(symbol)?;
        if declaration.scope == Some(BindingScope::Root) {
            Ok(())
        } else {
            Err(scope_failure(
                "query_builder_output_binding_not_root",
                "terminal outputs may reference only bindings already attached to the root",
                &declaration.variable,
            ))
        }
    }

    fn binding_declaration(
        &self,
        id: AuthoredBindingId,
    ) -> Result<&BindingDeclaration, Diagnostic> {
        self.bindings.get(id.0).ok_or_else(|| {
            builder_failure(
                DiagnosticCategory::Integrity,
                "query_builder_unknown_binding_handle",
                "a builder-owned binding handle has no declaration",
            )
        })
    }

    fn ensure_active(&self) -> Result<(), Diagnostic> {
        if matches!(self.state, BuilderState::Active) {
            Ok(())
        } else {
            Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_finalized",
                "the query plan builder is terminal after successful finalization",
            ))
        }
    }

    fn ensure_owner(&self, owner: &HandleOwner) -> Result<(), Diagnostic> {
        if self.owner.authority != owner.authority {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_cross_authority_handle",
                "a query handle belongs to a different schema authority",
            ));
        }
        if self.owner.builder != owner.builder {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_builder_cross_builder_handle",
                "a query handle belongs to a different builder",
            ));
        }
        Ok(())
    }
}

enum AuthoredOutput {
    Rows(Vec<AuthoredBindingId>),
    Documents(Vec<(QueryVariable, AuthoredDocumentSource)>),
}

impl AuthoredOutput {
    fn to_contract(&self, map: &[Option<BindingId>]) -> Result<QueryOutput, Diagnostic> {
        match self {
            Self::Rows(columns) => Ok(QueryOutput::Rows {
                columns: columns
                    .iter()
                    .map(|binding| mapped_binding(map, *binding))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Self::Documents(fields) => Ok(QueryOutput::Documents {
                fields: fields
                    .iter()
                    .map(|(key, source)| {
                        let source = match source {
                            AuthoredDocumentSource::Binding(binding) => DocumentSource::Binding {
                                binding: mapped_binding(map, *binding)?,
                            },
                            AuthoredDocumentSource::AttributeList { attribute, owner } => {
                                DocumentSource::AttributeList {
                                    attribute: attribute.clone(),
                                    owner: mapped_binding(map, *owner)?,
                                }
                            }
                        };
                        Ok(DocumentField::new(key.clone(), source))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?,
            }),
        }
    }
}

impl AuthoredStage {
    fn to_contract(&self, map: &[Option<BindingId>]) -> Result<ReadStage, Diagnostic> {
        match self {
            Self::Match { patterns } => Ok(ReadStage::Match {
                patterns: patterns
                    .iter()
                    .map(|pattern| pattern.to_contract(map))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Self::Select { bindings } => Ok(ReadStage::Select {
                bindings: mapped_bindings(map, bindings)?,
            }),
            Self::Require { bindings } => Ok(ReadStage::Require {
                bindings: mapped_bindings(map, bindings)?,
            }),
            Self::Distinct => Ok(ReadStage::Distinct),
            Self::Reduce {
                assignments,
                groups,
            } => Ok(ReadStage::Reduce {
                assignments: assignments
                    .iter()
                    .map(|assignment| {
                        Ok(ReduceAssignment::new(
                            mapped_binding(map, assignment.assigned)?,
                            assignment.reducer,
                            assignment
                                .input
                                .map(|input| mapped_binding(map, input))
                                .transpose()?,
                        ))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?,
                groups: mapped_bindings(map, groups)?,
            }),
            Self::Sort { terms } => Ok(ReadStage::Sort {
                terms: terms
                    .iter()
                    .map(|(binding, direction)| {
                        Ok(OrderTerm::new(mapped_binding(map, *binding)?, *direction))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?,
            }),
            Self::Offset { rows } => Ok(ReadStage::Offset { rows: *rows }),
            Self::Limit { rows } => Ok(ReadStage::Limit { rows: *rows }),
        }
    }
}

impl AuthoredPattern {
    const fn is_flat(&self) -> bool {
        matches!(
            self,
            Self::Isa { .. } | Self::Has { .. } | Self::Links { .. } | Self::Value { .. }
        )
    }

    fn references_input(&self) -> bool {
        match self {
            Self::Value { left, right, .. } => {
                matches!(left, AuthoredOperand::Input(_))
                    || matches!(right, AuthoredOperand::Input(_))
            }
            Self::Or { branches } => branches
                .iter()
                .flatten()
                .any(|pattern| pattern.references_input()),
            Self::Not { patterns } | Self::Try { patterns } => {
                patterns.iter().any(|pattern| pattern.references_input())
            }
            Self::FunctionCall { arguments, .. } => arguments
                .iter()
                .any(|operand| matches!(operand, AuthoredOperand::Input(_))),
            Self::Isa { .. } | Self::Has { .. } | Self::Links { .. } | Self::Reachable { .. } => {
                false
            }
        }
    }

    fn collect_bindings(&self, bindings: &mut BTreeSet<AuthoredBindingId>) {
        match self {
            Self::Isa { binding, .. } => {
                bindings.insert(*binding);
            }
            Self::Has {
                attribute, owner, ..
            } => {
                bindings.insert(*attribute);
                bindings.insert(*owner);
            }
            Self::Links {
                players, relation, ..
            } => {
                bindings.insert(*relation);
                bindings.extend(players.iter().map(|(_, player)| *player));
            }
            Self::Value { left, right, .. } => {
                collect_operand_binding(left, bindings);
                collect_operand_binding(right, bindings);
            }
            Self::Or { branches } => {
                for pattern in branches.iter().flatten() {
                    pattern.collect_bindings(bindings);
                }
            }
            Self::Not { patterns } | Self::Try { patterns } => {
                for pattern in patterns {
                    pattern.collect_bindings(bindings);
                }
            }
            Self::Reachable { source, target, .. } => {
                bindings.insert(*source);
                bindings.insert(*target);
            }
            Self::FunctionCall {
                arguments,
                assigned,
                ..
            } => {
                bindings.insert(*assigned);
                for argument in arguments {
                    collect_operand_binding(argument, bindings);
                }
            }
        }
    }

    fn collect_direct_positive_bindings(&self, bindings: &mut BTreeSet<AuthoredBindingId>) {
        match self {
            Self::Isa { binding, .. } => {
                bindings.insert(*binding);
            }
            Self::Has {
                attribute, owner, ..
            } => {
                bindings.extend([*owner, *attribute]);
            }
            Self::Links {
                players, relation, ..
            } => {
                bindings.insert(*relation);
                bindings.extend(players.iter().map(|(_, player)| *player));
            }
            Self::Reachable { source, target, .. } => {
                bindings.extend([*source, *target]);
            }
            Self::FunctionCall { assigned, .. } => {
                bindings.insert(*assigned);
            }
            Self::Or { branches } => {
                let mut branches = branches.iter().map(|patterns| {
                    let mut branch = BTreeSet::new();
                    for pattern in patterns {
                        pattern.collect_direct_positive_bindings(&mut branch);
                    }
                    branch
                });
                if let Some(mut intersection) = branches.next() {
                    for branch in branches {
                        intersection.retain(|binding| branch.contains(binding));
                    }
                    bindings.extend(intersection);
                }
            }
            Self::Value { .. } | Self::Not { .. } | Self::Try { .. } => {}
        }
    }

    fn collect_negation_positive_bindings(&self, bindings: &mut BTreeSet<AuthoredBindingId>) {
        match self {
            Self::Not { patterns } => {
                for child in patterns {
                    child.collect_direct_positive_bindings(bindings);
                    child.collect_negation_positive_bindings(bindings);
                }
            }
            Self::Or { branches } => {
                for child in branches.iter().flatten() {
                    child.collect_negation_positive_bindings(bindings);
                }
            }
            Self::Isa { .. }
            | Self::Has { .. }
            | Self::Links { .. }
            | Self::Value { .. }
            | Self::Try { .. }
            | Self::Reachable { .. }
            | Self::FunctionCall { .. } => {}
        }
    }

    fn to_contract(&self, map: &[Option<BindingId>]) -> Result<QueryPattern, Diagnostic> {
        match self {
            Self::Isa {
                binding,
                include_subtypes,
                type_id,
            } => Ok(QueryPattern::Isa {
                binding: mapped_binding(map, *binding)?,
                include_subtypes: *include_subtypes,
                type_id: type_id.clone(),
            }),
            Self::Has {
                attribute,
                attribute_id,
                owner,
            } => Ok(QueryPattern::Has {
                attribute: mapped_binding(map, *attribute)?,
                attribute_id: attribute_id.clone(),
                owner: mapped_binding(map, *owner)?,
            }),
            Self::Links {
                players,
                relation,
                relation_id,
            } => Ok(QueryPattern::Links {
                players: players
                    .iter()
                    .map(|(role, player)| {
                        Ok(AssertionRolePlayer::new(
                            role.clone(),
                            mapped_binding(map, *player)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?,
                relation: mapped_binding(map, *relation)?,
                relation_id: relation_id.clone(),
            }),
            Self::Value {
                comparator,
                left,
                right,
            } => Ok(QueryPattern::Value {
                comparator: *comparator,
                left: left.to_contract(map)?,
                right: right.to_contract(map)?,
            }),
            Self::Or { branches } => Ok(QueryPattern::Or {
                branches: branches
                    .iter()
                    .map(|branch| {
                        branch
                            .iter()
                            .map(|pattern| pattern.to_contract(map))
                            .collect()
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?,
            }),
            Self::Not { patterns } => Ok(QueryPattern::Not {
                patterns: patterns
                    .iter()
                    .map(|pattern| pattern.to_contract(map))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Self::Try { patterns } => Ok(QueryPattern::Try {
                patterns: patterns
                    .iter()
                    .map(|pattern| pattern.to_contract(map))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Self::Reachable {
                min_depth,
                max_depth,
                relation,
                role_from,
                role_to,
                source,
                target,
            } => Ok(QueryPattern::Reachable {
                min_depth: *min_depth,
                max_depth: *max_depth,
                relation: relation.clone(),
                role_from: role_from.clone(),
                role_to: role_to.clone(),
                source: mapped_binding(map, *source)?,
                target: mapped_binding(map, *target)?,
            }),
            Self::FunctionCall {
                arguments,
                assigned,
                function,
            } => Ok(QueryPattern::FunctionCall {
                arguments: arguments
                    .iter()
                    .map(|argument| argument.to_contract(map))
                    .collect::<Result<Vec<_>, _>>()?,
                assigned: mapped_binding(map, *assigned)?,
                function: function.clone(),
            }),
        }
    }
}

fn collect_operand_binding(operand: &AuthoredOperand, bindings: &mut BTreeSet<AuthoredBindingId>) {
    if let AuthoredOperand::Binding(binding) = operand {
        bindings.insert(*binding);
    }
}

impl AuthoredOperand {
    fn to_contract(&self, map: &[Option<BindingId>]) -> Result<QueryOperand, Diagnostic> {
        match self {
            Self::Binding(binding) => Ok(QueryOperand::Binding {
                binding: mapped_binding(map, *binding)?,
            }),
            Self::Literal(value) => Ok(QueryOperand::Literal {
                value: value.clone(),
            }),
            Self::Input(column) => Ok(QueryOperand::Input { column: *column }),
        }
    }
}

fn validate_root_patterns(patterns: &[Arc<AuthoredPattern>]) -> Result<usize, Diagnostic> {
    if patterns.is_empty() || patterns.len() > MAX_BOOLEAN_TERMS {
        return Err(query_builder_root_pattern_limit_error());
    }
    let mut nodes = 0usize;
    for pattern in patterns {
        inspect_authored_pattern(pattern, 1, &mut nodes)?;
    }
    Ok(nodes)
}

fn inspect_authored_pattern(
    pattern: &AuthoredPattern,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_PREDICATE_NODES {
        return Err(builder_failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_pattern_node_limit",
            "plan pattern count exceeds the structural ceiling",
        ));
    }
    if depth > MAX_PREDICATE_DEPTH {
        return Err(builder_failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_pattern_depth_limit",
            "plan pattern depth exceeds the structural ceiling",
        ));
    }
    match pattern {
        AuthoredPattern::Or { branches } => {
            for branch in branches {
                for child in branch {
                    inspect_authored_pattern(child, depth + 1, nodes)?;
                }
            }
        }
        AuthoredPattern::Not { patterns } => {
            for child in patterns {
                inspect_authored_pattern(child, depth + 1, nodes)?;
            }
        }
        AuthoredPattern::Try { patterns } => {
            if depth > 1 {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_try_not_root",
                    "optional blocks are admitted only in the root conjunction",
                ));
            }
            for child in patterns {
                inspect_authored_pattern(child, depth + 1, nodes)?;
            }
        }
        AuthoredPattern::Reachable {
            min_depth,
            max_depth,
            ..
        } => {
            if depth > 1 {
                return Err(builder_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_reachable_not_root",
                    "bounded reachability is admitted only in the root conjunction",
                ));
            }
            *nodes = nodes.saturating_add(reachability_nodes(*min_depth, *max_depth) - 1);
            if *nodes > MAX_PREDICATE_NODES {
                return Err(builder_failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_reachable_expansion_limit",
                    "reachability expansion exceeds the plan pattern-node ceiling",
                ));
            }
        }
        AuthoredPattern::FunctionCall { .. } if depth > 1 => {
            return Err(builder_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_function_in_negation",
                "function calls are admitted only in the root conjunction",
            ));
        }
        AuthoredPattern::Isa { .. }
        | AuthoredPattern::Has { .. }
        | AuthoredPattern::Links { .. }
        | AuthoredPattern::Value { .. }
        | AuthoredPattern::FunctionCall { .. } => {}
    }
    Ok(())
}

fn reachability_nodes(min_depth: u8, max_depth: u8) -> usize {
    let first_positive = usize::from(min_depth.max(1));
    let bound = usize::from(max_depth);
    let positive = if first_positive <= bound {
        (first_positive..=bound).fold(0usize, usize::saturating_add)
    } else {
        0
    };
    positive.saturating_add(usize::from(min_depth == 0))
}

fn pattern_bindings(patterns: &[Arc<AuthoredPattern>]) -> BTreeSet<AuthoredBindingId> {
    let mut bindings = BTreeSet::new();
    for pattern in patterns {
        pattern.collect_bindings(&mut bindings);
    }
    bindings
}

fn validate_components(
    components: QueryPlanComponents,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<ValidatedQuery, Diagnostic> {
    let plan = QueryPlan::new_v2_with_functions(
        components.bindings,
        components.functions,
        components.inputs,
        components.pipeline,
        components.output,
        components.compatibility,
        context.managed_state().managed_semantic_schema().clone(),
    )?;
    validate_query_plan(&plan, context, limits)
}

fn dense_binding_map(
    order: &[AuthoredBindingId],
    authored_count: usize,
) -> Result<Vec<Option<BindingId>>, Diagnostic> {
    let mut map = vec![None; authored_count];
    for (dense, authored) in order.iter().enumerate() {
        let dense = u16::try_from(dense).map_err(|_| {
            builder_failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_binding_limit",
                "binding count exceeds the canonical structural ceiling",
            )
        })?;
        map[authored.0] = Some(BindingId::new(dense)?);
    }
    Ok(map)
}

fn mapped_binding(
    map: &[Option<BindingId>],
    authored: AuthoredBindingId,
) -> Result<BindingId, Diagnostic> {
    map.get(authored.0).copied().flatten().ok_or_else(|| {
        builder_failure(
            DiagnosticCategory::Integrity,
            "query_builder_unmapped_binding",
            "a scoped authored binding has no canonical dense identity",
        )
    })
}

fn mapped_bindings(
    map: &[Option<BindingId>],
    authored: &[AuthoredBindingId],
) -> Result<Vec<BindingId>, Diagnostic> {
    authored
        .iter()
        .map(|binding| mapped_binding(map, *binding))
        .collect()
}

fn builder_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static builder diagnostic code is canonical"),
        message,
    )
}

fn stage_order_failure() -> Diagnostic {
    builder_failure(
        DiagnosticCategory::InvalidContract,
        "query_plan_stage_order",
        "pipeline stages must follow the canonical order exactly once each",
    )
}

fn scope_failure(
    code: &'static str,
    message: &'static str,
    variable: &QueryVariable,
) -> Diagnostic {
    builder_failure(DiagnosticCategory::InvalidContract, code, message).with_detail(
        "binding",
        DiagnosticDetailValue::Text(variable.as_str().to_owned()),
    )
}

// Every closed family in the Phase-0 low-level ledger is exhaustively matched
// in a compiled test. A contract enum addition therefore breaks all-target
// compilation until the builder inventory and both future wrappers are
// deliberately reconciled.
#[cfg(test)]
mod completeness_guard {
    use super::*;
    use type_bridge_contract::query_plan::{
        ModelQueryV2, QueryModelOutputSlotV2, QueryModelOutputV2, QueryRowCardinalityV2,
    };

    #[test]
    fn every_closed_contract_family_is_exhaustive() {
        fn operand(value: &QueryOperand) {
            match value {
                QueryOperand::Binding { .. }
                | QueryOperand::Literal { .. }
                | QueryOperand::Input { .. } => {}
            }
        }
        fn pattern(value: &QueryPattern) {
            match value {
                QueryPattern::Isa { .. }
                | QueryPattern::Has { .. }
                | QueryPattern::Links { .. }
                | QueryPattern::Value { .. }
                | QueryPattern::Or { .. }
                | QueryPattern::Not { .. }
                | QueryPattern::Try { .. }
                | QueryPattern::Reachable { .. }
                | QueryPattern::FunctionCall { .. } => {}
            }
        }
        fn stage(value: &ReadStage) {
            match value {
                ReadStage::Match { .. }
                | ReadStage::Select { .. }
                | ReadStage::Require { .. }
                | ReadStage::Distinct
                | ReadStage::Reduce { .. }
                | ReadStage::Sort { .. }
                | ReadStage::Offset { .. }
                | ReadStage::Limit { .. } => {}
            }
        }
        fn output(value: &QueryOutput) {
            match value {
                QueryOutput::Rows { .. } | QueryOutput::Documents { .. } => {}
            }
        }
        fn document_source(value: &DocumentSource) {
            match value {
                DocumentSource::Binding { .. } | DocumentSource::AttributeList { .. } => {}
            }
        }
        fn operation(value: QueryOperation) {
            match value {
                QueryOperation::Rows | QueryOperation::Count | QueryOperation::Exists => {}
            }
        }
        fn reducer(value: Reducer) {
            match value {
                Reducer::Count | Reducer::Max | Reducer::Mean | Reducer::Min | Reducer::Sum => {}
            }
        }
        fn direction(value: OrderDirection) {
            match value {
                OrderDirection::Ascending | OrderDirection::Descending => {}
            }
        }
        fn comparator(value: ValueComparator) {
            match value {
                ValueComparator::Equal
                | ValueComparator::NotEqual
                | ValueComparator::Less
                | ValueComparator::LessOrEqual
                | ValueComparator::Greater
                | ValueComparator::GreaterOrEqual => {}
            }
        }
        fn value_type(value: ValueTypeTag) {
            match value {
                ValueTypeTag::String
                | ValueTypeTag::Long
                | ValueTypeTag::Double
                | ValueTypeTag::Boolean
                | ValueTypeTag::Date
                | ValueTypeTag::DateTime
                | ValueTypeTag::DateTimeTz
                | ValueTypeTag::Decimal
                | ValueTypeTag::Duration => {}
            }
        }
        fn canonical_value(value: &CanonicalValue) {
            match value {
                CanonicalValue::String(_)
                | CanonicalValue::Long(_)
                | CanonicalValue::Double(_)
                | CanonicalValue::Boolean(_)
                | CanonicalValue::Date(_)
                | CanonicalValue::DateTime(_)
                | CanonicalValue::DateTimeTz(_)
                | CanonicalValue::Decimal(_)
                | CanonicalValue::Duration(_) => {}
            }
        }
        fn row_cardinality(value: QueryRowCardinalityV2) {
            match value {
                QueryRowCardinalityV2::ExactlyOne | QueryRowCardinalityV2::BoundedMany => {}
            }
        }
        fn model_slot(value: &QueryModelOutputSlotV2) {
            match value {
                QueryModelOutputSlotV2::One { .. }
                | QueryModelOutputSlotV2::Collect { distinct: true, .. }
                | QueryModelOutputSlotV2::Collect {
                    distinct: false, ..
                } => {}
            }
        }
        fn model_output(value: &QueryModelOutputV2) {
            match value {
                QueryModelOutputV2::Positional { .. } | QueryModelOutputV2::Named { .. } => {}
            }
        }
        fn model_query(value: &ModelQueryV2) {
            match value {
                ModelQueryV2::Rows { .. }
                | ModelQueryV2::Page {
                    include_total: true,
                    ..
                }
                | ModelQueryV2::Page {
                    include_total: false,
                    ..
                }
                | ModelQueryV2::DistinctCount { .. }
                | ModelQueryV2::DistinctExists { .. } => {}
            }
        }
        fn input_optionality(value: &InputColumn) {
            match value.optional() {
                true | false => {}
            }
        }
        fn local_form(value: &LocalFunction) {
            let _ = (
                value.name(),
                value.bindings(),
                value.parameters(),
                value.body(),
                value.returns(),
            );
        }
        fn local_return(value: &LocalReturn) {
            reducer(value.reducer());
            value_type(value.value_type());
            let _ = value.input();
        }

        let guards = (
            operand as fn(&QueryOperand),
            pattern as fn(&QueryPattern),
            stage as fn(&ReadStage),
            output as fn(&QueryOutput),
            document_source as fn(&DocumentSource),
            operation as fn(QueryOperation),
            reducer as fn(Reducer),
            direction as fn(OrderDirection),
            comparator as fn(ValueComparator),
            value_type as fn(ValueTypeTag),
            canonical_value as fn(&CanonicalValue),
            row_cardinality as fn(QueryRowCardinalityV2),
            model_slot as fn(&QueryModelOutputSlotV2),
            model_output as fn(&QueryModelOutputV2),
            model_query as fn(&ModelQueryV2),
            input_optionality as fn(&InputColumn),
            local_form as fn(&LocalFunction),
            local_return as fn(&LocalReturn),
        );
        std::hint::black_box(guards);
    }
}
