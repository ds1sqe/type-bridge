//! Opaque N-API seam for canonical match-request construction.
//!
//! JavaScript receives persistent native handles and deterministic diagnostic
//! strings only. It never owns a semantic request DTO, plan-local identity, or
//! live request token.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{FunctionId, TypeId};
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    AnswerCancellation, BindingHandle, ComparisonOp, DescriptorId, FieldHandle,
    FunctionArgumentHandle, FunctionCallHandle, FunctionHandle, FunctionValueHandle,
    HydratedAttribute, HydratedRole, HydratedRolePlayer, HydratedThing, InstalledRuntimeProjection,
    MatchError, MatchMode, MatchRequest, MatchResult, MissingOrder, OrderHandle, PredicateHandle,
    ProjectedAttributeValue, ProjectedQueryOrigin, ProjectedQuerySlotValue, ProjectedQueryValue,
    ProjectedReductionGroup, ProjectedThing, ProviderRuntimeOwner, QueryExecutionDeadline,
    QueryExecutionResourceLimits, QueryHandle, QueryTarget, QueryTargetError, ReducedValue,
    Reduction, ReductionRow, RoleHandle, RowCardinality, SelectionHandle, SessionHandle,
    ShapeHandle, SlotValue, SortDirection, ThingKind, UnknownEnumValue, UnvalidatedMatchRequest,
    ValidatedMatchRequest, ValidatedMatchResult, Window, lower_match_error,
    query_resource_closed_diagnostic,
};

#[cfg(test)]
use crate::NodeDescriptorRegistry;
use crate::{NodeRustDatabase, NodeRustTransactionContext};

pub(crate) fn napi_match_error(error: MatchError) -> napi::Error {
    napi_sdk_diagnostic(lower_match_error(&error))
}

/// Lower one terminal failure to the error its direct or remote half raised
/// before the shared executor existed.
pub(crate) fn napi_target_error(error: QueryTargetError) -> napi::Error {
    match error {
        QueryTargetError::Direct(error) => crate::napi_orm_error(error),
        QueryTargetError::Prepare(error) | QueryTargetError::Reply(error) => {
            crate::query_v2_model_remote_runtime::remote_model_error(error)
        }
        QueryTargetError::Interrupted(diagnostic) => napi_sdk_diagnostic(diagnostic),
    }
}

/// Raise `diagnostic` as a native error whose reason is its canonical host
/// JSON form.
pub(crate) fn napi_sdk_diagnostic(diagnostic: SdkExecutionDiagnostic) -> napi::Error {
    let status = match diagnostic.category() {
        SdkDiagnosticCategory::InvalidInput | SdkDiagnosticCategory::ResourceLimit => {
            Status::InvalidArg
        }
        SdkDiagnosticCategory::Cancelled => Status::Cancelled,
        _ => Status::GenericFailure,
    };
    let reason = serde_json::to_string(&diagnostic.host_json())
        .expect("the canonical SDK diagnostic form always encodes");
    Error::new(status, reason)
}

fn invalid_arg(message: impl Into<String>) -> napi::Error {
    Error::new(Status::InvalidArg, message.into())
}

fn result_decode_error(code: &'static str, _message: impl Into<String>) -> napi::Error {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCode, SdkQueryDiagnosticCategory, SdkQueryDiagnosticPathKind,
    };

    let diagnostic = SdkExecutionDiagnostic::query_failure(
        SdkQueryDiagnosticCategory::ResultDecode,
        SdkDiagnosticCode::new(code).expect("static result decode code is canonical"),
    )
    .try_at(SdkDiagnosticPathSegment::Query(
        SdkQueryDiagnosticPathKind::Result,
    ))
    .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure());
    napi_sdk_diagnostic(diagnostic)
}

/// Parse `value` with the core enum's one parser; an unknown spelling is the
/// structured `unknown_enum_value` diagnostic.
pub(crate) fn parse_enum<T>(value: &str) -> Result<T>
where
    T: std::str::FromStr<Err = UnknownEnumValue>,
{
    value
        .parse()
        .map_err(|error: UnknownEnumValue| napi_sdk_diagnostic(error.into()))
}

/// Check `value` against the core enum named by `kind`; return its canonical
/// spelling.
#[napi(js_name = "canonicalEnumSpelling")]
pub fn canonical_enum_spelling(kind: String, value: String) -> Result<String> {
    type_bridge_orm::canonical_enum_spelling(&kind, &value)
        .map(str::to_owned)
        .map_err(|error| napi_sdk_diagnostic(error.into()))
}

pub(crate) fn reduce_terms(
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
                parse_enum::<Reduction>(reducer)?,
                input.as_ref().map(|field| field.inner.clone()),
            ))
        })
        .collect()
}

pub(crate) fn borrow_reduce_terms(
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

/// Marshal a JavaScript `bigint` window value that arrives untyped.
pub(crate) fn unknown_u64(value: Unknown, name: &str) -> Result<u64> {
    let value =
        BigInt::from_unknown(value).map_err(|_| invalid_arg(format!("{name} must be a bigint")))?;
    bigint_u64(&value, name)
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
#[cfg(test)]
pub(crate) fn revalidate_match_diagnostic(
    registry: &NodeDescriptorRegistry,
    diagnostic_json: String,
) -> Result<String> {
    revalidate_diagnostic(registry.shared_registry().as_ref(), &diagnostic_json)
}

pub(crate) fn revalidate_diagnostic(
    registry: &DescriptorRegistry,
    diagnostic_json: &str,
) -> Result<String> {
    let unvalidated = UnvalidatedMatchRequest::from_canonical_bytes(diagnostic_json.as_bytes())
        .map_err(napi_match_error)?;
    let canonical = unvalidated.to_canonical_bytes().map_err(napi_match_error)?;
    unvalidated.validate(registry).map_err(napi_match_error)?;
    String::from_utf8(canonical)
        .map_err(|_| Error::new(Status::GenericFailure, "diagnostic JSON was not UTF-8"))
}

/// Query limits one generated session validates, encodes, and executes
/// under.
///
/// Constructed from parallel name and value arrays of overrides of the
/// canonical defaults. Rust admits them: an unknown name or an unsupported
/// value is refused here, before any session exists.
#[napi]
pub struct NodeQueryLimits {
    inner: QueryLimits,
}

impl NodeQueryLimits {
    pub(crate) const fn inner(&self) -> QueryLimits {
        self.inner
    }
}

#[napi]
impl NodeQueryLimits {
    #[napi(constructor)]
    pub fn new(names: Vec<String>, values: Vec<BigInt>) -> Result<Self> {
        if names.len() != values.len() {
            return Err(Error::new(
                Status::InvalidArg,
                "query limit names and values must have the same length",
            ));
        }
        let mut named = Vec::with_capacity(names.len());
        for (name, value) in names.iter().zip(&values) {
            named.push((name.as_str(), bigint_u64(value, name)?));
        }
        QueryLimits::from_overrides(named)
            .map(|inner| Self { inner })
            .map_err(|error| Error::new(Status::InvalidArg, error.to_string()))
    }

    /// Return the stable names of every query limit.
    #[napi]
    pub fn names(&self) -> Vec<String> {
        QueryLimits::FIELDS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    }

    /// Return the effective value of one named limit.
    #[napi]
    pub fn get(&self, name: String) -> Result<BigInt> {
        self.inner
            .get(&name)
            .map(|value| u64_bigint(u64::try_from(value).unwrap_or(u64::MAX)))
            .ok_or_else(|| {
                Error::new(
                    Status::InvalidArg,
                    format!("`{name}` is not a known query limit"),
                )
            })
    }
}

/// Canonical cross-binding generated-query execution budgets.
#[napi]
pub struct NodeQueryExecutionResources {
    inner: QueryExecutionResourceLimits,
}

impl NodeQueryExecutionResources {
    pub(crate) const fn from_inner(inner: QueryExecutionResourceLimits) -> Self {
        Self { inner }
    }

    pub(crate) const fn inner(&self) -> QueryExecutionResourceLimits {
        self.inner
    }
}

#[napi]
impl NodeQueryExecutionResources {
    /// Construct an answer policy. An omitted dimension keeps its core
    /// default; a given one may tighten or raise it, and zero is valid. A
    /// value above a dimension's supported maximum is refused by name, never
    /// clamped.
    #[napi(constructor)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the common policy has eight explicit dimensions"
    )]
    pub fn new(
        timeout_milliseconds: Option<BigInt>,
        items: Option<BigInt>,
        bytes: Option<BigInt>,
        graph_nodes: Option<BigInt>,
        attribute_values: Option<BigInt>,
        collection_members: Option<BigInt>,
        role_players: Option<BigInt>,
        statements: Option<BigInt>,
    ) -> Result<Self> {
        let defaults = QueryExecutionResourceLimits::default();
        let dimension = |value: Option<BigInt>, default: u64, name: &str| {
            value.map_or(Ok(default), |value| bigint_u64(&value, name))
        };
        let statements = dimension(statements, u64::from(defaults.statements), "statements")?;
        let inner = QueryExecutionResourceLimits {
            timeout_milliseconds: dimension(
                timeout_milliseconds,
                defaults.timeout_milliseconds,
                "timeoutMilliseconds",
            )?,
            items: dimension(items, defaults.items, "items")?,
            bytes: dimension(bytes, defaults.bytes, "bytes")?,
            graph_nodes: dimension(graph_nodes, defaults.graph_nodes, "graphNodes")?,
            attribute_values: dimension(
                attribute_values,
                defaults.attribute_values,
                "attributeValues",
            )?,
            collection_members: dimension(
                collection_members,
                defaults.collection_members,
                "collectionMembers",
            )?,
            role_players: dimension(role_players, defaults.role_players, "rolePlayers")?,
            statements: u32::try_from(statements).unwrap_or(u32::MAX),
        }
        .validate()
        .map_err(|refusal| invalid_arg(refusal.to_string()))?;
        Ok(Self { inner })
    }

    #[napi(getter, js_name = "timeoutMilliseconds")]
    pub fn timeout_milliseconds(&self) -> BigInt {
        u64_bigint(self.inner.timeout_milliseconds)
    }

    #[napi(getter)]
    pub fn items(&self) -> BigInt {
        u64_bigint(self.inner.items)
    }

    #[napi(getter)]
    pub fn bytes(&self) -> BigInt {
        u64_bigint(self.inner.bytes)
    }

    #[napi(getter, js_name = "graphNodes")]
    pub fn graph_nodes(&self) -> BigInt {
        u64_bigint(self.inner.graph_nodes)
    }

    #[napi(getter, js_name = "attributeValues")]
    pub fn attribute_values(&self) -> BigInt {
        u64_bigint(self.inner.attribute_values)
    }

    #[napi(getter, js_name = "collectionMembers")]
    pub fn collection_members(&self) -> BigInt {
        u64_bigint(self.inner.collection_members)
    }

    #[napi(getter, js_name = "rolePlayers")]
    pub fn role_players(&self) -> BigInt {
        u64_bigint(self.inner.role_players)
    }

    #[napi(getter)]
    pub fn statements(&self) -> BigInt {
        u64_bigint(u64::from(self.inner.statements))
    }
}

