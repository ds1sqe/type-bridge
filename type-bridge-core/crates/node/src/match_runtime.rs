//! Opaque N-API seam for canonical match-request construction.
//!
//! JavaScript receives persistent native handles and deterministic diagnostic
//! strings only. It never owns a semantic request DTO, plan-local identity, or
//! live request token.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use type_bridge_orm::{
    BindingHandle, ComparisonOp, FieldHandle, HydratedAttribute, HydratedRole, HydratedRolePlayer,
    HydratedThing, MatchError, MatchErrorCategory, MatchRequest, MatchResult, MissingOrder,
    OrderHandle, PredicateHandle, QueryHandle, ReducedValue, Reduction, ReductionRow, RoleHandle,
    RowCardinality, SelectionHandle, SessionHandle, ShapeHandle, SlotValue, SortDirection,
    ThingKind, UnvalidatedMatchRequest, ValidatedMatchRequest, ValidatedMatchResult, Window,
    validate_public_order_term_count,
};

use crate::{NodeDescriptorRegistry, NodeRustDatabase, NodeRustTransactionContext};

/// Stable JSON payload placed in the reason of an N-API match error.
#[derive(Serialize)]
struct NativeMatchErrorPayload<'a> {
    category: &'static str,
    code: &'a str,
    message: &'a str,
    path: &'a type_bridge_orm::MatchErrorPath,
    details: &'a std::collections::BTreeMap<String, type_bridge_orm::MatchErrorDetailValue>,
}

pub(crate) fn napi_match_error(error: MatchError) -> napi::Error {
    let status = match error.category() {
        MatchErrorCategory::InvalidPlan | MatchErrorCategory::ResourceLimit => Status::InvalidArg,
        MatchErrorCategory::Cardinality
        | MatchErrorCategory::UnsupportedCapability
        | MatchErrorCategory::StaleSchema
        | MatchErrorCategory::Provider
        | MatchErrorCategory::ResultDecode => Status::GenericFailure,
    };
    let payload = NativeMatchErrorPayload {
        category: error.category().as_str(),
        code: error.code().as_str(),
        message: error.message(),
        path: error.path(),
        details: error.details(),
    };
    let reason = serde_json::to_string(&payload)
        .expect("structured match errors contain only JSON-safe canonical values");
    Error::new(status, reason)
}

fn invalid_arg(message: impl Into<String>) -> napi::Error {
    Error::new(Status::InvalidArg, message.into())
}

fn result_decode_error(code: &'static str, message: impl Into<String>) -> napi::Error {
    let reason = json!({
        "category": "result_decode",
        "code": code,
        "message": message.into(),
        "path": [{ "kind": "result" }],
        "details": {},
    })
    .to_string();
    Error::new(Status::GenericFailure, reason)
}

fn parse_comparison(value: &str) -> Result<ComparisonOp> {
    match value {
        "equal" => Ok(ComparisonOp::Equal),
        "not_equal" => Ok(ComparisonOp::NotEqual),
        "less_than" => Ok(ComparisonOp::LessThan),
        "less_than_or_equal" => Ok(ComparisonOp::LessThanOrEqual),
        "greater_than" => Ok(ComparisonOp::GreaterThan),
        "greater_than_or_equal" => Ok(ComparisonOp::GreaterThanOrEqual),
        "contains" => Ok(ComparisonOp::Contains),
        "starts_with" => Ok(ComparisonOp::StartsWith),
        "ends_with" => Ok(ComparisonOp::EndsWith),
        "regex" => Ok(ComparisonOp::Regex),
        _ => Err(invalid_arg(format!(
            "comparison must be a canonical comparison name, got '{value}'"
        ))),
    }
}

fn parse_direction(value: &str) -> Result<SortDirection> {
    match value {
        "ascending" => Ok(SortDirection::Ascending),
        "descending" => Ok(SortDirection::Descending),
        _ => Err(invalid_arg(format!(
            "direction must be 'ascending' or 'descending', got '{value}'"
        ))),
    }
}

fn parse_missing(value: &str) -> Result<MissingOrder> {
    match value {
        "reject" => Ok(MissingOrder::Reject),
        "first" => Ok(MissingOrder::First),
        "last" => Ok(MissingOrder::Last),
        _ => Err(invalid_arg(format!(
            "missing order must be 'reject', 'first', or 'last', got '{value}'"
        ))),
    }
}

pub(crate) fn parse_cardinality(value: &str) -> Result<RowCardinality> {
    match value {
        "exactly_one" => Ok(RowCardinality::ExactlyOne),
        "bounded_many" => Ok(RowCardinality::BoundedMany),
        _ => Err(invalid_arg(format!(
            "cardinality must be 'exactly_one' or 'bounded_many', got '{value}'"
        ))),
    }
}

fn parse_reduction(value: &str) -> Result<Reduction> {
    match value {
        "count" => Ok(Reduction::Count),
        "sum" => Ok(Reduction::Sum),
        "min" => Ok(Reduction::Min),
        "max" => Ok(Reduction::Max),
        "mean" => Ok(Reduction::Mean),
        "median" => Ok(Reduction::Median),
        "std" => Ok(Reduction::Std),
        _ => Err(invalid_arg(format!(
            "reducer must be a canonical reducer name, got '{value}'"
        ))),
    }
}

fn reduce_terms(
    reducers: &[String],
    inputs: &[Option<Reference<NodeMatchFieldHandle>>],
) -> Result<Vec<(Reduction, Option<FieldHandle>)>> {
    if reducers.len() != inputs.len() {
        return Err(invalid_arg(
            "reducer names and reducer inputs must have equal length",
        ));
    }
    reducers
        .iter()
        .zip(inputs)
        .map(|(reducer, input)| {
            Ok((
                parse_reduction(reducer)?,
                input.as_ref().map(|field| field.inner.clone()),
            ))
        })
        .collect()
}

fn borrow_reduce_terms(
    terms: &[(Reduction, Option<FieldHandle>)],
) -> Vec<(Reduction, Option<&FieldHandle>)> {
    terms
        .iter()
        .map(|(reduction, input)| (*reduction, input.as_ref()))
        .collect()
}

pub(crate) fn bigint_u64(value: &BigInt, name: &str) -> Result<u64> {
    let (negative, value, lossless) = value.get_u64();
    if negative || !lossless {
        return Err(invalid_arg(format!(
            "{name} must be a non-negative bigint within the u64 range"
        )));
    }
    Ok(value)
}

fn u64_bigint(value: u64) -> BigInt {
    BigInt {
        sign_bit: false,
        words: vec![value],
    }
}

fn diagnostic(request: MatchRequest) -> Result<String> {
    let diagnostic = UnvalidatedMatchRequest::from_request(request).map_err(napi_match_error)?;
    let bytes = diagnostic.to_canonical_bytes().map_err(napi_match_error)?;
    String::from_utf8(bytes)
        .map_err(|_| Error::new(Status::GenericFailure, "diagnostic JSON was not UTF-8"))
}

pub(crate) fn order_handles(orders: &[Reference<NodeMatchOrderHandle>]) -> Vec<OrderHandle> {
    orders.iter().map(|order| order.inner.clone()).collect()
}

