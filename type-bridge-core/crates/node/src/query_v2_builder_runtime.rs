//! Opaque N-API projection of the shared V2 plan-authoring state machine.
//!
//! JavaScript values are reduced to checked primitives and opaque native
//! references here. Rust owns every transition, semantic check, canonical
//! byte, fingerprint, and capability.

use napi::bindgen_prelude::{
    BigInt, Buffer, Env, FromNapiValue, Reference, TypeName, Unknown, ValidateNapiValue,
};
use napi::{JsObject, ValueType};
use napi_derive::napi;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{
    AttributeId, FunctionId, MAX_LABEL_BYTES, RoleId, TypeId, TypeKind,
};
use type_bridge_contract::limits::{
    MAX_BINDINGS, MAX_BOOLEAN_TERMS, MAX_CANONICAL_STRING_BYTES, MAX_INPUT_BYTES, MAX_INPUT_ROWS,
    MAX_ORDER_TERMS, MAX_OUTPUT_NAME_BYTES, MAX_SELECTED_SLOTS,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::query_v2_builder::{
    AuthoredQueryInvocation, AuthoredQueryPlan, QueryAuthorityIdentity, QueryBindingHandle,
    QueryBuilderScalarInput, QueryDocumentFieldHandle, QueryInputHandle, QueryLocalFunctionHandle,
    QueryLocalReturnHandle, QueryOperandHandle, QueryOrderHandle, QueryPatternHandle,
    QueryPlanBuilder, QueryReduceAssignmentHandle, query_builder_binding_limit_error,
    query_builder_boolean_host_type_error, query_builder_comparator, query_builder_depth,
    query_builder_depth_error, query_builder_disjunction_term_limit_error,
    query_builder_document_output_limit_error, query_builder_function_argument_limit_error,
    query_builder_function_target, query_builder_host_collection_type_error,
    query_builder_invocation_input_byte_limit_error, query_builder_invocation_row_arity_error,
    query_builder_invocation_row_limit_error, query_builder_local_binding_limit_error,
    query_builder_local_body_limit_error, query_builder_local_parameters,
    query_builder_negation_term_limit_error, query_builder_order_direction,
    query_builder_reduce_term_limit_error, query_builder_reducer,
    query_builder_role_player_limit_error, query_builder_role_players,
    query_builder_root_pattern_limit_error, query_builder_row_output_limit_error,
    query_builder_scalar, query_builder_scalar_host_type_error,
    query_builder_scalar_integer_range_error, query_builder_scalar_unicode_error,
    query_builder_sort_term_limit_error, query_builder_try_term_limit_error,
    query_builder_type_kind, query_builder_unsigned_error, query_builder_value_type,
};
use type_bridge_orm::query_v2_prepared::{
    query_v2_host_string_type_error, query_v2_host_string_unicode_error,
};

use crate::query_v2_runtime::{
    BoundedNodeString, NodeQueryV2Authority, bounded_node_string_snapshot,
    bounded_string_with_unicode_error, napi_error,
};

fn diagnostic<T>(result: Result<T, Diagnostic>) -> napi::Result<T> {
    result.map_err(|diagnostic| napi_error(&diagnostic))
}

unsafe fn node_host_string(
    env: napi::sys::napi_env,
    napi_value: napi::sys::napi_value,
    limit: usize,
) -> napi::Result<String> {
    if napi::type_of!(env, napi_value)? != ValueType::String {
        return Err(napi_error(&query_v2_host_string_type_error()));
    }

    match unsafe { bounded_node_string_snapshot(env, napi_value, limit) }? {
        BoundedNodeString::Value(value) => Ok(value),
        BoundedNodeString::Oversized => Ok(" ".repeat(limit.saturating_add(1))),
        BoundedNodeString::InvalidUnicode => Err(napi_error(&query_v2_host_string_unicode_error())),
    }
}

macro_rules! define_node_host_string {
    ($name:ident, $limit:expr) => {
        pub struct $name(String);

        #[allow(dead_code)]
        impl $name {
            fn into_inner(self) -> String {
                self.0
            }

            fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TypeName for $name {
            fn type_name() -> &'static str {
                "String"
            }

            fn value_type() -> ValueType {
                ValueType::Unknown
            }
        }

        impl ValidateNapiValue for $name {}

        impl FromNapiValue for $name {
            unsafe fn from_napi_value(
                env: napi::sys::napi_env,
                napi_value: napi::sys::napi_value,
            ) -> napi::Result<Self> {
                unsafe { node_host_string(env, napi_value, $limit) }.map(Self)
            }
        }
    };
}

define_node_host_string!(NodeVariableString, MAX_OUTPUT_NAME_BYTES);
define_node_host_string!(NodeLabelString, MAX_LABEL_BYTES);
define_node_host_string!(NodeVocabularyString, MAX_OUTPUT_NAME_BYTES);

fn oversized_scalar(value_type: ValueTypeTag) -> Diagnostic {
    query_builder_scalar(
        value_type,
        QueryBuilderScalarInput::Text(" ".repeat(MAX_CANONICAL_STRING_BYTES.saturating_add(1))),
    )
    .expect_err("oversized text marker cannot be canonical")
}

fn node_scalar(
    env: &Env,
    value_type: ValueTypeTag,
    value: Unknown,
) -> napi::Result<CanonicalValue> {
    let input = match value_type {
        ValueTypeTag::String
        | ValueTypeTag::Date
        | ValueTypeTag::DateTime
        | ValueTypeTag::DateTimeTz
        | ValueTypeTag::Decimal
        | ValueTypeTag::Duration => {
            if value.get_type()? != ValueType::String {
                return Err(napi_error(&query_builder_scalar_host_type_error()));
            }
            QueryBuilderScalarInput::Text(bounded_string_with_unicode_error(
                env,
                value,
                MAX_CANONICAL_STRING_BYTES,
                || oversized_scalar(value_type),
                query_builder_scalar_unicode_error,
            )?)
        }
        ValueTypeTag::Long => {
            let integer = BigInt::from_unknown(value)
                .map_err(|_| napi_error(&query_builder_scalar_host_type_error()))?;
            let (integer, lossless) = integer.get_i64();
            if !lossless {
                return Err(napi_error(&query_builder_scalar_integer_range_error()));
            }
            QueryBuilderScalarInput::Long(integer)
        }
        ValueTypeTag::Double => QueryBuilderScalarInput::Double(
            f64::from_unknown(value)
                .map_err(|_| napi_error(&query_builder_scalar_host_type_error()))?,
        ),
        ValueTypeTag::Boolean => QueryBuilderScalarInput::Boolean(
            bool::from_unknown(value)
                .map_err(|_| napi_error(&query_builder_scalar_host_type_error()))?,
        ),
    };
    diagnostic(query_builder_scalar(value_type, input))
}

fn node_boolean(value: Unknown) -> napi::Result<bool> {
    bool::from_unknown(value).map_err(|_| napi_error(&query_builder_boolean_host_type_error()))
}

fn node_depth(value: Unknown) -> napi::Result<u8> {
    let value = f64::from_unknown(value).map_err(|_| napi_error(&query_builder_depth_error()))?;
    if !value.is_finite()
        || value.fract() != 0.0
        || value < f64::from(u8::MIN)
        || value > f64::from(u8::MAX)
    {
        return Err(napi_error(&query_builder_depth_error()));
    }
    diagnostic(query_builder_depth(value as i128))
}

fn node_u64(value: Unknown) -> napi::Result<u64> {
    let value =
        BigInt::from_unknown(value).map_err(|_| napi_error(&query_builder_unsigned_error()))?;
    let (negative, value, lossless) = value.get_u64();
    if negative || !lossless {
        return Err(napi_error(&query_builder_unsigned_error()));
    }
    Ok(value)
}

fn bounded_array_shape(
    value: Unknown,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> napi::Result<(JsObject, u32)> {
    if !value.is_array()? {
        return Err(napi_error(&query_builder_host_collection_type_error()));
    }
    let array: JsObject = value.coerce_to_object()?;
    let length = array.get_array_length_unchecked()?;
    if usize::try_from(length).unwrap_or(usize::MAX) > limit {
        return Err(napi_error(&oversized()));
    }
    Ok((array, length))
}

fn array_elements(array: &JsObject, length: u32) -> napi::Result<Vec<Unknown>> {
    (0..length)
        .map(|index| array.get_element::<Unknown>(index))
        .collect()
}

fn bounded_array(
    value: Unknown,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> napi::Result<Vec<Unknown>> {
    let (array, length) = bounded_array_shape(value, limit, oversized)?;
    array_elements(&array, length)
}

fn binding_handles(
    value: Unknown,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> napi::Result<Vec<QueryBindingHandle>> {
    bounded_array(value, limit, oversized)?
        .into_iter()
        .map(|handle| {
            Reference::<NodeQueryV2BindingHandle>::from_unknown(handle)
                .map(|handle| handle.inner.clone())
        })
        .collect()
}

fn operand_handles(
    value: Unknown,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> napi::Result<Vec<QueryOperandHandle>> {
    bounded_array(value, limit, oversized)?
        .into_iter()
        .map(|handle| {
            Reference::<NodeQueryV2OperandHandle>::from_unknown(handle)
                .map(|handle| handle.inner.clone())
        })
        .collect()
}

fn pattern_handles(
    value: Unknown,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> napi::Result<Vec<QueryPatternHandle>> {
    bounded_array(value, limit, oversized)?
        .into_iter()
        .map(|handle| {
            Reference::<NodeQueryV2PatternHandle>::from_unknown(handle)
                .map(|handle| handle.inner.clone())
        })
        .collect()
}

fn pattern_branches(branches: Unknown) -> napi::Result<Vec<Vec<QueryPatternHandle>>> {
    bounded_array(
        branches,
        MAX_BOOLEAN_TERMS,
        query_builder_disjunction_term_limit_error,
    )?
    .into_iter()
    .map(|branch| {
        pattern_handles(
            branch,
            MAX_BOOLEAN_TERMS,
            query_builder_disjunction_term_limit_error,
        )
    })
    .collect()
}

fn invocation_rows(
    env: &Env,
    plan: &AuthoredQueryPlan,
    rows: Unknown,
) -> napi::Result<Vec<Vec<Option<CanonicalValue>>>> {
    let columns = plan.input_columns();
    let (rows, row_count) = bounded_array_shape(
        rows,
        MAX_INPUT_ROWS,
        query_builder_invocation_row_limit_error,
    )?;
    if columns.is_empty() {
        return Ok(vec![
            Vec::new();
            usize::try_from(row_count).unwrap_or(usize::MAX)
        ]);
    }
    let mut converted = Vec::with_capacity(usize::try_from(row_count).unwrap_or(usize::MAX));
    let mut input_bytes = 2usize;
    for row_index in 0..row_count {
        let row = rows.get_element::<Unknown>(row_index)?;
        let (row, value_count) =
            bounded_array_shape(row, MAX_BINDINGS, query_builder_invocation_row_arity_error)?;
        if usize::try_from(value_count).unwrap_or(usize::MAX) != columns.len() {
            return Err(napi_error(&query_builder_invocation_row_arity_error()));
        }
        let values = array_elements(&row, value_count)?;
        input_bytes = input_bytes
            .saturating_add(usize::from(row_index > 0))
            .saturating_add(2);
        if input_bytes > MAX_INPUT_BYTES {
            return Err(napi_error(
                &query_builder_invocation_input_byte_limit_error(),
            ));
        }
        let mut converted_row = Vec::with_capacity(values.len().min(columns.len()));
        for (column_index, (value, column)) in values.into_iter().zip(columns).enumerate() {
            let value = if matches!(value.get_type()?, ValueType::Null) {
                None
            } else {
                Some(node_scalar(env, column.value_type(), value)?)
            };
            let encoded = serde_json::to_vec(&value)
                .map_err(|_| napi_error(&query_builder_invocation_input_byte_limit_error()))?;
            input_bytes = input_bytes
                .saturating_add(usize::from(column_index > 0))
                .saturating_add(encoded.len());
            if input_bytes > MAX_INPUT_BYTES {
                return Err(napi_error(
                    &query_builder_invocation_input_byte_limit_error(),
                ));
            }
            converted_row.push(value);
        }
        converted.push(converted_row);
    }
    Ok(converted)
}

/// Opaque authority identity carried by plans and invocations.
#[napi]
pub struct NodeQueryV2AuthorityIdentity {
    inner: QueryAuthorityIdentity,
}

#[napi]
impl NodeQueryV2AuthorityIdentity {
    /// Compare opaque authority allocation identity.
    #[napi(js_name = "sameAuthority")]
    pub fn same_authority(&self, other: &NodeQueryV2AuthorityIdentity) -> bool {
        self.inner == other.inner
    }
}

/// Opaque builder-owned binding handle.
#[napi]
pub struct NodeQueryV2BindingHandle {
    inner: QueryBindingHandle,
}

/// Opaque builder-owned input-column handle.
#[napi]
pub struct NodeQueryV2InputHandle {
    inner: QueryInputHandle,
}

/// Opaque builder-owned operand handle.
#[napi]
pub struct NodeQueryV2OperandHandle {
    inner: QueryOperandHandle,
}

/// Opaque builder-owned pattern handle.
#[napi]
pub struct NodeQueryV2PatternHandle {
    inner: QueryPatternHandle,
}

/// Opaque builder-owned order-term handle.
#[napi]
pub struct NodeQueryV2OrderHandle {
    inner: QueryOrderHandle,
}

/// Opaque builder-owned reducer-assignment handle.
#[napi]
pub struct NodeQueryV2ReduceAssignmentHandle {
    inner: QueryReduceAssignmentHandle,
}

/// Opaque builder-owned local-return handle.
#[napi]
pub struct NodeQueryV2LocalReturnHandle {
    inner: QueryLocalReturnHandle,
}

/// Opaque builder-owned local-function handle.
#[napi]
pub struct NodeQueryV2LocalFunctionHandle {
    inner: QueryLocalFunctionHandle,
}

/// Opaque builder-owned document-field handle.
#[napi]
pub struct NodeQueryV2DocumentFieldHandle {
    inner: QueryDocumentFieldHandle,
}

/// Immutable plan-bound typed invocation.
#[napi]
pub struct NodeAuthoredQueryInvocation {
    inner: AuthoredQueryInvocation,
}

#[napi]
impl NodeAuthoredQueryInvocation {
    /// Return an immutable copy of canonical invocation bytes.
    #[napi(getter, js_name = "canonicalBytes")]
    pub fn canonical_bytes(&self) -> Buffer {
        self.inner.canonical_bytes().into()
    }

    /// Return the stable operation spelling.
    #[napi(getter)]
    pub fn operation(&self) -> &'static str {
        self.inner.operation_name()
    }

    /// Return the exact bound plan fingerprint.
    #[napi(getter, js_name = "planFingerprint")]
    pub fn plan_fingerprint(&self) -> String {
        self.inner.plan_fingerprint_hex()
    }

    /// Return the opaque authority token.
    #[napi(getter, js_name = "authorityIdentity")]
    pub fn authority_identity(&self) -> NodeQueryV2AuthorityIdentity {
        NodeQueryV2AuthorityIdentity {
            inner: self.inner.authority_identity().clone(),
        }
    }

    /// Return invocation-derived transport capabilities.
    #[napi(getter, js_name = "requiredTransportCapabilities")]
    pub fn required_transport_capabilities(&self) -> Vec<String> {
        self.inner.required_transport_capabilities().to_vec()
    }
}

/// Immutable finalized V2 query plan.
#[napi]
pub struct NodeAuthoredQueryPlan {
    inner: AuthoredQueryPlan,
}

#[napi]
impl NodeAuthoredQueryPlan {
    /// Return an immutable copy of canonical plan bytes.
    #[napi(getter, js_name = "canonicalBytes")]
    pub fn canonical_bytes(&self) -> Buffer {
        self.inner.canonical_bytes().into()
    }

    /// Return the exact V2 format discriminator.
    #[napi(getter)]
    pub fn format(&self) -> &str {
        self.inner.format()
    }

    /// Return the lower-hex plan fingerprint.
    #[napi(getter)]
    pub fn fingerprint(&self) -> &str {
        self.inner.fingerprint_hex()
    }

    /// Return lexically sorted required capability identifiers.
    #[napi(getter, js_name = "requiredCapabilities")]
    pub fn required_capabilities(&self) -> Vec<String> {
        self.inner.required_capabilities().to_vec()
    }

    /// Return the opaque authority token.
    #[napi(getter, js_name = "authorityIdentity")]
    pub fn authority_identity(&self) -> NodeQueryV2AuthorityIdentity {
        NodeQueryV2AuthorityIdentity {
            inner: self.inner.authority_identity().clone(),
        }
    }

    /// Create a row-output invocation.
    #[napi]
    pub fn rows(&self, env: Env, rows: Unknown) -> napi::Result<NodeAuthoredQueryInvocation> {
        let rows = invocation_rows(&env, &self.inner, rows)?;
        diagnostic(self.inner.rows(rows)).map(|inner| NodeAuthoredQueryInvocation { inner })
    }

    /// Create a document-output invocation.
    #[napi]
    pub fn documents(&self, env: Env, rows: Unknown) -> napi::Result<NodeAuthoredQueryInvocation> {
        let rows = invocation_rows(&env, &self.inner, rows)?;
        diagnostic(self.inner.documents(rows)).map(|inner| NodeAuthoredQueryInvocation { inner })
    }

    /// Create a count invocation.
    #[napi]
    pub fn count(&self, env: Env, rows: Unknown) -> napi::Result<NodeAuthoredQueryInvocation> {
        let rows = invocation_rows(&env, &self.inner, rows)?;
        diagnostic(self.inner.count(rows)).map(|inner| NodeAuthoredQueryInvocation { inner })
    }

    /// Create an existence invocation.
    #[napi]
    pub fn exists(&self, env: Env, rows: Unknown) -> napi::Result<NodeAuthoredQueryInvocation> {
        let rows = invocation_rows(&env, &self.inner, rows)?;
        diagnostic(self.inner.exists(rows)).map(|inner| NodeAuthoredQueryInvocation { inner })
    }
}

/// The only mutable native handle for low-level V2 plan authoring.
#[napi]
pub struct NodeQueryPlanBuilder {
    inner: QueryPlanBuilder,
}

#[napi]
impl NodeQueryPlanBuilder {
    /// Start one builder under an exact prepared authority.
    #[napi(constructor)]
    pub fn new(authority: &NodeQueryV2Authority) -> Self {
        Self {
            inner: QueryPlanBuilder::new(authority.authority()),
        }
    }

    /// Declare one binding.
    #[napi]
    pub fn binding(
        &mut self,
        variable: NodeVariableString,
    ) -> napi::Result<NodeQueryV2BindingHandle> {
        diagnostic(self.inner.binding(variable.into_inner()))
            .map(|inner| NodeQueryV2BindingHandle { inner })
    }

    /// Declare one typed invocation input.
    #[napi]
    pub fn input(
        &mut self,
        public_name: NodeVariableString,
        value_type: NodeVocabularyString,
        optional: Unknown,
    ) -> napi::Result<NodeQueryV2InputHandle> {
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        let optional = node_boolean(optional)?;
        diagnostic(
            self.inner
                .input(public_name.into_inner(), value_type, optional),
        )
        .map(|inner| NodeQueryV2InputHandle { inner })
    }

    /// Create a binding operand.
    #[napi(js_name = "bindingOperand")]
    pub fn binding_operand(
        &self,
        binding: &NodeQueryV2BindingHandle,
    ) -> napi::Result<NodeQueryV2OperandHandle> {
        diagnostic(self.inner.binding_operand(&binding.inner))
            .map(|inner| NodeQueryV2OperandHandle { inner })
    }

    /// Create a canonical literal operand.
    #[napi(js_name = "literalOperand")]
    pub fn literal_operand(
        &self,
        env: Env,
        value_type: NodeVocabularyString,
        value: Unknown,
    ) -> napi::Result<NodeQueryV2OperandHandle> {
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        let value = node_scalar(&env, value_type, value)?;
        diagnostic(self.inner.literal_operand(value))
            .map(|inner| NodeQueryV2OperandHandle { inner })
    }

    /// Create an input operand.
    #[napi(js_name = "inputOperand")]
    pub fn input_operand(
        &self,
        input: &NodeQueryV2InputHandle,
    ) -> napi::Result<NodeQueryV2OperandHandle> {
        diagnostic(self.inner.input_operand(&input.inner))
            .map(|inner| NodeQueryV2OperandHandle { inner })
    }

    /// Create an exact/subtype type pattern.
    #[napi]
    pub fn isa(
        &self,
        binding: &NodeQueryV2BindingHandle,
        type_kind: NodeVocabularyString,
        type_label: NodeLabelString,
        include_subtypes: Unknown,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let kind = diagnostic(query_builder_type_kind(type_kind.as_str()))?;
        let type_id = diagnostic(TypeId::new(kind, type_label.into_inner()))?;
        let include_subtypes = node_boolean(include_subtypes)?;
        diagnostic(self.inner.isa(&binding.inner, type_id, include_subtypes))
            .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create an ownership pattern.
    #[napi]
    pub fn has(
        &self,
        owner: &NodeQueryV2BindingHandle,
        attribute: &NodeQueryV2BindingHandle,
        attribute_label: NodeLabelString,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let attribute_id = diagnostic(AttributeId::new(attribute_label.into_inner()))?;
        diagnostic(self.inner.has(&owner.inner, &attribute.inner, attribute_id))
            .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a relation role-player pattern.
    #[napi]
    pub fn links(
        &self,
        relation: &NodeQueryV2BindingHandle,
        relation_label: NodeLabelString,
        roles: Unknown,
        players: Unknown,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let relation_label = relation_label.into_inner();
        let relation_id = diagnostic(TypeId::new(TypeKind::Relation, relation_label.clone()))?;
        let roles = bounded_array(
            roles,
            MAX_BOOLEAN_TERMS,
            query_builder_role_player_limit_error,
        )?
        .into_iter()
        .map(NodeLabelString::from_unknown)
        .map(|label| label.map(NodeLabelString::into_inner))
        .collect::<napi::Result<Vec<_>>>()?;
        let players = diagnostic(query_builder_role_players(
            &relation_label,
            roles,
            binding_handles(
                players,
                MAX_BOOLEAN_TERMS,
                query_builder_role_player_limit_error,
            )?,
        ))?;
        diagnostic(self.inner.links(&relation.inner, relation_id, players))
            .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a typed scalar comparison.
    #[napi]
    pub fn value(
        &self,
        comparator: NodeVocabularyString,
        left: &NodeQueryV2OperandHandle,
        right: &NodeQueryV2OperandHandle,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let comparator = diagnostic(query_builder_comparator(comparator.as_str()))?;
        diagnostic(self.inner.value(comparator, &left.inner, &right.inner))
            .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a nested negated conjunction.
    #[napi(js_name = "not")]
    pub fn not_pattern(&self, patterns: Unknown) -> napi::Result<NodeQueryV2PatternHandle> {
        diagnostic(self.inner.not(pattern_handles(
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_negation_term_limit_error,
        )?))
        .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a disjunction.
    #[napi(js_name = "or")]
    pub fn or_pattern(&self, branches: Unknown) -> napi::Result<NodeQueryV2PatternHandle> {
        diagnostic(self.inner.or(pattern_branches(branches)?))
            .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a root-only optional conjunction.
    #[napi(js_name = "try")]
    pub fn try_pattern(&self, patterns: Unknown) -> napi::Result<NodeQueryV2PatternHandle> {
        diagnostic(self.inner.r#try(pattern_handles(
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_try_term_limit_error,
        )?))
        .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a finite directed reachability predicate.
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn reachable(
        &self,
        source: &NodeQueryV2BindingHandle,
        target: &NodeQueryV2BindingHandle,
        relation_label: NodeLabelString,
        role_from: NodeLabelString,
        role_to: NodeLabelString,
        min_depth: Unknown,
        max_depth: Unknown,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let min_depth = node_depth(min_depth)?;
        let max_depth = node_depth(max_depth)?;
        let relation_label = relation_label.into_inner();
        let relation = diagnostic(TypeId::new(TypeKind::Relation, relation_label.clone()))?;
        let role_from = diagnostic(RoleId::new(relation_label.clone(), role_from.into_inner()))?;
        let role_to = diagnostic(RoleId::new(relation_label, role_to.into_inner()))?;
        diagnostic(self.inner.reachable(
            &source.inner,
            &target.inner,
            relation,
            role_from,
            role_to,
            min_depth,
            max_depth,
        ))
        .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create a scalar schema/local function assignment.
    #[napi(js_name = "functionCall")]
    pub fn function_call(
        &self,
        assigned: &NodeQueryV2BindingHandle,
        arguments: Unknown,
        function_name: Option<NodeLabelString>,
        local_function: Option<Reference<NodeQueryV2LocalFunctionHandle>>,
    ) -> napi::Result<NodeQueryV2PatternHandle> {
        let function_name = function_name
            .map(NodeLabelString::into_inner)
            .map(FunctionId::new)
            .transpose()
            .map_err(|diagnostic| napi_error(&diagnostic))?;
        let target = diagnostic(query_builder_function_target(
            function_name,
            local_function.as_ref().map(|function| &function.inner),
        ))?;
        diagnostic(self.inner.function_call(
            &assigned.inner,
            target,
            operand_handles(
                arguments,
                MAX_BOOLEAN_TERMS,
                query_builder_function_argument_limit_error,
            )?,
        ))
        .map(|inner| NodeQueryV2PatternHandle { inner })
    }

    /// Create one typed sort term.
    #[napi]
    pub fn order(
        &self,
        binding: &NodeQueryV2BindingHandle,
        direction: NodeVocabularyString,
    ) -> napi::Result<NodeQueryV2OrderHandle> {
        let direction = diagnostic(query_builder_order_direction(direction.as_str()))?;
        diagnostic(self.inner.order(&binding.inner, direction))
            .map(|inner| NodeQueryV2OrderHandle { inner })
    }

    /// Create one reducer assignment.
    #[napi(js_name = "reduceAssignment")]
    pub fn reduce_assignment(
        &self,
        assigned: &NodeQueryV2BindingHandle,
        reducer: NodeVocabularyString,
        input: Option<&NodeQueryV2BindingHandle>,
    ) -> napi::Result<NodeQueryV2ReduceAssignmentHandle> {
        let reducer = diagnostic(query_builder_reducer(reducer.as_str()))?;
        diagnostic(self.inner.reduce_assignment(
            &assigned.inner,
            reducer,
            input.map(|input| &input.inner),
        ))
        .map(|inner| NodeQueryV2ReduceAssignmentHandle { inner })
    }

    /// Create one local-function return.
    #[napi(js_name = "localReturn")]
    pub fn local_return(
        &self,
        reducer: NodeVocabularyString,
        input: &NodeQueryV2BindingHandle,
        value_type: NodeVocabularyString,
    ) -> napi::Result<NodeQueryV2LocalReturnHandle> {
        let reducer = diagnostic(query_builder_reducer(reducer.as_str()))?;
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        diagnostic(self.inner.local_return(reducer, &input.inner, value_type))
            .map(|inner| NodeQueryV2LocalReturnHandle { inner })
    }

    /// Consume one private binding set into a local function.
    #[napi(js_name = "localFunction")]
    #[allow(clippy::too_many_arguments)]
    pub fn local_function(
        &mut self,
        name: NodeLabelString,
        bindings: Unknown,
        parameter_bindings: Unknown,
        parameter_labels: Unknown,
        body: Unknown,
        returns: &NodeQueryV2LocalReturnHandle,
    ) -> napi::Result<NodeQueryV2LocalFunctionHandle> {
        let name = diagnostic(FunctionId::new(name.into_inner()))?;
        let parameter_labels = bounded_array(
            parameter_labels,
            MAX_BINDINGS,
            query_builder_local_binding_limit_error,
        )?
        .into_iter()
        .map(NodeLabelString::from_unknown)
        .map(|label| label.map(NodeLabelString::into_inner))
        .collect::<napi::Result<Vec<_>>>()?;
        let parameters = diagnostic(query_builder_local_parameters(
            binding_handles(
                parameter_bindings,
                MAX_BINDINGS,
                query_builder_local_binding_limit_error,
            )?,
            parameter_labels,
        ))?;
        diagnostic(self.inner.local_function(
            name,
            binding_handles(
                bindings,
                MAX_BINDINGS,
                query_builder_local_binding_limit_error,
            )?,
            parameters,
            pattern_handles(
                body,
                MAX_BOOLEAN_TERMS,
                query_builder_local_body_limit_error,
            )?,
            &returns.inner,
        ))
        .map(|inner| NodeQueryV2LocalFunctionHandle { inner })
    }

    /// Attach the one root match conjunction.
    #[napi(js_name = "match")]
    pub fn match_stage(&mut self, patterns: Unknown) -> napi::Result<()> {
        diagnostic(self.inner.r#match(pattern_handles(
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_root_pattern_limit_error,
        )?))
    }

    /// Attach a visible-binding selection.
    #[napi]
    pub fn select(&mut self, bindings: Unknown) -> napi::Result<()> {
        diagnostic(self.inner.select(binding_handles(
            bindings,
            MAX_BINDINGS,
            query_builder_binding_limit_error,
        )?))
    }

    /// Attach a mandatory-binding requirement.
    #[napi]
    pub fn require(&mut self, bindings: Unknown) -> napi::Result<()> {
        diagnostic(self.inner.require(binding_handles(
            bindings,
            MAX_BINDINGS,
            query_builder_binding_limit_error,
        )?))
    }

    /// Attach row deduplication.
    #[napi]
    pub fn distinct(&mut self) -> napi::Result<()> {
        diagnostic(self.inner.distinct())
    }

    /// Attach a reduction stage.
    #[napi]
    pub fn reduce(&mut self, assignments: Unknown, groups: Unknown) -> napi::Result<()> {
        let assignments = bounded_array(
            assignments,
            MAX_BOOLEAN_TERMS,
            query_builder_reduce_term_limit_error,
        )?
        .into_iter()
        .map(|assignment| {
            Reference::<NodeQueryV2ReduceAssignmentHandle>::from_unknown(assignment)
                .map(|assignment| assignment.inner.clone())
        })
        .collect::<napi::Result<Vec<_>>>()?;
        diagnostic(self.inner.reduce(
            assignments,
            binding_handles(groups, MAX_BINDINGS, query_builder_binding_limit_error)?,
        ))
    }

    /// Attach a total sort.
    #[napi]
    pub fn sort(&mut self, terms: Unknown) -> napi::Result<()> {
        let terms = bounded_array(terms, MAX_ORDER_TERMS, query_builder_sort_term_limit_error)?
            .into_iter()
            .map(|term| {
                Reference::<NodeQueryV2OrderHandle>::from_unknown(term)
                    .map(|term| term.inner.clone())
            })
            .collect::<napi::Result<Vec<_>>>()?;
        diagnostic(self.inner.sort(terms))
    }

    /// Attach an ordered row offset.
    #[napi]
    pub fn offset(&mut self, rows: Unknown) -> napi::Result<()> {
        diagnostic(self.inner.offset(node_u64(rows)?))
    }

    /// Attach an ordered row limit.
    #[napi]
    pub fn limit(&mut self, rows: Unknown) -> napi::Result<()> {
        diagnostic(self.inner.limit(node_u64(rows)?))
    }

    /// Create a document scalar field.
    #[napi(js_name = "documentBinding")]
    pub fn document_binding(
        &self,
        key: NodeVariableString,
        binding: &NodeQueryV2BindingHandle,
    ) -> napi::Result<NodeQueryV2DocumentFieldHandle> {
        diagnostic(
            self.inner
                .document_binding(key.into_inner(), &binding.inner),
        )
        .map(|inner| NodeQueryV2DocumentFieldHandle { inner })
    }

    /// Create a document attribute-list field.
    #[napi(js_name = "documentAttributeList")]
    pub fn document_attribute_list(
        &self,
        key: NodeVariableString,
        owner: &NodeQueryV2BindingHandle,
        attribute_label: NodeLabelString,
    ) -> napi::Result<NodeQueryV2DocumentFieldHandle> {
        let attribute = diagnostic(AttributeId::new(attribute_label.into_inner()))?;
        diagnostic(
            self.inner
                .document_attribute_list(key.into_inner(), &owner.inner, attribute),
        )
        .map(|inner| NodeQueryV2DocumentFieldHandle { inner })
    }

    /// Finalize a row-output V2 plan.
    #[napi(js_name = "finalizeRows")]
    pub fn finalize_rows(&mut self, columns: Unknown) -> napi::Result<NodeAuthoredQueryPlan> {
        diagnostic(self.inner.finalize_rows(binding_handles(
            columns,
            MAX_SELECTED_SLOTS,
            query_builder_row_output_limit_error,
        )?))
        .map(|inner| NodeAuthoredQueryPlan { inner })
    }

    /// Finalize a document-output V2 plan.
    #[napi(js_name = "finalizeDocuments")]
    pub fn finalize_documents(&mut self, fields: Unknown) -> napi::Result<NodeAuthoredQueryPlan> {
        let fields = bounded_array(
            fields,
            MAX_SELECTED_SLOTS,
            query_builder_document_output_limit_error,
        )?
        .into_iter()
        .map(|field| {
            Reference::<NodeQueryV2DocumentFieldHandle>::from_unknown(field)
                .map(|field| field.inner.clone())
        })
        .collect::<napi::Result<Vec<_>>>()?;
        diagnostic(self.inner.finalize_documents(fields))
            .map(|inner| NodeAuthoredQueryPlan { inner })
    }
}

#[cfg(test)]
mod tests {
    use type_bridge_orm::query_v2_builder::QUERY_PLAN_BUILDER_OPERATIONS;

    #[test]
    fn every_shared_operation_has_one_napi_method() {
        let methods = [
            ("binding", "binding"),
            ("input", "input"),
            ("binding_operand", "binding_operand"),
            ("literal_operand", "literal_operand"),
            ("input_operand", "input_operand"),
            ("isa", "isa"),
            ("has", "has"),
            ("links", "links"),
            ("value", "value"),
            ("not", "not_pattern"),
            ("or", "or_pattern"),
            ("try", "try_pattern"),
            ("reachable", "reachable"),
            ("function_call", "function_call"),
            ("order", "order"),
            ("reduce_assignment", "reduce_assignment"),
            ("local_return", "local_return"),
            ("local_function", "local_function"),
            ("match", "match_stage"),
            ("select", "select"),
            ("require", "require"),
            ("distinct", "distinct"),
            ("reduce", "reduce"),
            ("sort", "sort"),
            ("offset", "offset"),
            ("limit", "limit"),
            ("document_binding", "document_binding"),
            ("document_attribute_list", "document_attribute_list"),
            ("finalize_rows", "finalize_rows"),
            ("finalize_documents", "finalize_documents"),
        ];
        assert_eq!(
            methods.map(|(shared, _)| shared),
            QUERY_PLAN_BUILDER_OPERATIONS
        );
        let source = include_str!("query_v2_builder_runtime.rs");
        for (_, method) in methods {
            assert!(
                source.contains(&format!("fn {method}(")),
                "missing N-API method {method}"
            );
        }
    }
}