/// Caller-owned cooperative cancellation shared by direct execution, remote
/// preparation/reconstruction, and generated result materialization.
#[napi]
pub struct NodeQueryCancellation {
    inner: AnswerCancellation,
}

impl NodeQueryCancellation {
    pub(crate) fn inner(&self) -> AnswerCancellation {
        self.inner.clone()
    }
}

#[napi]
impl NodeQueryCancellation {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: AnswerCancellation::default(),
        }
    }

    #[napi]
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    #[napi(getter, js_name = "isCancelled")]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl Default for NodeQueryCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Opaque owner of one native match-handle construction session.
#[napi]
pub struct NodeMatchSessionHandle {
    inner: SessionHandle,
    installed: Option<Arc<InstalledRuntimeProjection>>,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
    closed: Arc<AtomicBool>,
}

impl NodeMatchSessionHandle {
    #[cfg(test)]
    pub(crate) fn from_registry(registry: Arc<DescriptorRegistry>) -> Self {
        Self::from_registry_with_resources(
            registry,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_registry_with_resources(
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            inner: SessionHandle::new(registry),
            installed: None,
            resources: resources.effective(),
            cancellation,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn from_installed(
        installed: Arc<InstalledRuntimeProjection>,
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
        query_limits: QueryLimits,
    ) -> Self {
        Self {
            inner: SessionHandle::with_limits(registry, query_limits),
            installed: Some(installed),
            resources: resources.effective(),
            cancellation,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(registry: &NodeDescriptorRegistry) -> Self {
        Self::from_registry(registry.shared_registry())
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(napi_sdk_diagnostic(query_resource_closed_diagnostic()));
        }
        Ok(())
    }
}

fn supports_projected_query_companion(installed: &InstalledRuntimeProjection) -> bool {
    installed.projection().models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
}

#[napi]
impl NodeMatchSessionHandle {
    #[napi]
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    #[napi(getter, js_name = "isClosed")]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    #[napi]
    pub fn exact(&self, type_name: String) -> Result<NodeMatchBindingHandle> {
        self.ensure_open()?;
        self.inner
            .exact(&type_name)
            .map(|inner| NodeMatchBindingHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn subtypes(&self, type_name: String) -> Result<NodeMatchBindingHandle> {
        self.ensure_open()?;
        self.inner
            .subtypes(&type_name)
            .map(|inner| NodeMatchBindingHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    /// Bind `type_name` with a match mode spelled `exact` or `subtypes`; any
    /// other spelling is the core's `unknown_enum_value` diagnostic.
    #[napi]
    pub fn binding(&self, type_name: String, match_mode: String) -> Result<NodeMatchBindingHandle> {
        self.ensure_open()?;
        let match_mode: MatchMode = parse_enum(&match_mode)?;
        self.inner
            .binding(&type_name, match_mode)
            .map(|inner| NodeMatchBindingHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "functionById")]
    pub fn function_by_id(&self, function_id: String) -> Result<NodeMatchFunctionHandle> {
        self.ensure_open()?;
        let id = FunctionId::new(function_id).map_err(|error| invalid_arg(error.to_string()))?;
        self.inner
            .function(&id)
            .map(|inner| NodeMatchFunctionHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "functionValueJson")]
    pub fn function_value_json(
        &self,
        attribute_type_key: String,
        value_json: String,
    ) -> Result<NodeMatchFunctionValueHandle> {
        self.ensure_open()?;
        let installed = self.installed.as_deref().ok_or_else(|| {
            invalid_arg("this match session has no installed function projection authority")
        })?;
        let attribute_type: TypeId =
            serde_json::from_str(&attribute_type_key).map_err(|error| {
                invalid_arg(format!("invalid function attribute identity: {error}"))
            })?;
        let canonical =
            to_canonical_json(&attribute_type).map_err(|error| invalid_arg(error.to_string()))?;
        if canonical != attribute_type_key.as_bytes() {
            return Err(invalid_arg("function attribute identity is not canonical"));
        }
        let value: Value = serde_json::from_str(&value_json)
            .map_err(|error| invalid_arg(format!("invalid function scalar JSON: {error}")))?;
        let value = crate::attribute_value_from_js(&value, None)?;
        let projected =
            ProjectedAttributeValue::try_from_attribute_value(installed, attribute_type, &value)
                .map_err(napi_sdk_diagnostic)?;
        self.inner
            .function_value(&projected)
            .map(|inner| NodeMatchFunctionValueHandle { inner })
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
        self.ensure_open()?;
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
        self.ensure_open()?;
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
        self.ensure_open()?;
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
        self.ensure_open()?;
        self.inner
            .query(shape.inner.clone())
            .map(|inner| NodeMatchQueryHandle {
                inner,
                installed: self
                    .installed
                    .as_ref()
                    .filter(|installed| supports_projected_query_companion(installed))
                    .map(Arc::clone),
                output: Arc::clone(&shape.output),
                lineage: Arc::new(()),
                resources: self.resources,
                cancellation: self.cancellation.clone(),
                session_closed: Arc::clone(&self.closed),
                closed: AtomicBool::new(false),
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
    pub fn iid(&self, iid: String) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .iid(iid)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "iidIn")]
    pub fn iid_in(&self, iids: Vec<String>) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .iid_in(iids)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

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

    #[napi(js_name = "functionArgument")]
    pub fn function_argument(&self) -> NodeMatchFunctionArgumentHandle {
        NodeMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }
}

/// One exact installed scalar schema-function token.
#[napi]
pub struct NodeMatchFunctionHandle {
    inner: FunctionHandle,
}

#[napi]
impl NodeMatchFunctionHandle {
    #[napi(js_name = "call")]
    pub fn call(
        &self,
        arguments: Vec<Reference<NodeMatchFunctionArgumentHandle>>,
    ) -> Result<NodeMatchFunctionCallHandle> {
        self.inner
            .call(arguments.iter().map(|argument| argument.inner.clone()))
            .map(|inner| NodeMatchFunctionCallHandle { inner })
            .map_err(crate::napi_orm_error)
    }
}

/// One exact projection-branded scalar schema-function value.
#[napi]
pub struct NodeMatchFunctionValueHandle {
    inner: FunctionValueHandle,
}

#[napi]
impl NodeMatchFunctionValueHandle {
    #[napi(js_name = "functionArgument")]
    pub fn function_argument(&self) -> NodeMatchFunctionArgumentHandle {
        NodeMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }
}

/// One immutable session-branded schema-function argument.
#[napi]
pub struct NodeMatchFunctionArgumentHandle {
    inner: FunctionArgumentHandle,
}

/// One immutable session-branded scalar schema-function call.
#[napi]
pub struct NodeMatchFunctionCallHandle {
    inner: FunctionCallHandle,
}

#[napi]
impl NodeMatchFunctionCallHandle {
    #[napi(js_name = "functionArgument")]
    pub fn function_argument(&self) -> NodeMatchFunctionArgumentHandle {
        NodeMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }

    #[napi(js_name = "compareField")]
    pub fn compare_field(
        &self,
        comparison: String,
        field: &NodeMatchFieldHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .compare_field(parse_enum::<ComparisonOp>(&comparison)?, &field.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "compareValue")]
    pub fn compare_value(
        &self,
        comparison: String,
        value: &NodeMatchFunctionValueHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .compare_value(parse_enum::<ComparisonOp>(&comparison)?, &value.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi(js_name = "compareCall")]
    pub fn compare_call(
        &self,
        comparison: String,
        other: &NodeMatchFunctionCallHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .compare_call(parse_enum::<ComparisonOp>(&comparison)?, &other.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }
}

/// Opaque native bound-field handle.
#[napi]
pub struct NodeMatchFieldHandle {
    inner: FieldHandle,
}

impl NodeMatchFieldHandle {
    pub(crate) const fn inner(&self) -> &FieldHandle {
        &self.inner
    }
}

#[napi]
impl NodeMatchFieldHandle {
    #[napi]
    pub fn presence(&self, present: bool) -> NodeMatchPredicateHandle {
        NodeMatchPredicateHandle {
            inner: self.inner.presence(present),
        }
    }

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
                .compare_value(parse_enum::<ComparisonOp>(&comparison)?, value),
        })
    }

    #[napi(js_name = "compareField")]
    pub fn compare_field(
        &self,
        comparison: String,
        other: &NodeMatchFieldHandle,
    ) -> Result<NodeMatchPredicateHandle> {
        self.inner
            .compare_field(parse_enum::<ComparisonOp>(&comparison)?, &other.inner)
            .map(|inner| NodeMatchPredicateHandle { inner })
            .map_err(crate::napi_orm_error)
    }

    #[napi]
    pub fn order(&self, direction: String, missing: String) -> Result<NodeMatchOrderHandle> {
        Ok(NodeMatchOrderHandle {
            inner: self.inner.order(
                parse_enum::<SortDirection>(&direction)?,
                parse_enum::<MissingOrder>(&missing)?,
            ),
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
    installed: Option<Arc<InstalledRuntimeProjection>>,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
    session_closed: Arc<AtomicBool>,
    closed: AtomicBool,
}

impl NodeMatchQueryHandle {
    pub(crate) const fn inner(&self) -> &QueryHandle {
        &self.inner
    }

    pub(crate) fn begin_invocation(&self) -> Result<(QueryExecutionDeadline, AnswerCancellation)> {
        self.ensure_open()?;
        let deadline = QueryExecutionDeadline::for_limits(self.resources);
        deadline
            .check(&self.cancellation)
            .map_err(napi_sdk_diagnostic)?;
        Ok((deadline, self.cancellation.clone()))
    }

    pub(crate) fn result_context(
        &self,
        resources: QueryExecutionResourceLimits,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> NodeMatchResultContext {
        NodeMatchResultContext {
            installed: self.installed.as_ref().map(Arc::clone),
            output: Arc::clone(&self.output),
            lineage: Arc::clone(&self.lineage),
            resources,
            deadline,
            cancellation,
        }
    }

    /// The policy and deadline one terminal on `target` runs under: this
    /// session's resources and invocation deadline, constrained by the
    /// target connection's answer-limit ceiling. Execution and hydration
    /// share them, so neither can outrun the ceiling.
    pub(crate) fn terminal_budget(
        &self,
        target: &QueryTarget<'_>,
        deadline: QueryExecutionDeadline,
    ) -> (QueryExecutionResourceLimits, QueryExecutionDeadline) {
        target.budget(self.resources, deadline)
    }

    fn execute_owned(
        &self,
        env: &Env,
        database: &NodeRustDatabase,
        validated: ValidatedMatchRequest,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> Result<Object<'static>> {
        let (database, runtime) = database.handles();
        self.execute_direct(
            env,
            DirectTarget::Database(database),
            &runtime,
            validated,
            deadline,
            cancellation,
        )
    }

    fn execute_borrowed(
        &self,
        env: &Env,
        transaction: &NodeRustTransactionContext,
        validated: ValidatedMatchRequest,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> Result<Object<'static>> {
        let (transaction, runtime) = transaction.handles();
        self.execute_direct(
            env,
            DirectTarget::Transaction(transaction),
            &runtime,
            validated,
            deadline,
            cancellation,
        )
    }

    /// Run one terminal on a direct target through the shared executor on
    /// the provider runtime, then bind its hydration to the target's
    /// database. The returned Promise settles off the JavaScript thread, so
    /// a direct terminal never blocks the event loop.
    fn execute_direct(
        &self,
        env: &Env,
        target: DirectTarget,
        runtime: &Arc<ProviderRuntimeOwner>,
        validated: ValidatedMatchRequest,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> Result<Object<'static>> {
        let (resources, deadline) = self.terminal_budget(&target.query_target(), deadline);
        let registry = self.inner.registry_arc();
        let context = self.result_context(resources, deadline, cancellation.clone());
        crate::proxy_connection_runtime::settle(env, runtime, async move {
            let (request, result) = target
                .query_target()
                .execute(&registry, validated, resources, &cancellation, deadline)
                .await
                .map_err(napi_target_error)?;
            let origin = context
                .installed
                .as_ref()
                .map(|_| target.origin())
                .transpose()
                .map_err(napi_sdk_diagnostic)?;
            context.attach_with_origin(origin, request, result, registry)
        })
    }

    fn derived(&self, inner: QueryHandle) -> Self {
        Self {
            inner,
            installed: self.installed.as_ref().map(Arc::clone),
            output: Arc::clone(&self.output),
            lineage: Arc::new(()),
            resources: self.resources,
            cancellation: self.cancellation.clone(),
            session_closed: Arc::clone(&self.session_closed),
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) || self.session_closed.load(Ordering::Acquire) {
            return Err(napi_sdk_diagnostic(query_resource_closed_diagnostic()));
        }
        Ok(())
    }
}

#[napi]
impl NodeMatchQueryHandle {
    #[napi]
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    #[napi(getter, js_name = "isClosed")]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.session_closed.load(Ordering::Acquire)
    }

    /// Create an independently closable value-equivalent query handle.
    #[napi]
    pub fn fork(&self) -> Result<NodeMatchQueryHandle> {
        self.ensure_open()?;
        Ok(self.derived(self.inner.clone()))
    }

    #[napi(js_name = "addHidden")]
    pub fn add_hidden(&self, binding: &NodeMatchBindingHandle) -> Result<NodeMatchQueryHandle> {
        self.ensure_open()?;
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
        self.ensure_open()?;
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
        self.ensure_open()?;
        self.inner
            .allow_cross_join(&left.inner, &right.inner)
            .map(|inner| self.derived(inner))
            .map_err(crate::napi_orm_error)
    }

    /// Refuse group fields at `groupBy()` with the reduction terminal's
    /// diagnostic (`field_group_tuple_cap_exceeded` under `selected_slots`).
    #[napi(js_name = "checkGroupFields")]
    pub fn check_group_fields(&self, groups: Vec<Reference<NodeMatchFieldHandle>>) -> Result<()> {
        self.ensure_open()?;
        let groups = groups.iter().map(|group| &group.inner).collect::<Vec<_>>();
        self.inner
            .check_group_fields(&groups)
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
        self.ensure_open()?;
        let request = self
            .inner
            .fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_enum::<RowCardinality>(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        diagnostic(request)
    }

    #[napi(
        js_name = "executeFetchRowsOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_fetch_rows_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        cardinality: String,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_enum::<RowCardinality>(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeFetchRowsBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_fetch_rows_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        cardinality: String,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_fetch_rows(
                &order_handles(&orders),
                Window {
                    offset: bigint_u64(&offset, "offset")?,
                    limit: bigint_u64(&limit, "limit")?,
                },
                parse_enum::<RowCardinality>(&cardinality)?,
            )
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executePageByOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "terminal mirrors released grammar"
    )]
    pub fn execute_page_by_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        include_total: bool,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
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
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executePageByBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "terminal mirrors released grammar"
    )]
    pub fn execute_page_by_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        orders: Vec<Reference<NodeMatchOrderHandle>>,
        offset: BigInt,
        limit: BigInt,
        include_total: bool,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
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
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeCountByOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_count_by_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeCountByBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_count_by_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeExistsByOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_exists_by_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeExistsByBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_exists_by_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
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
        self.ensure_open()?;
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
        self.ensure_open()?;
        diagnostic(
            self.inner
                .count_by(&root.inner)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(js_name = "existsByDiagnostic")]
    pub fn exists_by_diagnostic(&self, root: &NodeMatchBindingHandle) -> Result<String> {
        self.ensure_open()?;
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
        self.ensure_open()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic(
            self.inner
                .reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(
        js_name = "executeReduceByOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeReduceByBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(&root.inner, group.map(|group| &group.inner), &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
    }

    #[napi(js_name = "reduceByFieldDiagnostic")]
    pub fn reduce_by_field_diagnostic(
        &self,
        root: &NodeMatchBindingHandle,
        group: &NodeMatchFieldHandle,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<String> {
        self.ensure_open()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic(
            self.inner
                .reduce_by_field(&root.inner, &group.inner, &terms)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(
        js_name = "executeReduceByFieldOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_field_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        group: &NodeMatchFieldHandle,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by_field(&root.inner, &group.inner, &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeReduceByFieldBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_field_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        group: &NodeMatchFieldHandle,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by_field(&root.inner, &group.inner, &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
    }

    #[napi(js_name = "reduceByFieldsDiagnostic")]
    pub fn reduce_by_fields_diagnostic(
        &self,
        root: &NodeMatchBindingHandle,
        groups: Vec<Reference<NodeMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<String> {
        self.ensure_open()?;
        let groups = groups.iter().map(|group| &group.inner).collect::<Vec<_>>();
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic(
            self.inner
                .reduce_by_fields(&root.inner, &groups, &terms)
                .map_err(crate::napi_orm_error)?,
        )
    }

    #[napi(
        js_name = "executeReduceByFieldsOwned",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_fields_owned(
        &self,
        env: Env,
        database: &NodeRustDatabase,
        root: &NodeMatchBindingHandle,
        groups: Vec<Reference<NodeMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let groups = groups.iter().map(|group| &group.inner).collect::<Vec<_>>();
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by_fields(&root.inner, &groups, &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_owned(&env, database, validated, deadline, cancellation)
    }

    #[napi(
        js_name = "executeReduceByFieldsBorrowed",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn execute_reduce_by_fields_borrowed(
        &self,
        env: Env,
        transaction: &NodeRustTransactionContext,
        root: &NodeMatchBindingHandle,
        groups: Vec<Reference<NodeMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> Result<Object<'static>> {
        let (deadline, cancellation) = self.begin_invocation()?;
        let groups = groups.iter().map(|group| &group.inner).collect::<Vec<_>>();
        let terms = reduce_terms(&reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by_fields(&root.inner, &groups, &terms)
            .map_err(crate::napi_orm_error)?;
        self.execute_borrowed(&env, transaction, validated, deadline, cancellation)
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
    registry: Arc<DescriptorRegistry>,
    projected: Option<Arc<ProjectedQueryValue>>,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

/// Immutable materialization metadata retained while a remote reply is decoded.
/// A direct target a Promise-returning terminal owns while it runs.
enum DirectTarget {
    Database(Arc<type_bridge_orm::Database>),
    Transaction(type_bridge_orm::TransactionContext),
}

impl DirectTarget {
    fn query_target(&self) -> QueryTarget<'_> {
        match self {
            Self::Database(database) => QueryTarget::Database(database),
            Self::Transaction(transaction) => QueryTarget::Transaction(transaction),
        }
    }

    fn origin(&self) -> std::result::Result<ProjectedQueryOrigin, SdkExecutionDiagnostic> {
        match self {
            Self::Database(database) => Ok(ProjectedQueryOrigin::for_database(database)),
            Self::Transaction(transaction) => ProjectedQueryOrigin::for_transaction(transaction),
        }
    }
}

#[derive(Clone)]
pub(crate) struct NodeMatchResultContext {
    installed: Option<Arc<InstalledRuntimeProjection>>,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
    resources: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl NodeMatchResultContext {
    pub(crate) fn attach(
        self,
        request: ValidatedMatchRequest,
        inner: ValidatedMatchResult,
        registry: Arc<DescriptorRegistry>,
    ) -> Result<NodeValidatedMatchResultHandle> {
        let origin = self
            .installed
            .as_ref()
            .map(|_| ProjectedQueryOrigin::remote_unbound());
        self.attach_with_origin(origin, request, inner, registry)
    }

    fn attach_with_origin(
        self,
        origin: Option<ProjectedQueryOrigin>,
        request: ValidatedMatchRequest,
        inner: ValidatedMatchResult,
        registry: Arc<DescriptorRegistry>,
    ) -> Result<NodeValidatedMatchResultHandle> {
        validated_result_handle(
            self.installed.as_ref(),
            origin,
            request,
            inner,
            registry,
            self.output,
            self.lineage,
            self.resources,
            self.deadline,
            self.cancellation,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn validated_result_handle(
    installed: Option<&Arc<InstalledRuntimeProjection>>,
    origin: Option<ProjectedQueryOrigin>,
    request: ValidatedMatchRequest,
    result: ValidatedMatchResult,
    registry: Arc<DescriptorRegistry>,
    output: Arc<NodeOutputShape>,
    lineage: Arc<()>,
    resources: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
) -> Result<NodeValidatedMatchResultHandle> {
    let projected = installed
        .zip(origin)
        .map(|(installed, origin)| {
            origin
                .materialize_borrowed_with_budget(
                    installed,
                    &registry,
                    &request,
                    &result,
                    resources.projected(),
                    &cancellation,
                    Some(deadline),
                )
                .map(|(value, _measure)| Arc::new(value))
                .map_err(napi_sdk_diagnostic)
        })
        .transpose()?;
    Ok(NodeValidatedMatchResultHandle {
        inner: result,
        request,
        registry,
        projected,
        output,
        lineage,
        deadline,
        cancellation,
    })
}

impl NodeValidatedMatchResultHandle {
    fn result<'a>(&'a self, query: &NodeMatchQueryHandle) -> Result<&'a MatchResult> {
        self.deadline
            .check(&self.cancellation)
            .map_err(napi_sdk_diagnostic)?;
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
            MatchResult::Reduction { rows, .. }
            | MatchResult::FieldReduction { rows, .. }
            | MatchResult::FieldTupleReduction { rows, .. } => Ok(rows),
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

    fn projected_slot_thing(
        &self,
        raw: &HydratedThing,
        row_index: u32,
        slot_index: u32,
        value_index: u32,
        page: bool,
    ) -> Result<Option<Arc<ProjectedThing>>> {
        let Some(projected) = &self.projected else {
            return Ok(None);
        };
        let rows = match (page, projected.as_ref()) {
            (false, ProjectedQueryValue::Rows { rows }) => rows,
            (true, ProjectedQueryValue::Page { entries, .. }) => entries,
            _ => return Err(projected_companion_mismatch()),
        };
        let value = rows
            .get(row_index as usize)
            .and_then(|row| row.slots().get(slot_index as usize))
            .map(|slot| slot.value())
            .ok_or_else(projected_companion_mismatch)?;
        let thing = match value {
            ProjectedQuerySlotValue::One(thing) if value_index == 0 => Arc::clone(thing),
            ProjectedQuerySlotValue::Many(things) => things
                .get(value_index as usize)
                .cloned()
                .ok_or_else(projected_companion_mismatch)?,
            ProjectedQuerySlotValue::One(_) => return Err(projected_companion_mismatch()),
        };
        self.ensure_projected_thing_matches(raw, &thing)?;
        Ok(Some(thing))
    }

    fn projected_reduction_group(
        &self,
        raw: &HydratedThing,
        row_index: u32,
    ) -> Result<Option<Arc<ProjectedThing>>> {
        let Some(projected) = &self.projected else {
            return Ok(None);
        };
        let rows = match projected.as_ref() {
            ProjectedQueryValue::Reduction { rows, .. } => rows,
            _ => return Err(projected_companion_mismatch()),
        };
        let thing = rows
            .get(row_index as usize)
            .and_then(|row| row.group())
            .and_then(|group| match group {
                ProjectedReductionGroup::Thing(thing) => Some(Arc::clone(thing)),
                ProjectedReductionGroup::Field(_) | ProjectedReductionGroup::Fields(_) => None,
            })
            .ok_or_else(projected_companion_mismatch)?;
        self.ensure_projected_thing_matches(raw, &thing)?;
        Ok(Some(thing))
    }

    fn ensure_projected_thing_matches(
        &self,
        raw: &HydratedThing,
        projected: &ProjectedThing,
    ) -> Result<()> {
        let label = self
            .registry
            .descriptor_type_name(raw.concrete_descriptor())
            .ok_or_else(projected_companion_mismatch)?;
        let kind_matches = matches!(
            (raw.kind(), projected.type_id().kind()),
            (
                ThingKind::Entity,
                type_bridge_contract::id::TypeKind::Entity
            ) | (
                ThingKind::Relation,
                type_bridge_contract::id::TypeKind::Relation
            )
        );
        if !kind_matches
            || label != projected.type_id().label().as_str()
            || raw.concept_id().as_str() != projected.iid()
        {
            return Err(projected_companion_mismatch());
        }
        Ok(())
    }
}

fn projected_companion_mismatch() -> napi::Error {
    result_decode_error(
        "result_projected_companion_mismatch",
        "projected companion differs from its validated result path",
    )
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
                projected: self.projected_slot_thing(thing, row_index, slot_index, 0, false)?,
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
            projected: self.projected_slot_thing(
                thing,
                entry_index,
                slot_index,
                value_index,
                true,
            )?,
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
            projected: self.projected_reduction_group(group, row_index)?,
            inner: NodeValidatedThing::Selected(group.clone()),
        })
    }

    #[napi(js_name = "reductionGroupValueJson")]
    pub fn reduction_group_value_json(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<String> {
        let value = self
            .reduction_row(query, row_index)?
            .field_group()
            .ok_or_else(|| {
                result_decode_error(
                    "result_reduction_not_field_grouped",
                    "reduction row carries no field group evidence",
                )
            })?;
        let (value_type, value) = match value {
            type_bridge_orm::AttributeValue::String(value) => ("string", json!(value)),
            type_bridge_orm::AttributeValue::Long(value) => ("long", json!(value.to_string())),
            type_bridge_orm::AttributeValue::Double(value) => ("double", json!(value)),
            type_bridge_orm::AttributeValue::Boolean(value) => ("boolean", json!(value)),
            type_bridge_orm::AttributeValue::Date(value) => ("date", json!(value)),
            type_bridge_orm::AttributeValue::DateTime(value) => ("datetime", json!(value)),
            type_bridge_orm::AttributeValue::DateTimeTZ(value) => ("datetime_tz", json!(value)),
            type_bridge_orm::AttributeValue::Decimal(value) => ("decimal", json!(value)),
            type_bridge_orm::AttributeValue::Duration(value) => ("duration", json!(value)),
        };
        serde_json::to_string(&json!({ "valueType": value_type, "value": value })).map_err(
            |error| {
                result_decode_error(
                    "result_reduction_group_encoding_failed",
                    format!("field group value could not be encoded: {error}"),
                )
            },
        )
    }

    #[napi(js_name = "reductionGroupValuesJson")]
    pub fn reduction_group_values_json(
        &self,
        query: &NodeMatchQueryHandle,
        row_index: u32,
    ) -> Result<String> {
        let values = self
            .reduction_row(query, row_index)?
            .field_groups()
            .ok_or_else(|| {
                result_decode_error(
                    "result_reduction_not_tuple_field_grouped",
                    "reduction row carries no tuple field group evidence",
                )
            })?;
        let encoded = values
            .iter()
            .map(|value| {
                let (value_type, value) = match value {
                    type_bridge_orm::AttributeValue::String(value) => ("string", json!(value)),
                    type_bridge_orm::AttributeValue::Long(value) => {
                        ("long", json!(value.to_string()))
                    }
                    type_bridge_orm::AttributeValue::Double(value) => ("double", json!(value)),
                    type_bridge_orm::AttributeValue::Boolean(value) => ("boolean", json!(value)),
                    type_bridge_orm::AttributeValue::Date(value) => ("date", json!(value)),
                    type_bridge_orm::AttributeValue::DateTime(value) => ("datetime", json!(value)),
                    type_bridge_orm::AttributeValue::DateTimeTZ(value) => {
                        ("datetime_tz", json!(value))
                    }
                    type_bridge_orm::AttributeValue::Decimal(value) => ("decimal", json!(value)),
                    type_bridge_orm::AttributeValue::Duration(value) => ("duration", json!(value)),
                };
                json!({ "valueType": value_type, "value": value })
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&encoded).map_err(|error| {
            result_decode_error(
                "result_reduction_group_encoding_failed",
                format!("tuple field group values could not be encoded: {error}"),
            )
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

    fn descriptor_id(&self) -> &DescriptorId {
        match self {
            Self::Selected(thing) => thing.concrete_descriptor(),
            Self::RolePlayer(player) => player.concrete_descriptor(),
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
    projected: Option<Arc<ProjectedThing>>,
}

#[napi]
impl NodeValidatedThingHandle {
    pub(crate) fn projected_thing(&self) -> Option<Arc<ProjectedThing>> {
        self.projected.as_ref().map(Arc::clone)
    }

    pub(crate) fn hydrated_kind(&self) -> ThingKind {
        self.inner.kind()
    }

    pub(crate) fn hydrated_attributes(&self) -> &[HydratedAttribute] {
        self.inner.attributes()
    }

    pub(crate) fn hydrated_roles(&self) -> &[HydratedRole] {
        self.inner.roles()
    }

    pub(crate) fn hydrated_concept_id(&self) -> &str {
        self.inner.concept_id()
    }

    pub(crate) fn hydrated_descriptor(&self) -> &DescriptorId {
        self.inner.descriptor_id()
    }

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
            projected: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;
    use sha2::{Digest, Sha256};
    use tokio::sync::Notify;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
    use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
    use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
    use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
    use type_bridge_orm::ProxyQueryConnection;
    use type_bridge_orm::query_v2_prepared::QueryAuthority;
    use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
    use type_bridge_orm::session::backend::{
        AnswerCancellation, BoundedAnswerLimits, BoundedAnswerReader, BoxFuture, DriverBackend,
        QueryResult, TransactionOps, TxType,
    };
    use type_bridge_orm::{
        CapabilitySet, Database, MatchExecutionLimits, OrmError, ProjectedCreate,
        ProjectedCrudExecutor,
    };
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_codegen::TypeScriptEmitter;

    const ORDERED_QUERY_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      tag:
        card: { min: 0 }
        ordered: true
relations:
  membership:
    relates:
      member: { card: { min: 0 } }
plays:
  person:
    membership: [member]
"#;

    #[derive(Default)]
    struct ProjectedQueryEvents {
        opens: AtomicUsize,
        queries: AtomicUsize,
        commits: AtomicUsize,
        rollbacks: AtomicUsize,
        closes: AtomicUsize,
    }

    struct ProjectedQueryBackend {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        events: Arc<ProjectedQueryEvents>,
    }

    impl DriverBackend for ProjectedQueryBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            self.events.opens.fetch_add(1, Ordering::SeqCst);
            let transaction = ProjectedQueryTransaction {
                responses: Arc::clone(&self.responses),
                events: Arc::clone(&self.events),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct ProjectedQueryTransaction {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        events: Arc<ProjectedQueryEvents>,
    }

    impl TransactionOps for ProjectedQueryTransaction {
        fn query(
            &mut self,
            _typeql: &str,
        ) -> BoxFuture<'_, std::result::Result<QueryResult, OrmError>> {
            self.events.queries.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("projected query bridge issued an unexpected provider query");
            Box::pin(async move { Ok(response) })
        }

        fn commit(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.events.commits.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.events.rollbacks.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.events.closes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

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

    struct BlockingOpenBackend {
        opens: Arc<AtomicUsize>,
        opened: Arc<Notify>,
    }

    impl DriverBackend for BlockingOpenBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened.notify_one();
            Box::pin(std::future::pending())
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    fn source_identity(root: &Path, relative: &str) -> Value {
        let path = root.join(relative);
        let metadata = path
            .symlink_metadata()
            .unwrap_or_else(|error| panic!("proof source {relative} is not inspectable: {error}"));
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        let mut source = File::open(&path)
            .unwrap_or_else(|error| panic!("proof source {relative} is not readable: {error}"));
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).unwrap();
        json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
    }

    fn emit_direct_cancellation_proof(observation: Value) {
        let Ok(destination) = std::env::var("TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT") else {
            return;
        };
        let nonce = std::env::var("TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE")
            .expect("queries proof fragment requires the same-run nonce");
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value)),
            "queries proof run nonce must be 64 lowercase hex characters"
        );
        let destination = PathBuf::from(destination);
        assert!(destination.is_absolute());
        let parent = destination
            .parent()
            .expect("proof fragment path has no parent");
        let parent_metadata = parent
            .symlink_metadata()
            .expect("queries proof fragment parent is not inspectable");
        assert!(parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink());
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .ancestors()
            .nth(3)
            .expect("Node crate is not nested under the repository root");
        let fragment = json!({
            "binding": "node",
            "contract": {
                "allowlist": source_identity(
                    root,
                    "tests/contracts/sdk_conformance/queries/proof-fragment-allowlist.json",
                ),
                "journey": source_identity(
                    root,
                    "tests/contracts/sdk_conformance/queries/journey.json",
                ),
                "proof_schema": source_identity(
                    root,
                    "tests/contracts/sdk_conformance/queries/proof-fragment-schema.json",
                ),
            },
            "format": "typebridge.sdk-queries-proof-fragment/v1",
            "producer": {
                "id": "node.native_direct_cancellation",
                "sources": [
                    source_identity(
                        root,
                        "type-bridge-core/crates/node/src/match_runtime.rs",
                    ),
                    source_identity(
                        root,
                        "type-bridge-core/crates/orm/src/match_request/selected_result_executor.rs",
                    ),
                ],
            },
            "results": [{
                "observation": observation,
                "observation_ref": "cancellation_direct",
                "outcome": "passed",
                "proof_kind": "direct_runtime",
                "test_id": "node.native_direct_cancellation",
            }],
            "run_nonce": nonce,
            "semantic_profile": crate::sdk_profile_matrix::requested(2).semantic_profile,
        });
        let mut payload = serde_json::to_vec(&fragment).unwrap();
        payload.push(b'\n');
        assert!(payload.len() <= 64 * 1024);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .expect("queries proof fragment destination must not exist");
        output.write_all(&payload).unwrap();
        output.sync_all().unwrap();
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

    fn ordered_query_projection() -> Arc<InstalledRuntimeProjection> {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("node-ordered-query.yaml").unwrap(),
            ORDERED_QUERY_SCHEMA,
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let emitter = TypeScriptEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(&resolved),
            &emitter.code_resources_for(&resolved).unwrap(),
        )
        .unwrap();
        // Installed the way a generated package is: with the schema
        // authority the package embeds, which direct execution adapts under.
        let authority = type_bridge_schema::build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &type_bridge_schema::ManagedDeltaContext::new(
                type_bridge_contract::managed_scope::ManagedScopeId::new("node-ordered-query")
                    .unwrap(),
                profile,
                type_bridge_schema::schema_authority_capability_vocabulary(),
            ),
        )
        .unwrap();
        Arc::new(
            InstalledRuntimeProjection::try_new(projection)
                .unwrap()
                .with_schema_authority(&authority),
        )
    }

    fn projected_query_database(
        responses: Vec<QueryResult>,
    ) -> (Database, Arc<ProjectedQueryEvents>) {
        let events = Arc::new(ProjectedQueryEvents::default());
        let backend = ProjectedQueryBackend {
            responses: Arc::new(Mutex::new(responses.into())),
            events: Arc::clone(&events),
        };
        (
            Database::with_backend(Box::new(backend), "ordered-query"),
            events,
        )
    }

    fn projected_query() -> (NodeMatchQueryHandle, Arc<DescriptorRegistry>) {
        let installed = ordered_query_projection();
        let registry = Arc::new(installed.match_registry().unwrap());
        let session = NodeMatchSessionHandle::from_installed(
            Arc::clone(&installed),
            Arc::clone(&registry),
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
            QueryLimits::CANONICAL,
        );
        let person = session.inner.exact("person").unwrap();
        let shape = session.inner.positional([person.one()]).unwrap();
        let inner = session.inner.query(shape).unwrap();
        (
            NodeMatchQueryHandle {
                inner,
                installed: Some(installed),
                output: Arc::new(NodeOutputShape::Positional {
                    slots: vec![NodeOutputSlotKind::One],
                }),
                lineage: Arc::new(()),
                resources: session.resources,
                cancellation: session.cancellation.clone(),
                session_closed: Arc::clone(&session.closed),
                closed: AtomicBool::new(false),
            },
            registry,
        )
    }

    fn projected_rows_request(query: &NodeMatchQueryHandle) -> ValidatedMatchRequest {
        query
            .inner
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::BoundedMany,
            )
            .unwrap()
    }

    fn projected_query_responses(iid: &str) -> Vec<QueryResult> {
        vec![
            QueryResult::Rows(vec![serde_json::json!({
                "bindings": [{"binding": 0, "concept_id": iid}],
                "satisfied_role_edges": [],
            })]),
            QueryResult::Documents(vec![serde_json::json!({
                "binding": 0,
                "concept_id": iid,
                "concrete_type": "person",
                "kind": "entity",
                "attributes": [
                    {"field": "identifier", "value_type": "string", "values": ["ada"]},
                    {"field": "tag", "value_type": "string", "values": ["engineer"]},
                ],
                "roles": [],
            })]),
        ]
    }

    fn projected_membership_create(
        installed: &InstalledRuntimeProjection,
        thing: &ProjectedThing,
    ) -> ProjectedCreate {
        ProjectedCreate::try_new(
            installed,
            type_bridge_contract::id::TypeId::new(
                type_bridge_contract::id::TypeKind::Relation,
                "membership",
            )
            .unwrap(),
            Vec::new(),
            vec![(
                type_bridge_contract::id::RoleId::new("membership", "member").unwrap(),
                vec![thing.try_to_reference(installed).unwrap()],
            )],
        )
        .unwrap()
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

    #[tokio::test]
    async fn ordered_query_companion_is_cached_aligned_and_origin_scoped() {
        let (query, registry) = projected_query();
        let installed = query.installed.as_ref().unwrap();
        let responses = ["0xa1", "0xa2", "0xa3"]
            .into_iter()
            .flat_map(projected_query_responses)
            .collect();
        let (database, events) = projected_query_database(responses);
        let deadline = || QueryExecutionDeadline::from_timeout_milliseconds(30_000);

        let owned_request = projected_rows_request(&query);
        let owned_inner = database
            .execute_match(registry.as_ref(), &owned_request)
            .await
            .unwrap();
        let owned = query
            .result_context(
                QueryExecutionResourceLimits::default(),
                deadline(),
                AnswerCancellation::default(),
            )
            .attach_with_origin(
                Some(ProjectedQueryOrigin::for_database(&database)),
                owned_request,
                owned_inner,
                Arc::clone(&registry),
            )
            .unwrap();
        let owned_first = owned.slot_thing(&query, 0, 0).unwrap().projected.unwrap();
        let owned_second = owned.slot_thing(&query, 0, 0).unwrap().projected.unwrap();
        assert!(Arc::ptr_eq(&owned_first, &owned_second));
        assert!(owned_first.origin_carrier().is_some());
        assert_eq!(owned_first.iid(), "0xa1");
        ProjectedCrudExecutor::new(installed)
            .preflight_relation_create_for_database_with_compatibility(
                &database,
                &projected_membership_create(installed, &owned_first),
            )
            .unwrap();

        let raw = match owned.result(&query).unwrap() {
            MatchResult::Rows { rows } => match &rows[0].slots()[0] {
                SlotValue::One(thing) => thing,
                SlotValue::Many(_) => panic!("projected query fixture must be singular"),
            },
            _ => panic!("projected query fixture must return rows"),
        };
        let mut mismatched_json = serde_json::to_value(raw).unwrap();
        mismatched_json["concept_id"] = json!("0xaf");
        let mismatched: HydratedThing = serde_json::from_value(mismatched_json).unwrap();
        let error = owned
            .projected_slot_thing(&mismatched, 0, 0, 0, false)
            .unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_projected_companion_mismatch");

        let transaction = database.transaction_context(TxType::Read).await.unwrap();
        let borrowed_request = projected_rows_request(&query);
        let borrowed_inner = transaction
            .execute_match(registry.as_ref(), &borrowed_request)
            .await
            .unwrap();
        let borrowed = query
            .result_context(
                QueryExecutionResourceLimits::default(),
                deadline(),
                AnswerCancellation::default(),
            )
            .attach_with_origin(
                Some(ProjectedQueryOrigin::for_transaction(&transaction).unwrap()),
                borrowed_request,
                borrowed_inner,
                Arc::clone(&registry),
            )
            .unwrap();
        let borrowed_thing = borrowed
            .slot_thing(&query, 0, 0)
            .unwrap()
            .projected
            .unwrap();
        assert!(borrowed_thing.origin_carrier().is_some());
        assert_eq!(borrowed_thing.iid(), "0xa2");
        ProjectedCrudExecutor::new(installed)
            .preflight_relation_create_for_database_with_compatibility(
                &database,
                &projected_membership_create(installed, &borrowed_thing),
            )
            .unwrap();

        let remote_request = projected_rows_request(&query);
        let remote_inner = database
            .execute_match(registry.as_ref(), &remote_request)
            .await
            .unwrap();
        let remote = query
            .result_context(
                QueryExecutionResourceLimits::default(),
                deadline(),
                AnswerCancellation::default(),
            )
            .attach(remote_request, remote_inner, Arc::clone(&registry))
            .unwrap();
        let remote_thing = remote.slot_thing(&query, 0, 0).unwrap().projected.unwrap();
        assert!(remote_thing.origin_carrier().is_none());
        assert!(
            remote_thing
                .try_to_reference(installed)
                .unwrap()
                .origin_carrier()
                .is_none()
        );
        ProjectedCrudExecutor::new(installed)
            .preflight_relation_create_for_database_with_compatibility(
                &database,
                &projected_membership_create(installed, &remote_thing),
            )
            .unwrap();

        let foreign_create = projected_membership_create(installed, &owned_first);
        let (foreign, foreign_events) = projected_query_database(Vec::new());
        let diagnostic = ProjectedCrudExecutor::new(installed)
            .insert_relation(&foreign, &foreign_create)
            .await
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "reference_database_mismatch");
        assert_eq!(foreign_events.opens.load(Ordering::SeqCst), 0);
        assert_eq!(foreign_events.queries.load(Ordering::SeqCst), 0);
        assert_eq!(foreign_events.commits.load(Ordering::SeqCst), 0);
        assert_eq!(foreign_events.rollbacks.load(Ordering::SeqCst), 0);

        assert_eq!(events.opens.load(Ordering::SeqCst), 3);
        assert_eq!(events.queries.load(Ordering::SeqCst), 6);
        assert_eq!(events.commits.load(Ordering::SeqCst), 0);
        assert_eq!(events.rollbacks.load(Ordering::SeqCst), 0);
        transaction.close().await.unwrap();
    }

    #[tokio::test]
    async fn remote_projected_companion_retains_effective_resources_cancellation_and_deadline() {
        let (query, registry) = projected_query();
        let responses = ["0xa1", "0xa2", "0xa3"]
            .into_iter()
            .flat_map(projected_query_responses)
            .collect();
        let (database, events) = projected_query_database(responses);

        let tight = QueryExecutionResourceLimits::tightened(
            30_000,
            0,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u32::MAX,
        );
        let request = projected_rows_request(&query);
        let inner = database
            .execute_match(registry.as_ref(), &request)
            .await
            .unwrap();
        let error = match query
            .result_context(
                tight,
                QueryExecutionDeadline::for_limits(tight),
                AnswerCancellation::default(),
            )
            .attach(request, inner, Arc::clone(&registry))
        {
            Err(error) => error,
            Ok(_) => panic!("tight remote context must reject projected materialization"),
        };
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "projected_query_row_limit");

        let request = projected_rows_request(&query);
        let inner = database
            .execute_match(registry.as_ref(), &request)
            .await
            .unwrap();
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let error = match query
            .result_context(
                QueryExecutionResourceLimits::default(),
                QueryExecutionDeadline::from_timeout_milliseconds(30_000),
                cancellation,
            )
            .attach(request, inner, Arc::clone(&registry))
        {
            Err(error) => error,
            Ok(_) => panic!("cancelled remote context must reject projected materialization"),
        };
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "provider_cancelled");

        let request = projected_rows_request(&query);
        let inner = database
            .execute_match(registry.as_ref(), &request)
            .await
            .unwrap();
        let error = match query
            .result_context(
                QueryExecutionResourceLimits::default(),
                QueryExecutionDeadline::from_timeout_milliseconds(0),
                AnswerCancellation::default(),
            )
            .attach(request, inner, registry)
        {
            Err(error) => error,
            Ok(_) => panic!("expired remote context must reject projected materialization"),
        };
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "transaction_deadline_exceeded");

        assert_eq!(events.opens.load(Ordering::SeqCst), 3);
        assert_eq!(events.queries.load(Ordering::SeqCst), 6);
    }

    /// A proxy connection to the ordered-query schema whose ceiling is
    /// `answer_limits`; its exchange is never performed.
    fn ordered_query_proxy(answer_limits: QueryExecutionResourceLimits) -> ProxyQueryConnection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("node-ordered-query.yaml").unwrap(),
            ORDERED_QUERY_SCHEMA,
        )])
        .unwrap();
        let declared = encode_declared_schema(&normalize_documents(&documents).unwrap()).unwrap();
        let authority = QueryAuthority::from_declared_bytes(
            &declared,
            "node-ordered-query",
            "typedb-3.12.1/v1",
        )
        .unwrap();
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("node-ceiling-test", "epoch-00000000001").unwrap(),
            RemoteReplySigningKey::from_secret_bytes([0x5c; 32]).public_key(),
        )
        .encode()
        .unwrap();
        ProxyQueryConnection::with_exchange(advertisement, Arc::new(authority), answer_limits)
            .unwrap()
    }

    #[tokio::test]
    async fn terminals_run_and_hydrate_under_the_connection_ceiling() {
        let (mut query, registry) = projected_query();
        // The session asks for more than its connection allows.
        query.resources = QueryExecutionResourceLimits {
            items: QueryExecutionResourceLimits::default().items * 4,
            ..QueryExecutionResourceLimits::default()
        };
        let ceiling = QueryExecutionResourceLimits {
            timeout_milliseconds: 5_000,
            items: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let connection = ordered_query_proxy(ceiling);
        let session_deadline = QueryExecutionDeadline::for_limits(query.resources);

        let (resources, deadline) =
            query.terminal_budget(&QueryTarget::Proxy(&connection), session_deadline);
        assert_eq!(resources, query.resources.constrained_by(ceiling));
        assert_eq!(resources.items, 0);
        assert_eq!(resources.timeout_milliseconds, 5_000);
        assert!(deadline.instant() < session_deadline.instant());

        // Hydration shares the constrained policy: the session's raised item
        // budget would admit this row, the ceiling does not.
        let (database, _events) = projected_query_database(projected_query_responses("0xa1"));
        let request = projected_rows_request(&query);
        let inner = database
            .execute_match(registry.as_ref(), &request)
            .await
            .unwrap();
        let error = match query
            .result_context(resources, deadline, AnswerCancellation::default())
            .attach(request, inner, Arc::clone(&registry))
        {
            Err(error) => error,
            Ok(_) => panic!("the ceiling must bound projected materialization"),
        };
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "projected_query_row_limit");

        // A target without a ceiling keeps the session's policy and deadline.
        let (resources, deadline) =
            query.terminal_budget(&QueryTarget::Database(&database), session_deadline);
        assert_eq!(resources, query.resources);
        assert_eq!(deadline.instant(), session_deadline.instant());
    }

    #[test]
    fn every_terminal_runs_through_the_shared_query_target() {
        let production = |source: &'static str| {
            source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .expect("source has a production half")
        };
        let direct = production(include_str!("match_runtime.rs"));
        // Direct terminals settle the shared target on the provider runtime
        // as a Promise rather than blocking the event loop.
        assert!(direct.contains(".query_target()\n                .execute("));
        assert!(direct.contains("proxy_connection_runtime::settle("));
        assert!(!direct.contains("execute_match_with_limits"));
        assert!(!direct.contains("direct_with_deadline"));
        let remote = production(include_str!("query_v2_model_remote_runtime.rs"));
        assert!(remote.contains("ProxyQueryConnection::finish("));
        assert!(!remote.contains("prepare_remote_model_query_v2_with_budget"));
        assert!(!remote.contains("claim_reply_with_cancellation"));
        let proxy = production(include_str!("proxy_connection_runtime.rs"));
        assert!(proxy.contains("QueryTarget::Proxy(&connection)"));
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
        assert_eq!(
            payload["message"],
            "The typed query plan does not satisfy the generated query contract"
        );
        assert!(!error.reason.contains("missing"));
        assert!(payload["path"].is_array());
        assert!(payload["details"].is_object());
    }

    #[test]
    fn common_resources_keep_a_raise_refuse_past_the_maximum_and_preserve_zero() {
        let wide = |value| {
            Some(BigInt {
                sign_bit: false,
                words: vec![value],
            })
        };
        let resources = NodeQueryExecutionResources::new(
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_ITEMS + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_BYTES + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_GRAPH_NODES + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS + 1),
            wide(type_bridge_orm::DEFAULT_DIRECT_QUERY_ROLE_PLAYERS + 1),
            wide(u64::from(type_bridge_orm::DEFAULT_DIRECT_QUERY_STATEMENTS)),
        )
        .unwrap();
        let default = QueryExecutionResourceLimits::default();
        assert_eq!(
            resources.inner(),
            QueryExecutionResourceLimits {
                timeout_milliseconds: default.timeout_milliseconds + 1,
                items: default.items + 1,
                bytes: default.bytes + 1,
                graph_nodes: default.graph_nodes + 1,
                attribute_values: default.attribute_values + 1,
                collection_members: default.collection_members + 1,
                role_players: default.role_players + 1,
                statements: default.statements,
            },
            "a raised dimension keeps what the caller asked for"
        );
        let with = |timeout: u64, statements: u64| {
            NodeQueryExecutionResources::new(
                wide(timeout),
                None,
                None,
                None,
                None,
                None,
                None,
                wide(statements),
            )
            .map(|_| ())
            .map_err(|error| error.reason.clone())
        };
        let refused = with(type_bridge_orm::MAX_QUERY_TIMEOUT_MILLISECONDS + 1, 3).unwrap_err();
        assert!(refused.contains("timeout_milliseconds"), "{refused}");
        let refused = with(1, u64::from(type_bridge_orm::MAX_QUERY_STATEMENTS) + 1).unwrap_err();
        assert!(refused.contains("statements"), "{refused}");

        let zero = || wide(0);
        let resources = NodeQueryExecutionResources::new(
            zero(),
            zero(),
            zero(),
            zero(),
            zero(),
            zero(),
            zero(),
            zero(),
        )
        .unwrap();
        assert_eq!(
            resources.inner(),
            QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn query_and_session_close_preserve_persistent_handle_independence() {
        let registry = registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.inner.exact("person").unwrap();
        let shape = session.inner.positional([person.one()]).unwrap();
        let inner = session.inner.query(shape).unwrap();
        let query = NodeMatchQueryHandle {
            inner,
            installed: None,
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
            lineage: Arc::new(()),
            resources: session.resources,
            cancellation: session.cancellation.clone(),
            session_closed: Arc::clone(&session.closed),
            closed: AtomicBool::new(false),
        };
        let clone = query.fork().unwrap();
        let derived = query
            .where_predicate(&NodeMatchPredicateHandle {
                inner: person.iid("0x1").unwrap(),
            })
            .unwrap();

        query.close();
        query.close();
        assert!(query.is_closed());
        assert!(!clone.is_closed());
        assert!(!derived.is_closed());
        assert!(
            query
                .fork()
                .err()
                .expect("closed query rejects fork")
                .reason
                .contains("query_resource_closed")
        );
        clone.fork().expect("independent clone remains composable");
        derived.close();
        assert!(!clone.is_closed());

        session.close();
        session.close();
        assert!(session.is_closed());
        assert!(clone.is_closed());
        assert!(
            clone
                .begin_invocation()
                .unwrap_err()
                .reason
                .contains("query_resource_closed")
        );
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
                Status::Cancelled,
                "cancelled",
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
    fn unknown_enum_spelling_is_the_structured_core_diagnostic() {
        let error = parse_enum::<SortDirection>("desc").unwrap_err();
        assert_eq!(error.status, Status::InvalidArg);
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["category"], "invalid_input");
        assert_eq!(payload["code"], "unknown_enum_value");
        assert_eq!(
            payload["details"],
            json!({
                "kind": {"kind": "query_identity", "value": "sort_direction"},
                "value": {"kind": "query_identity", "value": "desc"},
                "accepted": {
                    "kind": "query_identity_list",
                    "value": ["ascending", "descending"],
                },
            })
        );
    }

    #[test]
    fn binding_calls_raise_the_enum_spelling_corpus_diagnostic() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../tests/contracts/enum-spellings-v1.json"
        ))
        .unwrap();
        let expected = |id: &str| {
            corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap()["expected"]
                .clone()
        };
        let registry = registry();
        let session = NodeMatchSessionHandle::new(&registry);
        let person = session.exact("person".into()).unwrap();
        let name = person.field("name".into()).unwrap();
        let shape = NodeMatchShapeHandle {
            inner: session.inner.positional([person.inner.one()]).unwrap(),
            output: Arc::new(NodeOutputShape::Positional {
                slots: vec![NodeOutputSlotKind::One],
            }),
        };
        let query = session.query(&shape).unwrap();
        let errors = [
            (
                "comparison_operator.unknown",
                name.compare_field("equals".into(), &name).err(),
            ),
            (
                "sort_direction.unknown",
                name.order("desc".into(), "reject".into()).err(),
            ),
            (
                "missing_order.unknown",
                name.order("ascending".into(), "none".into()).err(),
            ),
            (
                "row_cardinality.unknown",
                query
                    .fetch_rows_diagnostic(Vec::new(), 0u64.into(), 1u64.into(), "many".into())
                    .err(),
            ),
            (
                "reducer.unknown",
                query
                    .reduce_by_diagnostic(&person, None, vec!["variance".into()], vec![None])
                    .err(),
            ),
        ];
        for (id, error) in errors {
            let error = error.unwrap_or_else(|| panic!("{id} was accepted"));
            assert_eq!(error.status, Status::InvalidArg, "{id}");
            let payload: Value = serde_json::from_str(&error.reason).unwrap();
            let actual = json!({
                "category": payload["category"],
                "code": payload["code"],
                "details": payload["details"],
            });
            assert_eq!(actual, expected(id), "{id}");
        }
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
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
            registry: query.inner.registry_arc(),
            projected: None,
            output: Arc::clone(&query.output),
            lineage: Arc::clone(&query.lineage),
            deadline: QueryExecutionDeadline::from_timeout_milliseconds(30_000),
            cancellation: AnswerCancellation::default(),
        };

        assert_eq!(result.reduction_row_count(&query).unwrap(), 0);
        let error = result.reduction_value_count(&query, 0).unwrap_err();
        let payload: Value = serde_json::from_str(&error.reason).unwrap();
        assert_eq!(payload["code"], "result_reduction_row_out_of_bounds");
    }

    #[test]
    fn node_direct_cancellation_fragment_is_measured_from_owned_execution() {
        let registry = registry().shared_registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let validated = session
            .query(shape)
            .unwrap()
            .validate_count_by(&person)
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let pre_dispatch_opens = Arc::new(AtomicUsize::new(0));
        let pre_dispatch_database = Database::with_backend(
            Box::new(BlockingOpenBackend {
                opens: Arc::clone(&pre_dispatch_opens),
                opened: Arc::new(Notify::new()),
            }),
            "test",
        );
        let pre_dispatch_cancellation = AnswerCancellation::default();
        pre_dispatch_cancellation.cancel();
        let pre_dispatch_error = runtime
            .block_on(pre_dispatch_database.execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    u64::MAX,
                    u64::MAX,
                    Duration::from_secs(30),
                    pre_dispatch_cancellation,
                ),
            ))
            .unwrap_err();
        assert_eq!(pre_dispatch_opens.load(Ordering::SeqCst), 0);
        let pre_dispatch_payload: Value =
            serde_json::from_str(&crate::napi_orm_error(pre_dispatch_error).reason).unwrap();

        let in_flight_opens = Arc::new(AtomicUsize::new(0));
        let in_flight_opened = Arc::new(Notify::new());
        let in_flight_database = Database::with_backend(
            Box::new(BlockingOpenBackend {
                opens: Arc::clone(&in_flight_opens),
                opened: Arc::clone(&in_flight_opened),
            }),
            "test",
        );
        let in_flight_cancellation = AnswerCancellation::default();
        let in_flight_limits = MatchExecutionLimits::tightened(
            u64::MAX,
            u64::MAX,
            Duration::from_secs(30),
            in_flight_cancellation.clone(),
        );
        let in_flight_error = runtime.block_on(async {
            let opened = in_flight_opened.notified();
            tokio::pin!(opened);
            let execution = in_flight_database.execute_match_with_limits(
                &registry,
                &validated,
                in_flight_limits,
            );
            tokio::pin!(execution);
            tokio::select! {
                () = &mut opened => {}
                result = &mut execution => panic!("blocked provider returned before cancellation: {result:?}"),
            }
            assert_eq!(in_flight_opens.load(Ordering::SeqCst), 1);
            in_flight_cancellation.cancel();
            tokio::time::timeout(Duration::from_secs(1), execution)
                .await
                .expect("cancellation did not wake the in-flight provider await")
                .unwrap_err()
        });
        let in_flight_payload: Value =
            serde_json::from_str(&crate::napi_orm_error(in_flight_error).reason).unwrap();
        let observation = json!({
            "in_flight": {
                "category": in_flight_payload["sdk_category"],
                "code": in_flight_payload["code"],
                "partial_result": false,
                "provider_await_woken": true,
            },
            "pre_dispatch": {
                "category": pre_dispatch_payload["sdk_category"],
                "code": pre_dispatch_payload["code"],
                "partial_result": false,
                "provider_calls": pre_dispatch_opens.load(Ordering::SeqCst),
            },
        });
        assert_eq!(
            observation,
            json!({
                "in_flight": {
                    "category": "cancelled",
                    "code": "provider_cancelled",
                    "partial_result": false,
                    "provider_await_woken": true,
                },
                "pre_dispatch": {
                    "category": "cancelled",
                    "code": "provider_cancelled",
                    "partial_result": false,
                    "provider_calls": 0,
                },
            })
        );
        emit_direct_cancellation_proof(observation);
    }
}