/// Deserialize a canonical diagnostic, validate it against `registry`, and
/// return its exact canonical bytes as a UTF-8 string.
///
/// The validation proof and its invocation token remain native and are not
/// serialized or returned to JavaScript.
#[napi(js_name = "revalidateMatchDiagnostic")]
pub fn revalidate_match_diagnostic(
    registry: &NodeDescriptorRegistry,
    diagnostic_json: String,
) -> Result<String> {
    let unvalidated = UnvalidatedMatchRequest::from_canonical_bytes(diagnostic_json.as_bytes())
        .map_err(napi_match_error)?;
    let canonical = unvalidated.to_canonical_bytes().map_err(napi_match_error)?;
    unvalidated
        .validate(registry.shared_registry().as_ref())
        .map_err(napi_match_error)?;
    String::from_utf8(canonical)
        .map_err(|_| Error::new(Status::GenericFailure, "diagnostic JSON was not UTF-8"))
}

/// Apply the canonical public-order ceiling before JavaScript maps a caller
/// array into native order handles.
#[napi(js_name = "validateMatchOrderTermCount")]
pub fn validate_match_order_term_count(actual: u32) -> Result<()> {
    validate_public_order_term_count(actual as usize).map_err(napi_match_error)
}

/// Opaque owner of one native match-handle construction session.
#[napi]
pub struct NodeMatchSessionHandle {
    inner: SessionHandle,
}

#[napi]
impl NodeMatchSessionHandle {
    #[napi(constructor)]
    pub fn new(registry: &NodeDescriptorRegistry) -> Self {
        Self {
            inner: SessionHandle::new(registry.shared_registry()),
        }
    }

    #[napi]
    pub fn exact(&self, type_name: String) -> Result<NodeMatchBindingHandle> {
        self.inner
            .exact(&type_name)
            .map(|inner| NodeMatchBindingHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn subtypes(&self, type_name: String) -> Result<NodeMatchBindingHandle> {
        self.inner
            .subtypes(&type_name)
            .map(|inner| NodeMatchBindingHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn reachable(
        &self,
        relation_type: String,
        role_from: String,
        role_to: String,
        source: &NodeMatchBindingHandle,
        target: &NodeMatchBindingHandle,
        min_depth: f64,
        max_depth: f64,
    ) -> Result<NodeMatchPredicateHandle> {
        let min_depth = node_reachability_depth(min_depth, "minDepth")?;
        let max_depth = node_reachability_depth(max_depth, "maxDepth")?;
        self.inner
            .reachable(
                &relation_type,
                &role_from,
                &role_to,
                &source.inner,
                &target.inner,
                min_depth,
                max_depth,
            )
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn positional(
        &self,
        selections: Vec<Reference<NodeMatchSelectionHandle>>,
    ) -> Result<NodeMatchShapeHandle> {
        let slots = selections.iter().map(|selection| selection.kind).collect();
        self.inner
            .positional(selections.iter().map(|selection| selection.inner.clone()))
            .map(|inner| NodeMatchShapeHandle {
                inner,
                output: Arc::new(NodeOutputShape::Positional { slots }),
            })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn named(
        &self,
        names: Vec<String>,
        selections: Vec<Reference<NodeMatchSelectionHandle>>,
    ) -> Result<NodeMatchShapeHandle> {
        if names.len() != selections.len() {
            return Err(invalid_arg(
                "named output names and selections must have equal length",
            ));
        }
        let output = Arc::new(NodeOutputShape::Named {
            names: names.clone(),
            slots: selections.iter().map(|selection| selection.kind).collect(),
        });
        self.inner
            .named(
                names
                    .into_iter()
                    .zip(selections.iter().map(|selection| selection.inner.clone())),
            )
            .map(|inner| NodeMatchShapeHandle { inner, output })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn query(&self, shape: &NodeMatchShapeHandle) -> Result<NodeMatchQueryHandle> {
        self.inner
            .query(shape.inner.clone())
            .map(|inner| NodeMatchQueryHandle {
                inner,
                output: Arc::clone(&shape.output),
                lineage: Arc::new(()),
            })
            .map_err(crate::napi_orm_error)
    }
}

fn node_reachability_depth(value: f64, name: &str) -> Result<u8> {
    if !value.is_finite()
        || value.fract() != 0.0
        || value < f64::from(u8::MIN)
        || value > f64::from(u8::MAX)
    {
        return Err(invalid_arg(format!(
            "{name} must be an integer between 0 and 255"
        )));
    }
    Ok(value as u8)
}

/// Opaque native binding handle.
#[napi]
pub struct NodeMatchBindingHandle {
    inner: BindingHandle,
}

impl NodeMatchBindingHandle {
    pub(crate) const fn inner(&self) -> &BindingHandle {
        &self.inner
    }
}

#[napi]
impl NodeMatchBindingHandle {
    #[napi]
    pub fn field(&self, field_name: String) -> Result<NodeMatchFieldHandle> {
        self.inner
            .field(&field_name)
            .map(|inner| NodeMatchFieldHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "fieldOwnedBy")]
    pub fn field_owned_by(
        &self,
        owner_type: String,
        field_name: String,
    ) -> Result<NodeMatchFieldHandle> {
        self.inner
            .field_owned_by(&owner_type, &field_name)
            .map(|inner| NodeMatchFieldHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn role(&self, role_name: String) -> Result<NodeMatchRoleHandle> {
        self.inner
            .role(&role_name)
            .map(|inner| NodeMatchRoleHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "roleOwnedBy")]
    pub fn role_owned_by(
        &self,
        owner_type: String,
        role_name: String,
    ) -> Result<NodeMatchRoleHandle> {
        self.inner
            .role_owned_by(&owner_type, &role_name)
            .map(|inner| NodeMatchRoleHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn one(&self) -> NodeMatchSelectionHandle {
        NodeMatchSelectionHandle {
            inner: self.inner.one(),
            kind: NodeOutputSlotKind::One,
        }
    }

    #[napi]
    pub fn collect(&self) -> NodeMatchSelectionHandle {
        NodeMatchSelectionHandle {
            inner: self.inner.collect(),
            kind: NodeOutputSlotKind::Many,
        }
    }
}

/// Opaque native bound-field handle.
#[napi]
pub struct NodeMatchFieldHandle {
    inner: FieldHandle,
}

#[napi]
impl NodeMatchFieldHandle {
    #[napi(js_name = "compareValueJson")]
    pub fn compare_value_json(
        &self,
        comparison: String,
        value_json: String,
    ) -> Result<NodeMatchPredicateHandle> {
        let value: Value = serde_json::from_str(&value_json)
            .map_err(|error| invalid_arg(format!("invalid attribute value JSON: {error}")))?;
        let value = crate::attribute_value_from_js(&value, None)?;
        Ok(NodeMatchPredicateHandle {
            inner: self
                .inner
                .compare_value(parse_comparison(&comparison)?, value),
        })
    }

    #[napi(js_name = "compareField")]
    pub fn compare_field(
        &self,
        comparison: String,
        other: &NodeMatchFieldHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .compare_field(parse_comparison(&comparison)?, &other.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn order(&self, direction: String, missing: String) -> Result<NodeMatchOrderHandle> {
        Ok(NodeMatchOrderHandle {
            inner: self
                .inner
                .order(parse_direction(&direction)?, parse_missing(&missing)?),
        })
    }
}

/// Opaque native relation-role handle.
#[napi]
pub struct NodeMatchRoleHandle {
    inner: RoleHandle,
}

#[napi]
impl NodeMatchRoleHandle {
    #[napi]
    pub fn connects(&self, player: &NodeMatchBindingHandle) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .connects(&player.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }
}

/// Opaque native boolean predicate handle.
#[napi]
pub struct NodeMatchPredicateHandle {
    inner: PredicateHandle,
}

#[napi]
impl NodeMatchPredicateHandle {
    #[napi(js_name = "and")]
    pub fn and_predicate(
        &self,
        other: &NodeMatchPredicateHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .and(&other.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "or")]
    pub fn or_predicate(
        &self,
        other: &NodeMatchPredicateHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .or(&other.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "not")]
    pub fn not_predicate(&self) -> NodeMatchPredicateHandle {
        NodeMatchPredicateHandle {
            inner: self.inner.not(),
        }
    }
}

/// Opaque native public-order handle.
#[napi]
pub struct NodeMatchOrderHandle {
    inner: OrderHandle,
}

/// Opaque native output-selection handle.
#[napi]
pub struct NodeMatchSelectionHandle {
    inner: SelectionHandle,
    kind: NodeOutputSlotKind,
}

#[napi]
impl NodeMatchSelectionHandle {
    #[napi]
    pub fn distinct(&self, distinct: bool) -> Result<NodeMatchSelectionHandle> {
        self.inner
            .distinct(distinct)
            .map(|inner| NodeMatchSelectionHandle {
                inner,
                kind: self.kind,
            })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "orderBy")]
    pub fn order_by(&self, order: &NodeMatchOrderHandle) -> Result<NodeMatchSelectionHandle> {
        self.inner
            .order_by(order.inner.clone())
            .map(|inner| NodeMatchSelectionHandle {
                inner,
                kind: self.kind,
            })
            .map_err(crate::napi_orm_error)
    }
}

/// Opaque native positional/named output-shape handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeOutputSlotKind {
    One,
    Many,
}

#[derive(Debug)]
enum NodeOutputShape {
    Positional {
        slots: Vec<NodeOutputSlotKind>,
    },
    Named {
        names: Vec<String>,
        slots: Vec<NodeOutputSlotKind>,
    },
}

impl NodeOutputShape {
    fn slots(&self) -> &[NodeOutputSlotKind] {
        match self {
            Self::Positional { slots } | Self::Named { slots, .. } => slots,
        }
    }

    fn names(&self) -> Option<&[String]> {
        match self {
            Self::Positional { .. } => None,
            Self::Named { names, .. } => Some(names),
        }
    }
}

#[napi]
pub struct NodeMatchShapeHandle {
    inner: ShapeHandle,
    output: Arc<NodeOutputShape>,
}

/// Opaque native persistent query-lineage handle.
#[napi]
pub struct NodeMatchQueryHandle {
    inner: QueryHandle,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
}

impl NodeMatchQueryHandle {
    pub(crate) const fn inner(&self) -> &QueryHandle {
        &self.inner
    }

    pub(crate) fn result_context(&self) -> NodeMatchResultContext {
        NodeMatchResultContext {
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        }
    }

    fn derived(&self, inner: QueryHandle) -> Self {
        Self {
            inner,
            output: Arc::clone(&self.output),
            lineage: Arc::new(()),
        }
    }
}

#[napi]
impl NodeMatchQueryHandle {
    #[napi(js_name = "addHidden")]
    pub fn add_hidden(&self, binding: &NodeMatchBindingHandle) -> Result<NodeMatchQueryHandle> {
        self.inner
            .add_hidden(binding.inner.clone())
            .map(|inner| self.derived(inner))
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "wherePredicate")]
    pub fn where_predicate(
        &self,
        predicate: &NodeMatchPredicateHandle,
    ) -> Result<NodeMatchQueryHandle> {
        self.inner
            .where_predicate(predicate.inner.clone())
            .map(|inner| self.derived(inner))
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "allowCrossJoin")]
    pub fn allow_cross_join(
        &self,
        left: &NodeMatchBindingHandle,
        right: &NodeMatchBindingHandle,
    ) -> Result<NodeMatchQueryHandle> {
        self.inner
            .allow_cross_join(&left.inner, &right.inner)
            .map(|inner| self.derived(inner))
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "fetchRowsDiagnostic")]
    pub fn fetch_rows_diagnostic(
        &self,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        cardinality: String,
    ) -> Result<String> {
        let request = self
            .inner
            .fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_cardinality(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        diagnostic(request)
    }

    #[napi(js_name = "executeFetchRowsOwned")]
    pub fn execute_fetch_rows_owned(
        &self,
        database: &NodeRustDatabase,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        cardinality: String,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_cardinality(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (database, runtime) = database.handles();
        let inner = runtime
            .block_on(database.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeFetchRowsBorrowed")]
    pub fn execute_fetch_rows_borrowed(
        &self,
        transaction: &NodeRustTransactionContext,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        cardinality: String,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_cardinality(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (transaction, runtime) = transaction.handles();
        let inner = runtime
            .block_on(transaction.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executePageByOwned")]
    pub fn execute_page_by_owned(
        &self,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        include_total: bool,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_page_by(
                &root.inner,
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                include_total,
            )
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (database, runtime) = database.handles();
        let inner = runtime
            .block_on(database.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executePageByBorrowed")]
    pub fn execute_page_by_borrowed(
        &self,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        include_total: bool,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_page_by(
                &root.inner,
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                include_total,
            )
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (transaction, runtime) = transaction.handles();
        let inner = runtime
            .block_on(transaction.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeCountByOwned")]
    pub fn execute_count_by_owned(
        &self,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (database, runtime) = database.handles();
        let inner = runtime
            .block_on(database.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeCountByBorrowed")]
    pub fn execute_count_by_borrowed(
        &self,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (transaction, runtime) = transaction.handles();
        let inner = runtime
            .block_on(transaction.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeExistsByOwned")]
    pub fn execute_exists_by_owned(
        &self,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (database, runtime) = database.handles();
        let inner = runtime
            .block_on(database.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeExistsByBorrowed")]
    pub fn execute_exists_by_borrowed(
        &self,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (transaction, runtime) = transaction.handles();
        let inner = runtime
            .block_on(transaction.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "pageByDiagnostic")]
    pub fn page_by_diagnostic(
        &self,
        root: &NodeMatchBindingHandle,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        include_total: bool,
    ) -> Result<String> {
        let request = self
            .inner
            .page_by(
                &root.inner,
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                include_total,
            )
            .map_err(crate::napi_orm_error)?;
        diagnostic(request)
    }

    #[napi(js_name = "countByDiagnostic")]
    pub fn count_by_diagnostic(&self, root: &NodeMatchBindingHandle) -> Result<String> {
        diagnostic(
            self.inner
                .count_by(&root.inner)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(js_name = "existsByDiagnostic")]
    pub fn exists_by_diagnostic(&self, root: &NodeMatchBindingHandle) -> Result<String> {
        diagnostic(
            self.inner
                .exists_by(&root.inner)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(js_name = "reduceByDiagnostic")]
    pub fn reduce_by_diagnostic(
        &self,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<String> {
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic(
            self.inner
                .reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(js_name = "executeReduceByOwned")]
    pub fn execute_reduce_by_owned(
        &self,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (database, runtime) = database.handles();
        let inner = runtime
            .block_on(database.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }

    #[napi(js_name = "executeReduceByBorrowed")]
    pub fn execute_reduce_by_borrowed(
        &self,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
            .map_err(crate::napi_orm_error)?;
        let registry = self.inner.registry_arc();
        let (transaction, runtime) = transaction.handles();
        let inner = runtime
            .block_on(transaction.execute_match(&registry, &validated))
            .map_err(crate::napi_orm_error)?;
        Ok(NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
        })
    }
}

/// Opaque proof that Rust validated one FetchRows provider result.
///
/// The invocation token and shape ID never cross N-API. Access requires the
/// exact immutable query lineage that executed the request.
#[napi]
pub struct NodeValidatedMatchResultHandle {
    inner: ValidatedMatchResult,
    request: ValidatedMatchRequest,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
}

/// Immutable materialization metadata retained while a remote reply is decoded.
#[derive(Clone)]
pub(crate) struct NodeMatchResultContext {
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
}

impl NodeMatchResultContext {
    pub(crate) fn attach(
        self,
        request: ValidatedMatchRequest,
        inner: ValidatedMatchResult,
    ) -> NodeValidatedMatchResultHandle {
        NodeValidatedMatchResultHandle {
            inner,
            request,
            output: self.output,
            lineage: self.lineage,
        }
    }
}

impl NodeValidatedMatchResultHandle {
    fn result<'a>(&'a self, query: &NodeMatchQueryHandle) -> Result<&'a MatchResult> {
        if !Arc::ptr_eq(&self.lineage, &query.lineage) || !Arc::ptr_eq(&self.output, &query.output)
        {
            return Err(result_decode_error(
                "result_query_mismatch",
                "validated result belongs to a different immutable query lineage",
            ));
        }
        self.inner
            .for_request(&self.request)
            .map_err(napi_match_error)
    }

    fn rows<'a>(&'a self, query: &NodeMatchQueryHandle) -> Result<&'a [type_bridge_orm::MatchRow]> {
        match self.result(query)? {
            MatchResult::Rows { rows } => Ok(rows),
            _ => Err(result_decode_error(
                "result_operation_mismatch",
                "validated result is not a FetchRows result",
            )),
        }
    }

    fn page<'a>(
        &'a self,
        query: &NodeMatchQueryHandle,
    ) -> Result<(&'a [type_bridge_orm::MatchRow], Window, Option<u64>)> {
        match self.result(query)? {
            MatchResult::Page {
                entries,
                window,
                total,
                ..
            } => Ok((entries, *window, *total)),
            _ => Err(result_decode_error(
                "result_operation_mismatch",
                "validated result is not a PageBy result",
            )),
        }
    }

    fn page_entry<'a>(
        &'a self,
        query: &NodeMatchQueryHandle,
        entry_index: u32,
    ) -> Result<&'a type_bridge_orm::MatchRow> {
        self.page(query)?
            .0
            .get(entry_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_page_entry_out_of_bounds",
                    "entry index is outside the validated page result",
                )
            })
    }

    fn output_slot_kind(
        &self,
        query: &NodeMatchQueryHandle,
        slot_index: u32,
    ) -> Result<NodeOutputSlotKind> {
        self.result(query)?;
        self.output
            .slots()
            .get(slot_index as usize)
            .copied()
            .ok_or_else(|| {
                result_decode_error(
                    "result_output_slot_out_of_bounds",
                    "slot index is outside the native output shape",
                )
            })
    }

    fn require_slot_shape(
        &self,
        query: &NodeMatchQueryHandle,
        slot_index: u32,
        slot: &SlotValue,
    ) -> Result<NodeOutputSlotKind> {
        let expected = self.output_slot_kind(query, slot_index)?;
        let actual = match slot {
            SlotValue::One(_) => NodeOutputSlotKind::One,
            SlotValue::Many(_) => NodeOutputSlotKind::Many,
        };
        if actual != expected {
            return Err(result_decode_error(
                "result_slot_kind_mismatch",
                "validated result slot kind differs from its native output selection",
            ));
        }
        Ok(actual)
    }

    fn row<'a>(
        &'a self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<&'a type_bridge_orm::MatchRow> {
        self.rows(query)?.get(row_index as usize).ok_or_else(|| {
            result_decode_error(
                "result_row_out_of_bounds",
                "row index is outside the validated result",
            )
        })
    }

    fn reduction<'a>(&'a self, query: &NodeMatchQueryHandle) -> Result<&'a [ReductionRow]> {
        match self.result(query)? {
            MatchResult::Reduction { rows, .. } => Ok(rows),
            _ => Err(result_decode_error(
                "result_operation_mismatch",
                "validated result is not a ReduceBy result",
            )),
        }
    }

    fn reduction_row<'a>(
        &'a self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<&'a ReductionRow> {
        self.reduction(query)?
            .get(row_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_reduction_row_out_of_bounds",
                    "row index is outside the validated reduction result",
                )
            })
    }

    fn reduced_value<'a>(
        &'a self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        value_index: u32,
    ) -> Result<&'a ReducedValue> {
        self.reduction_row(query, row_index)?
            .values()
            .get(value_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_reduction_value_out_of_bounds",
                    "value index is outside the validated reduction row",
                )
            })
    }
}

#[napi]
impl NodeValidatedMatchResultHandle {
    #[napi(js_name = "outputSlotCount")]
    pub fn output_slot_count(&self, query: &NodeMatchQueryHandle) -> Result<u32> {
        self.result(query)?;
        u32::try_from(self.output.slots().len()).map_err(|_| {
            result_decode_error(
                "result_slot_count_overflow",
                "validated output slot count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "outputSlotIsCollection")]
    pub fn output_slot_is_collection(
        &self,
        query: &NodeMatchQueryHandle,
        slot_index: u32,
    ) -> Result<bool> {
        Ok(self.output_slot_kind(query, slot_index)? == NodeOutputSlotKind::Many)
    }

    #[napi(js_name = "rowCount")]
    pub fn row_count(&self, query: &NodeMatchQueryHandle) -> Result<u32> {
        u32::try_from(self.rows(query)?.len()).map_err(|_| {
            result_decode_error(
                "result_row_count_overflow",
                "validated row count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "slotCount")]
    pub fn slot_count(&self, query: &NodeMatchQueryHandle, row_index: u32) -> Result<u32> {
        let slots = self.row(query, row_index)?.slots();
        let expected = self.output.slots().len();
        if slots.len() != expected {
            return Err(result_decode_error(
                "result_slot_count_mismatch",
                "validated row slot count differs from its native output shape",
            ));
        }
        u32::try_from(slots.len()).map_err(|_| {
            result_decode_error(
                "result_slot_count_overflow",
                "validated slot count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "outputNames")]
    pub fn output_names(&self, query: &NodeMatchQueryHandle) -> Result<Option<Vec<String>>> {
        self.result(query)?;
        Ok(self.output.names().map(<[String]>::to_vec))
    }

    #[napi(js_name = "slotThing")]
    pub fn slot_thing(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        slot_index: u32,
    ) -> Result<NodeValidatedThingHandle> {
        let slot = self
            .row(query, row_index)?
            .slots()
            .get(slot_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_slot_out_of_bounds",
                    "slot index is outside the validated row",
                )
            })?;
        self.require_slot_shape(query, slot_index, slot)?;
        match slot {
            SlotValue::One(thing) => Ok(NodeValidatedThingHandle {
                inner: NodeValidatedThing::Selected(thing.clone()),
            }),
            SlotValue::Many(_) => Err(result_decode_error(
                "collection_in_fetch_rows_result",
                "FetchRows materialization cannot consume a collected slot",
            )),
        }
    }

    #[napi(js_name = "pageEntryCount")]
    pub fn page_entry_count(&self, query: &NodeMatchQueryHandle) -> Result<u32> {
        u32::try_from(self.page(query)?.0.len()).map_err(|_| {
            result_decode_error(
                "result_page_entry_count_overflow",
                "validated page entry count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "pageSlotCount")]
    pub fn page_slot_count(&self, query: &NodeMatchQueryHandle, entry_index: u32) -> Result<u32> {
        let slots = self.page_entry(query, entry_index)?.slots();
        if slots.len() != self.output.slots().len() {
            return Err(result_decode_error(
                "result_slot_count_mismatch",
                "validated page row slot count differs from its native output shape",
            ));
        }
        u32::try_from(slots.len()).map_err(|_| {
            result_decode_error(
                "result_slot_count_overflow",
                "validated page slot count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "pageSlotValueCount")]
    pub fn page_slot_value_count(
        &self,
        query: &NodeMatchQueryHandle,
        entry_index: u32,
        slot_index: u32,
    ) -> Result<u32> {
        let slot = self
            .page_entry(query, entry_index)?
            .slots()
            .get(slot_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_slot_out_of_bounds",
                    "slot index is outside the validated page row",
                )
            })?;
        self.require_slot_shape(query, slot_index, slot)?;
        let count = match slot {
            SlotValue::One(_) => 1,
            SlotValue::Many(things) => things.len(),
        };
        u32::try_from(count).map_err(|_| {
            result_decode_error(
                "result_slot_value_count_overflow",
                "validated page slot value count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "pageSlotThing")]
    pub fn page_slot_thing(
        &self,
        query: &NodeMatchQueryHandle,
        entry_index: u32,
        slot_index: u32,
        value_index: u32,
    ) -> Result<NodeValidatedThingHandle> {
        let slot = self
            .page_entry(query, entry_index)?
            .slots()
            .get(slot_index as usize)
            .ok_or_else(|| {
                result_decode_error(
                    "result_slot_out_of_bounds",
                    "slot index is outside the validated page row",
                )
            })?;
        self.require_slot_shape(query, slot_index, slot)?;
        let thing = match slot {
            SlotValue::One(thing) if value_index == 0 => thing,
            SlotValue::One(_) => {
                return Err(result_decode_error(
                    "result_slot_value_out_of_bounds",
                    "singular page slot accepts only value index zero",
                ));
            }
            SlotValue::Many(things) => things.get(value_index as usize).ok_or_else(|| {
                result_decode_error(
                    "result_slot_value_out_of_bounds",
                    "value index is outside the validated collected slot",
                )
            })?,
        };
        Ok(NodeValidatedThingHandle {
            inner: NodeValidatedThing::Selected(thing.clone()),
        })
    }

    #[napi(js_name = "pageOffset")]
    pub fn page_offset(&self, query: &NodeMatchQueryHandle) -> Result<BigInt> {
        Ok(u64_bigint(self.page(query)?.1.offset))
    }

    #[napi(js_name = "pageLimit")]
    pub fn page_limit(&self, query: &NodeMatchQueryHandle) -> Result<BigInt> {
        Ok(u64_bigint(self.page(query)?.1.limit))
    }

    #[napi(js_name = "pageTotal")]
    pub fn page_total(&self, query: &NodeMatchQueryHandle) -> Result<Option<BigInt>> {
        Ok(self.page(query)?.2.map(u64_bigint))
    }

    #[napi(js_name = "countValue")]
    pub fn count_value(&self, query: &NodeMatchQueryHandle) -> Result<BigInt> {
        match self.result(query)? {
            MatchResult::Count { value, .. } => Ok(u64_bigint(*value)),
            _ => Err(result_decode_error(
                "result_operation_mismatch",
                "validated result is not a CountBy result",
            )),
        }
    }

    #[napi(js_name = "existsValue")]
    pub fn exists_value(&self, query: &NodeMatchQueryHandle) -> Result<bool> {
        match self.result(query)? {
            MatchResult::Exists { value, .. } => Ok(*value),
            _ => Err(result_decode_error(
                "result_operation_mismatch",
                "validated result is not an ExistsBy result",
            )),
        }
    }

    #[napi(js_name = "reductionRowCount")]
    pub fn reduction_row_count(&self, query: &NodeMatchQueryHandle) -> Result<u32> {
        u32::try_from(self.reduction(query)?.len()).map_err(|_| {
            result_decode_error(
                "result_reduction_row_count_overflow",
                "validated reduction row count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "reductionValueCount")]
    pub fn reduction_value_count(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<u32> {
        u32::try_from(self.reduction_row(query, row_index)?.values().len()).map_err(|_| {
            result_decode_error(
                "result_reduction_value_count_overflow",
                "validated reduction value count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "reductionValueKind")]
    pub fn reduction_value_kind(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        value_index: u32,
    ) -> Result<String> {
        Ok(match self.reduced_value(query, row_index, value_index)? {
            ReducedValue::Count(_) => "count".to_owned(),
            ReducedValue::Long(_) => "long".to_owned(),
            ReducedValue::Double(_) => "double".to_owned(),
        })
    }

    #[napi(js_name = "reductionCountValue")]
    pub fn reduction_count_value(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        value_index: u32,
    ) -> Result<BigInt> {
        match self.reduced_value(query, row_index, value_index)? {
            ReducedValue::Count(value) => Ok(u64_bigint(*value)),
            _ => Err(result_decode_error(
                "result_reduction_value_kind_mismatch",
                "validated reduction value is not a count",
            )),
        }
    }

    #[napi(js_name = "reductionLongValue")]
    pub fn reduction_long_value(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        value_index: u32,
    ) -> Result<Option<BigInt>> {
        match self.reduced_value(query, row_index, value_index)? {
            ReducedValue::Long(value) => Ok(value.map(BigInt::from)),
            _ => Err(result_decode_error(
                "result_reduction_value_kind_mismatch",
                "validated reduction value is not an integer-domain result",
            )),
        }
    }

    #[napi(js_name = "reductionDoubleValue")]
    pub fn reduction_double_value(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
        value_index: u32,
    ) -> Result<Option<f64>> {
        match self.reduced_value(query, row_index, value_index)? {
            ReducedValue::Double(value) => Ok(*value),
            _ => Err(result_decode_error(
                "result_reduction_value_kind_mismatch",
                "validated reduction value is not a double-domain result",
            )),
        }
    }

    #[napi(js_name = "reductionGroup")]
    pub fn reduction_group(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<NodeValidatedThingHandle> {
        let group = self
            .reduction_row(query, row_index)?
            .group()
            .ok_or_else(|| {
                result_decode_error(
                    "result_reduction_ungrouped",
                    "ungrouped reduction row carries no group evidence",
                )
            })?;
        Ok(NodeValidatedThingHandle {
            inner: NodeValidatedThing::Selected(group.clone()),
        })
    }
}

#[derive(Debug)]
enum NodeValidatedThing {
    Selected(HydratedThing),
    RolePlayer(HydratedRolePlayer),
}

impl NodeValidatedThing {
    fn concept_id(&self) -> &str {
        match self {
            Self::Selected(thing) => thing.concept_id().as_str(),
            Self::RolePlayer(player) => player.concept_id().as_str(),
        }
    }

    fn concrete_descriptor(&self) -> &str {
        match self {
            Self::Selected(thing) => thing.concrete_descriptor().as_str(),
            Self::RolePlayer(player) => player.concrete_descriptor().as_str(),
        }
    }

    fn role_data_complete(&self) -> bool {
        match self {
            Self::Selected(_) => true,
            Self::RolePlayer(player) => player.kind() == ThingKind::Entity,
        }
    }

    fn kind(&self) -> ThingKind {
        match self {
            Self::Selected(thing) => thing.kind(),
            Self::RolePlayer(player) => player.kind(),
        }
    }

    fn attributes(&self) -> &[HydratedAttribute] {
        match self {
            Self::Selected(thing) => thing.attributes(),
            Self::RolePlayer(player) => player.attributes(),
        }
    }

    fn roles(&self) -> &[HydratedRole] {
        match self {
            Self::Selected(thing) => thing.roles(),
            Self::RolePlayer(_) => &[],
        }
    }
}

/// Opaque view over one thing inside a validated result slot or relation role.
#[napi]
pub struct NodeValidatedThingHandle {
    inner: NodeValidatedThing,
}

#[napi]
impl NodeValidatedThingHandle {
    #[napi]
    pub fn iid(&self) -> String {
        self.inner.concept_id().to_owned()
    }

    #[napi(js_name = "concreteDescriptor")]
    pub fn concrete_descriptor(&self) -> String {
        self.inner.concrete_descriptor().to_owned()
    }

    #[napi(js_name = "thingKind")]
    pub fn thing_kind(&self) -> &'static str {
        match self.inner.kind() {
            ThingKind::Entity => "entity",
            ThingKind::Relation => "relation",
        }
    }

    #[napi(js_name = "fieldNames")]
    pub fn field_names(&self) -> Vec<String> {
        self.inner
            .attributes()
            .iter()
            .map(|attribute| attribute.field().name.clone())
            .collect()
    }

    #[napi(js_name = "fieldValuesJson")]
    pub fn field_values_json(&self, field_name: String) -> Result<Option<String>> {
        let Some(attribute) = self
            .inner
            .attributes()
            .iter()
            .find(|attribute| attribute.field().name == field_name)
        else {
            return Ok(None);
        };
        let values: Vec<_> = attribute
            .values()
            .iter()
            .map(crate::attribute_value_to_json)
            .collect();
        serde_json::to_string(&values).map(Some).map_err(|error| {
            result_decode_error(
                "attribute_materialization_failed",
                format!("validated attribute values could not be encoded: {error}"),
            )
        })
    }

    #[napi(js_name = "roleDataComplete")]
    pub fn role_data_complete(&self) -> bool {
        self.inner.role_data_complete()
    }

    #[napi(js_name = "roleNames")]
    pub fn role_names(&self) -> Vec<String> {
        self.inner
            .roles()
            .iter()
            .map(|role| role.role().name.clone())
            .collect()
    }

    #[napi(js_name = "rolePlayerCount")]
    pub fn role_player_count(&self, role_name: String) -> Result<u32> {
        let count = self
            .inner
            .roles()
            .iter()
            .find(|role| role.role().name == role_name)
            .map_or(0, |role| role.players().len());
        u32::try_from(count).map_err(|_| {
            result_decode_error(
                "role_player_count_overflow",
                "validated role-player count cannot be represented by the Node binding",
            )
        })
    }

    #[napi(js_name = "rolePlayer")]
    pub fn role_player(
        &self,
        role_name: String,
        player_index: u32,
    ) -> Result<NodeValidatedThingHandle> {
        let player = self
            .inner
            .roles()
            .iter()
            .find(|role| role.role().name == role_name)
            .and_then(|role| role.players().get(player_index as usize))
            .ok_or_else(|| {
                result_decode_error(
                    "role_player_out_of_bounds",
                    "role-player index is outside the validated relation result",
                )
            })?;
        Ok(NodeValidatedThingHandle {
            inner: NodeValidatedThing::RolePlayer(player.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use type_bridge_orm::session::backend::{
        AnswerCancellation, BoundedAnswerLimits, BoundedAnswerReader, BoxFuture, DriverBackend,
        TransactionOps, TxType,
    };
    use type_bridge_orm::{CapabilitySet, Database, OrmError};

    struct ProviderOpenFailureBackend;

    impl DriverBackend for ProviderOpenFailureBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async {
                Err(OrmError::Connection(
                    "credential=node-binding-secret".into(),
                ))
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    fn assert_send_sync<T: Send + Sync>() {}

    fn registry() -> NodeDescriptorRegistry {
        let registry = NodeDescriptorRegistry::new();
        for (type_name, attr_name) in [("person", "person-name"), ("company", "company-name")] {
            registry
                .register_entity_json(format!(
                    r#"{{
                        "type_name":"{type_name}",
                        "is_abstract":false,
                        "parent_type":null,
                        "owned_attributes":[{{
                            "field_name":"name",
                            "attr_name":"{attr_name}",
                            "value_type":"string",
                            "annotations":["Key"],
                            "is_optional":false,
                            "is_ordered":false
                        }}]
                    }}"#
                ))
                .unwrap();
        }
        registry
            .register_relation_json(
                r#"{
                    "type_name":"employment",
                    "is_abstract":false,
                    "parent_type":null,
                    "owned_attributes":[],
                    "roles":[
                        {
                            "role_name":"employee",
                            "player_type_names":["person"],
                            "cardinality":[1,1],
                            "overrides":null,
                            "is_abstract":false,
                            "ordered":false,
                            "distinct":false,
                            "plays_cardinality":null
                        },
                        {
                            "role_name":"employer",
                            "player_type_names":["company"],
                            "cardinality":[1,1],
                            "overrides":null,
                            "is_abstract":false,
                            "ordered":false,
                            "distinct":false,
                            "plays_cardinality":null
                        }
                    ]
                }"#
                .into(),
            )
            .unwrap();
        registry
    }

    #[test]
    fn opaque_wrapper_types_are_send_and_sync() {
        assert_send_sync::<NodeMatchSessionHandle>();
        assert_send_sync::<NodeMatchBindingHandle>();
        assert_send_sync::<NodeMatchFieldHandle>();
        assert_send_sync::<NodeMatchRoleHandle>();
        assert_send_sync::<NodeMatchPredicateHandle>();
        assert_send_sync::<NodeMatchOrderHandle>();
        assert_send_sync::<NodeMatchSelectionHandle>();
        assert_send_sync::<NodeMatchShapeHandle>();
        assert_send_sync::<NodeMatchQueryHandle>();
        assert_send_sync::<NodeValidatedMatchResultHandle>();
        assert_send_sync::<NodeValidatedThingHandle>();
    }

    #[test]
    fn bigint_windows_are_lossless_and_reject_negative_or_wide_values() {
        assert_eq!(
            bigint_u64(
                &BigInt {
                    sign_bit: false,
                    words: vec![u64::MAX],
                },
                "limit",
            )
            .unwrap(),
            u64::MAX
        );
        assert!(
            bigint_u64(
                &BigInt {
                    sign_bit: true,
                    words: vec![1],
                },
                "offset",
            )
            .is_err()
        );
        assert!(
            bigint_u64(
                &BigInt {
                    sign_bit: false,
                    words: vec![0, 1],
                },
                "limit",
            )
            .is_err()
        );
    }

    #[test]
    fn match_errors_are_structured_without_display_parsing() {
        let registry = NodeDescriptorRegistry::new();
        let session = NodeMatchSessionHandle::new(&registry);
        let error = match session.exact("missing".into()) {
            Err(error) => error,
            Ok(_) => panic!("unknown descriptors must fail"),
        };
        let payload: Value = serde_json::from_str(&error.reason).unwrap();

        assert_eq!(error.status, Status::InvalidArg);
        assert_eq!(payload["category"], "invalid_plan");
        assert_eq!(payload["code"], "unknown_descriptor");
        assert!(payload["message"].as_str().unwrap().contains("missing"));
        assert!(payload["path"].is_array());
        assert!(payload["details"].is_object());
    }

    #[test]
    fn node_marshalling_preserves_timeout_cancel_and_provider_diagnostics() {
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let cancelled = BoundedAnswerReader::new(BoundedAnswerLimits {
            cancellation,
            ..BoundedAnswerLimits::default()
        })
        .check_before_read()
        .unwrap_err();
        let timed_out = BoundedAnswerReader::new(BoundedAnswerLimits {
            deadline: Instant::now().checked_sub(Duration::from_secs(1)),
            ..BoundedAnswerLimits::default()
        })
        .check_before_read()
        .unwrap_err();

        let registry = registry().shared_registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let validated = session
            .query(shape)
            .unwrap()
            .validate_count_by(&person)
            .unwrap();
        let database = Database::with_backend(Box::new(ProviderOpenFailureBackend), "test");
        let provider = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(database.execute_match(&registry, &validated))
            .unwrap_err();

        for (error, status, category, code) in [
            (
                cancelled,
                Status::InvalidArg,
                "resource_limit",
                "provider_cancelled",
            ),
            (
                timed_out,
                Status::InvalidArg,
                "resource_limit",
                "transaction_deadline_exceeded",
            ),
            (
                provider,
                Status::GenericFailure,
                "provider",
                "provider_transaction_open_failed",
            ),
        ] {
            let error = crate::napi_orm_error(error);
            let payload: Value = serde_json::from_str(&error.reason).unwrap();
            assert_eq!(error.status, status);
            assert_eq!(payload["category"], category);
            assert_eq!(payload["code"], code);
            assert_eq!(payload["path"], json!([{"kind": "provider_evidence"}]));
            assert!(!error.reason.contains("node-binding-secret"));
        }
    }

    #[test]
    fn enum_inputs_use_only_canonical_stable_spellings() {
        assert_eq!(parse_comparison("equal").unwrap(), ComparisonOp::Equal);
        assert_eq!(
            parse_direction("descending").unwrap(),
            SortDirection::Descending
        );
        assert_eq!(parse_missing("reject").unwrap(), MissingOrder::Reject);
        assert_eq!(
            parse_cardinality("bounded_many").unwrap(),
            RowCardinality::BoundedMany
        );
        assert!(parse_comparison("=").is_err());
        assert!(parse_direction("desc").is_err());
        for (name, expected) in [
            ("count", Reduction::Count),
            ("sum", Reduction::Sum),
            ("min", Reduction::Min),
            ("max", Reduction::Max),
            ("mean", Reduction::Mean),
            ("median", Reduction::Median),
            ("std", Reduction::Std),
        ] {
            assert_eq!(parse_reduction(name).unwrap(), expected);
        }
        assert!(parse_reduction("variance").is_err());
    }

    #[test]
    fn opaque_handle_graph_emits_and_revalidates_one_canonical_diagnostic() {
        let registry = registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let company = session.exact("company".into()).unwrap();
        let employment = session.exact("employment".into()).unwrap();

        let shape = NodeMatchShapeHandle {
            inner: session
                .inner
                .positional([person.inner.one(), company.inner.one()])
                .unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One, NodeOutputSlotKind::One],
            }),
        };
        let base = session.query(&shape).unwrap();
        let attached = base.add_hidden(&employment).unwrap();
        let employee = employment
            .role("employee".into())
            .unwrap()
            .connects(&person)
            .unwrap();
        let employer = employment
            .role("employer".into())
            .unwrap()
            .connects(&company)
            .unwrap();
        let connected = employee.and_predicate(&employer).unwrap();
        let filtered = attached.where_predicate(&connected).unwrap();
        let order = person
            .field("name".into())
            .unwrap()
            .order("ascending".into(), "reject".into())
            .unwrap();
        let request = filtered
            .inner
            .fetch_rows(
                &[order.inner],
                Window {
                    offset: 0,
                    limit: 25,
                },
                RowCardinality::BoundedMany,
            )
            .unwrap();
        let canonical = diagnostic(request).unwrap();
        let revalidated = revalidate_match_diagnostic(&registry, canonical.clone()).unwrap();

        assert_eq!(canonical, revalidated);
        let wire: Value = serde_json::from_str(&canonical).unwrap();
        assert_eq!(wire["request"]["plan"]["bindings"][0]["id"], 0);
        assert_eq!(wire["request"]["plan"]["bindings"][1]["id"], 1);
        assert_eq!(wire["request"]["plan"]["bindings"][2]["id"], 2);
        assert_eq!(wire["request"]["operation"]["output"]["kind"], "positional");
        assert!(canonical.contains("EXACT_ENTITY_TARGET"));
        assert!(canonical.contains("EXACT_RELATION_TARGET"));

        let base_request = base.inner.count_by(&person.inner).unwrap();
        assert_eq!(base_request.plan.bindings.len(), 2);
        assert!(base_request.plan.predicate.is_none());
    }

    #[test]
    fn validated_result_handle_rejects_foreign_lineages_before_slot_access() {
        let registry = registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let shape = NodeMatchShapeHandle {
            inner: session.inner.positional([person.inner.one()]).unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
        };
        let query = session.query(&shape).unwrap();
        let foreign = session.query(&shape).unwrap();
        let validated = query
            .inner
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 10,
                },
                RowCardinality::BoundedMany,
            )
            .unwrap();
        let mut executor = type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        executor.push(type_bridge_orm::RecordingMatchResponse::EmptyRows);
        let inner = executor.execute(&validated).unwrap();
        let result = NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };

        assert_eq!(result.row_count(&query).unwrap(), 0);
        let error = result.row_count(&foreign).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["category"], "result_decode");
        assert_eq!(payload["code"], "result_query_mismatch");

        let error = result.slot_count(&query, 0).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_row_out_of_bounds");
    }

    #[test]
    fn page_count_and_exists_accessors_require_exact_invocation_proofs() {
        let registry = registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let shape = NodeMatchShapeHandle {
            inner: session.inner.positional([person.inner.one()]).unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
        };
        let query = session.query(&shape).unwrap();
        let order = person
            .field("name".into())
            .unwrap()
            .order("ascending".into(), "reject".into())
            .unwrap();

        let page_request = query
            .inner
            .validate_page_by(
                &person.inner,
                &[order.inner],
                Window {
                    offset: 0,
                    limit: 10,
                },
                true,
            )
            .unwrap();
        let mut page_executor =
            type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        page_executor.push(type_bridge_orm::RecordingMatchResponse::EmptyPage { total: Some(0) });
        let page_inner = page_executor.execute(&page_request).unwrap();
        let page = NodeValidatedMatchResultHandle {
            inner: page_inner,
            request: page_request,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };
        assert_eq!(page.page_entry_count(&query).unwrap(), 0);
        assert_eq!(page.output_slot_count(&query).unwrap(), 1);
        assert!(!page.output_slot_is_collection(&query, 0).unwrap());
        assert_eq!(
            bigint_u64(&page.page_offset(&query).unwrap(), "offset").unwrap(),
            0
        );
        assert_eq!(
            bigint_u64(&page.page_limit(&query).unwrap(), "limit").unwrap(),
            10
        );
        assert_eq!(
            bigint_u64(&page.page_total(&query).unwrap().unwrap(), "total").unwrap(),
            0
        );
        let error = page.count_value(&query).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_operation_mismatch");

        let count_request = query.inner.validate_count_by(&person.inner).unwrap();
        let mut count_executor =
            type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        count_executor.push(type_bridge_orm::RecordingMatchResponse::Count(u64::MAX));
        let count_inner = count_executor.execute(&count_request).unwrap();
        let count = NodeValidatedMatchResultHandle {
            inner: count_inner,
            request: count_request,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };
        assert_eq!(
            bigint_u64(&count.count_value(&query).unwrap(), "count").unwrap(),
            u64::MAX
        );

        let exists_request = query.inner.validate_exists_by(&person.inner).unwrap();
        let mut exists_executor =
            type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        exists_executor.push(type_bridge_orm::RecordingMatchResponse::Exists(true));
        let exists_inner = exists_executor.execute(&exists_request).unwrap();
        let exists = NodeValidatedMatchResultHandle {
            inner: exists_inner,
            request: exists_request,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };
        assert!(exists.exists_value(&query).unwrap());

        let original_request = query.inner.validate_count_by(&person.inner).unwrap();
        let foreign_request = query.inner.validate_count_by(&person.inner).unwrap();
        let mut token_executor =
            type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        token_executor.push(type_bridge_orm::RecordingMatchResponse::Count(1));
        let inner = token_executor.execute(&original_request).unwrap();
        let mismatched = NodeValidatedMatchResultHandle {
            inner,
            request: foreign_request,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };
        let error = mismatched.count_value(&query).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["category"], "result_decode");
        assert_eq!(payload["code"], "request_token_mismatch");
    }

    fn reduction_registry() -> NodeDescriptorRegistry {
        let registry = NodeDescriptorRegistry::new();
        registry
            .register_entity_json(
                r#"{
                    "type_name":"person",
                    "is_abstract":false,
                    "parent_type":null,
                    "owned_attributes":[
                        {
                            "field_name":"name",
                            "attr_name":"person-name",
                            "value_type":"string",
                            "annotations":["Key"],
                            "is_optional":false,
                            "is_ordered":false
                        },
                        {
                            "field_name":"age",
                            "attr_name":"person-age",
                            "value_type":"long",
                            "annotations":[{"Card":[0,1]}],
                            "is_optional":true,
                            "is_ordered":false
                        }
                    ]
                }"#
                .into(),
            )
            .unwrap();
        registry
            .register_entity_json(
                r#"{
                    "type_name":"team",
                    "is_abstract":false,
                    "parent_type":null,
                    "owned_attributes":[{
                        "field_name":"name",
                        "attr_name":"team-name",
                        "value_type":"string",
                        "annotations":["Key"],
                        "is_optional":false,
                        "is_ordered":false
                    }]
                }"#
                .into(),
            )
            .unwrap();
        registry
    }

    #[test]
    fn reduction_accessors_expose_typed_domains_behind_exact_invocation_proofs() {
        let registry = reduction_registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let shape = NodeMatchShapeHandle {
            inner: session.inner.positional([person.inner.one()]).unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
        };
        let query = session.query(&shape).unwrap();
        let age = person.inner.field("age").unwrap();
        let validated = query
            .inner
            .validate_reduce_by(
                &person.inner,
                None,
                &[
                    (Reduction::Count, None),
                    (Reduction::Sum, Some(&age)),
                    (Reduction::Mean, Some(&age)),
                ],
            )
            .unwrap();
        let mut executor = type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        executor.push(type_bridge_orm::RecordingMatchResponse::Reduction(vec![
            ReducedValue::Count(2),
            ReducedValue::Long(Some(70)),
            ReducedValue::Double(Some(35.0)),
        ]));
        let inner = executor.execute(&validated).unwrap();
        let result = NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };

        assert_eq!(result.reduction_row_count(&query).unwrap(), 1);
        assert_eq!(result.reduction_value_count(&query, 0).unwrap(), 3);
        assert_eq!(result.reduction_value_kind(&query, 0, 0).unwrap(), "count");
        assert_eq!(result.reduction_value_kind(&query, 0, 1).unwrap(), "long");
        assert_eq!(result.reduction_value_kind(&query, 0, 2).unwrap(), "double");
        assert_eq!(
            bigint_u64(
                &result.reduction_count_value(&query, 0, 0).unwrap(),
                "count"
            )
            .unwrap(),
            2
        );
        assert_eq!(
            bigint_u64(
                &result.reduction_long_value(&query, 0, 1).unwrap().unwrap(),
                "sum",
            )
            .unwrap(),
            70
        );
        assert_eq!(
            result.reduction_double_value(&query, 0, 2).unwrap(),
            Some(35.0)
        );

        let error = result.reduction_count_value(&query, 0, 1).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_reduction_value_kind_mismatch");

        let error = result.reduction_group(&query, 0).err().unwrap();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_reduction_ungrouped");

        let error = result.reduction_value_kind(&query, 0, 3).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_reduction_value_out_of_bounds");

        let error = result.count_value(&query).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_operation_mismatch");

        let foreign = session.query(&shape).unwrap();
        let error = result.reduction_row_count(&foreign).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_query_mismatch");
    }

    #[test]
    fn empty_grouped_reductions_expose_zero_rows_and_bounded_row_access() {
        let registry = reduction_registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let team = session.exact("team".into()).unwrap();
        let shape = NodeMatchShapeHandle {
            inner: session.inner.positional([person.inner.one()]).unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
        };
        let query = session
            .query(&shape)
            .unwrap()
            .add_hidden(&team)
            .unwrap()
            .allow_cross_join(&person, &team)
            .unwrap();
        let validated = query
            .inner
            .validate_reduce_by(
                &person.inner,
                Some(&team.inner),
                &[(Reduction::Count, None)],
            )
            .unwrap();
        let mut executor = type_bridge_orm::RecordingMatchExecutor::new(query.inner.registry_arc());
        executor.push(type_bridge_orm::RecordingMatchResponse::EmptyGroupedReduction);
        let inner = executor.execute(&validated).unwrap();
        let result = NodeValidatedMatchResultHandle {
            inner,
            request: validated,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
        };

        assert_eq!(result.reduction_row_count(&query).unwrap(), 0);
        let error = result.reduction_value_count(&query, 0).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_reduction_row_out_of_bounds");
    }
}
