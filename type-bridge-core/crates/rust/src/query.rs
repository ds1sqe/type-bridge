#![deny(missing_docs)]
//! Owner-branded query sessions, bindings, predicates, and the one query
//! facade (Flight 3: F3-01 foundation, F3-02 algebra, F3-03 singular shapes).

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::decimal::parse_decimal;
use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::query_plan::CompatibilityValueV2;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_orm::_descriptor::TypeDescriptorRef;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::match_request::handles::{
    BindingHandle as OrmBindingHandle, FieldHandle as OrmFieldHandle,
    FunctionArgumentHandle as OrmFunctionArgumentHandle,
    FunctionCallHandle as OrmFunctionCallHandle, OrderHandle as OrmOrderHandle,
    PredicateHandle as OrmPredicateHandle, QueryHandle as OrmQueryHandle,
    SelectionHandle as OrmSelectionHandle, SessionHandle as OrmSessionHandle,
    ShapeHandle as OrmShapeHandle,
};
use type_bridge_orm::match_request::model::{
    ComparisonOp, MissingOrder, RowCardinality, SortDirection, Window,
};
use type_bridge_orm::match_request::result::{
    HydratedThing, MatchResult, MatchRow, SlotValue, ValidatedMatchResult,
};
use type_bridge_orm::match_request::validation::ValidatedMatchRequest;
use type_bridge_orm::{
    AnswerCancellation, AttributeValue, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
    InstalledRuntimeProjection, ProjectedAttributeValue, QueryExecutionDeadline,
    QueryExecutionResourceLimits,
};

use crate::__codegen::{
    CompleteModel, EncodedScalar, FieldToken, FunctionToken, GroupedQueryValue, HydratedRow,
    HydrationCapability, Model, QueryValued, RelationModel, RolePlayerBinding, RoleToken,
    RoleTokenCompatible, SubtypeRootModel, ThingModel, TypeToken, ValidationError,
};
use crate::entity_codec::{hydrate_entity, map_validation_error};
use crate::error::{Error, ModelValidationPhase};
use crate::relation_codec::hydrate_relation;
use crate::schema::Schema;
use crate::{Database, Result};

#[cfg(test)]
mod tests;

static QUERY_SESSION_NONCE: AtomicU64 = AtomicU64::new(1);

fn schema_not_bound() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "schema_not_bound",
        vec![],
        "database is not schema-bound",
        None,
    )
}

fn cross_session_handle() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "cross_session_handle",
        vec![],
        "binding belongs to a different query session",
        None,
    )
}

fn model_label(type_id_json: &'static str) -> Result<String> {
    let id = from_canonical_json::<TypeId>(type_id_json.as_bytes()).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not canonical",
            Some(Box::new(source)),
        )
    })?;
    Ok(id.label().as_str().to_owned())
}

fn parse_owns_identity(owns_id_json: &'static str) -> Result<(String, String)> {
    let invalid = || {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_field_identity",
            vec!["type".into()],
            "generated field identity is not canonical",
            None,
        )
    };
    let value: serde_json::Value = serde_json::from_str(owns_id_json).map_err(|_| invalid())?;
    let attribute = value
        .get("attribute")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    let owner = value
        .get("owner")
        .and_then(|owner| owner.get("label"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    Ok((owner.to_owned(), attribute.to_owned()))
}

fn parse_role_identity(role_id_json: &'static str) -> Result<type_bridge_contract::id::RoleId> {
    from_canonical_json::<type_bridge_contract::id::RoleId>(role_id_json.as_bytes()).map_err(
        |source| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_role_identity",
                vec!["type".into()],
                "generated role identity is not canonical",
                Some(Box::new(source)),
            )
        },
    )
}

mod mode_sealed {
    pub trait Sealed {}
}

/// Sealed marker for a binding's exact-versus-subtypes selection behavior.
pub trait SelectionMode: mode_sealed::Sealed + 'static {}

/// Exact-match selection: results materialize as the bound concrete model.
#[derive(Clone, Copy, Debug)]
pub struct Exact;
impl mode_sealed::Sealed for Exact {}
impl SelectionMode for Exact {}

/// Subtype-inclusive selection: results materialize as the generated leaf or
/// closed family associated with the bound root.
#[derive(Clone, Copy, Debug)]
pub struct Subtypes;
impl mode_sealed::Sealed for Subtypes {}
impl SelectionMode for Subtypes {}

/// One isolated owner-branded query authoring session.
///
/// Every binding call allocates a fresh binding identity; bindings are
/// lightweight `Copy` values valid only for the session that created them.
/// Bindings from another session fail before I/O with
/// `cross_session_handle`.
pub struct QuerySession<'db, S: Schema> {
    cancellation: AnswerCancellation,
    installed: &'db InstalledRuntimeProjection,
    execution: QueryExecution<'db, S>,
    resources: QueryExecutionResourceLimits,
    session: OrmSessionHandle,
    registry: Arc<DescriptorRegistry>,
    nonce: u64,
    bindings: Vec<OrmBindingHandle>,
    closed: AtomicBool,
    marker: PhantomData<fn() -> S>,
}

enum QueryExecution<'db, S: Schema> {
    Local(&'db Database<S>),
    Borrowed(&'db type_bridge_orm::session::context::TransactionContext),
    Remote(&'db crate::remote::RemoteDatabase<S>),
}

impl<S: Schema> std::fmt::Debug for QuerySession<'_, S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QuerySession")
            .field("session", &self.nonce)
            .field("bindings", &self.bindings.len())
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

/// Opaque session-scoped binding identity carried by `Copy` handles.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingKey {
    pub(crate) nonce: u64,
    pub(crate) index: u32,
}

/// One schema/model-branded `Copy` binding token allocated by a
/// [`QuerySession`]. Copying preserves the binding identity; only a fresh
/// session binding call creates a new occurrence.
pub struct Binding<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode = Exact> {
    key: BindingKey,
    #[allow(clippy::type_complexity)]
    marker: PhantomData<fn() -> (S, M, Mode)>,
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Copy for Binding<S, M, Mode> {}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Clone for Binding<S, M, Mode> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> PartialEq for Binding<S, M, Mode> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Eq for Binding<S, M, Mode> {}
impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> std::fmt::Debug
    for Binding<S, M, Mode>
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Binding")
            .field("session", &self.key.nonce)
            .field("index", &self.key.index)
            .finish()
    }
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Binding<S, M, Mode> {
    pub(crate) fn key(self) -> BindingKey {
        self.key
    }

    /// Match this generated binding by one canonical TypeDB thing IID.
    #[must_use]
    pub fn iid(self, iid: impl Into<String>) -> Predicate<S> {
        Predicate::new(PredicateExpr::BindingIid {
            binding: self.key,
            iid: iid.into(),
        })
    }

    /// Match this generated binding by one of a non-empty bounded IID set.
    #[must_use]
    pub fn iid_in(self, iids: impl IntoIterator<Item = impl Into<String>>) -> Predicate<S> {
        Predicate::new(PredicateExpr::BindingIidIn {
            binding: self.key,
            iids: iids.into_iter().map(Into::into).collect(),
        })
    }

    /// Select this binding as an owned collection per distinct page root,
    /// preserving match multiplicity by default.
    #[must_use]
    pub fn collect(self) -> Collected<S, Self>
    where
        Self: Selectable<S>,
    {
        Collected {
            selection: self,
            distinct: false,
            order: Vec::new(),
        }
    }

    /// Lower this exact session binding into one generated schema-function
    /// argument. Nominal generated wrappers call this method; applications do
    /// not construct untyped function arguments directly.
    #[doc(hidden)]
    #[must_use]
    pub fn __function_argument(self) -> FunctionArgument<S> {
        FunctionArgument {
            nonce: self.key.nonce,
            expression: FunctionArgumentExpr::Binding(self.key),
            marker: PhantomData,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionInputExpr {
    attribute_type: TypeId,
    value: AttributeValue,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FunctionArgumentExpr {
    Binding(BindingKey),
    Value(FunctionInputExpr),
    Call(FunctionCallExpr),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionCallExpr {
    function_id: String,
    arguments: Vec<FunctionArgumentExpr>,
}

/// One immutable schema- and session-branded scalar function input.
///
/// Generated packages expose nominal domain aliases such as `IntegerInput`
/// and construct them only from projected attribute wrappers in that domain.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionInput<S: Schema, Domain> {
    nonce: u64,
    expression: FunctionInputExpr,
    marker: PhantomData<fn() -> (S, Domain)>,
}

impl<S: Schema, Domain> FunctionInput<S, Domain> {
    /// Use this scalar as one argument of the exact generated function call.
    #[doc(hidden)]
    #[must_use]
    pub fn __function_argument(&self) -> FunctionArgument<S> {
        FunctionArgument {
            nonce: self.nonce,
            expression: FunctionArgumentExpr::Value(self.expression.clone()),
            marker: PhantomData,
        }
    }
}

/// One immutable schema- and session-branded scalar function call.
///
/// Generated packages expose nominal domain aliases such as `IntegerCall`.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCall<S: Schema, Domain> {
    nonce: u64,
    expression: FunctionCallExpr,
    marker: PhantomData<fn() -> (S, Domain)>,
}

impl<S: Schema, Domain> FunctionCall<S, Domain> {
    /// Use this call result as one argument to a later generated function.
    #[doc(hidden)]
    #[must_use]
    pub fn __function_argument(&self) -> FunctionArgument<S> {
        FunctionArgument {
            nonce: self.nonce,
            expression: FunctionArgumentExpr::Call(self.expression.clone()),
            marker: PhantomData,
        }
    }

    fn field_predicate<Owner, Value>(
        &self,
        operator: ComparisonOp,
        field: BoundField<S, Owner, Value>,
    ) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        Predicate::new(PredicateExpr::FunctionField {
            call: self.expression.clone(),
            operator,
            binding: field.key,
            owns_id_json: field.owns_id_json,
        })
    }

    fn value_predicate(
        &self,
        operator: ComparisonOp,
        value: &FunctionInput<S, Domain>,
    ) -> Result<Predicate<S>> {
        if self.nonce != value.nonce {
            return Err(cross_session_handle());
        }
        Ok(Predicate::new(PredicateExpr::FunctionValue {
            call: self.expression.clone(),
            operator,
            value: value.expression.clone(),
        }))
    }

    fn call_predicate(&self, operator: ComparisonOp, other: &Self) -> Result<Predicate<S>> {
        if self.nonce != other.nonce {
            return Err(cross_session_handle());
        }
        Ok(Predicate::new(PredicateExpr::FunctionCall {
            left: self.expression.clone(),
            operator,
            right: other.expression.clone(),
        }))
    }

    /// Compare this call for equality with one same-domain bound field.
    #[must_use]
    pub fn eq_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::Equal, field)
    }

    /// Compare this call for inequality with one same-domain bound field.
    #[must_use]
    pub fn ne_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::NotEqual, field)
    }

    /// Compare this call as less than one same-domain bound field.
    #[must_use]
    pub fn lt_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::LessThan, field)
    }

    /// Compare this call as less than or equal to one same-domain bound field.
    #[must_use]
    pub fn le_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::LessThanOrEqual, field)
    }

    /// Compare this call as greater than one same-domain bound field.
    #[must_use]
    pub fn gt_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::GreaterThan, field)
    }

    /// Compare this call as greater than or equal to one same-domain bound field.
    #[must_use]
    pub fn ge_field<Owner, Value>(&self, field: BoundField<S, Owner, Value>) -> Predicate<S>
    where
        Owner: Model<Schema = S>,
        Value: QueryValued<Domain = Domain>,
    {
        self.field_predicate(ComparisonOp::GreaterThanOrEqual, field)
    }

    /// Compare this call for equality with one same-domain projected input.
    pub fn eq_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::Equal, value)
    }

    /// Compare this call for inequality with one same-domain projected input.
    pub fn ne_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::NotEqual, value)
    }

    /// Compare this call as less than one same-domain projected input.
    pub fn lt_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::LessThan, value)
    }

    /// Compare this call as less than or equal to one same-domain projected input.
    pub fn le_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::LessThanOrEqual, value)
    }

    /// Compare this call as greater than one same-domain projected input.
    pub fn gt_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::GreaterThan, value)
    }

    /// Compare this call as greater than or equal to one same-domain projected input.
    pub fn ge_value(&self, value: &FunctionInput<S, Domain>) -> Result<Predicate<S>> {
        self.value_predicate(ComparisonOp::GreaterThanOrEqual, value)
    }

    /// Compare this call for equality with another same-domain call.
    pub fn eq_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::Equal, other)
    }

    /// Compare this call for inequality with another same-domain call.
    pub fn ne_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::NotEqual, other)
    }

    /// Compare this call as less than another same-domain call.
    pub fn lt_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::LessThan, other)
    }

    /// Compare this call as less than or equal to another same-domain call.
    pub fn le_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::LessThanOrEqual, other)
    }

    /// Compare this call as greater than another same-domain call.
    pub fn gt_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::GreaterThan, other)
    }

    /// Compare this call as greater than or equal to another same-domain call.
    pub fn ge_call(&self, other: &Self) -> Result<Predicate<S>> {
        self.call_predicate(ComparisonOp::GreaterThanOrEqual, other)
    }
}

mod function_scalar_argument_sealed {
    pub trait Sealed {}
}

/// A sealed, exact-domain scalar argument accepted by generated schema
/// function wrappers.
///
/// Implementations are limited to a session-branded projected scalar input
/// and a prior scalar function call in the same schema and scalar domain.
/// This permits generated wrappers to build an immutable call DAG without
/// exposing an untyped argument constructor.
#[doc(hidden)]
pub trait FunctionScalarArgument<S: Schema, Domain>:
    function_scalar_argument_sealed::Sealed
{
    /// Lower this branded scalar into one function argument.
    #[doc(hidden)]
    fn __function_argument(&self) -> FunctionArgument<S>;
}

impl<S: Schema, Domain> function_scalar_argument_sealed::Sealed for FunctionInput<S, Domain> {}

impl<S: Schema, Domain> FunctionScalarArgument<S, Domain> for FunctionInput<S, Domain> {
    fn __function_argument(&self) -> FunctionArgument<S> {
        FunctionInput::__function_argument(self)
    }
}

impl<S: Schema, Domain> function_scalar_argument_sealed::Sealed for FunctionCall<S, Domain> {}

impl<S: Schema, Domain> FunctionScalarArgument<S, Domain> for FunctionCall<S, Domain> {
    fn __function_argument(&self) -> FunctionArgument<S> {
        FunctionCall::__function_argument(self)
    }
}

/// One opaque argument admitted by a nominal generated schema-function wrapper.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionArgument<S: Schema> {
    nonce: u64,
    expression: FunctionArgumentExpr,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Database<S> {
    /// Start one owner-branded query authoring session over this
    /// schema-bound database.
    pub fn query(&self) -> Result<QuerySession<'_, S>> {
        self.query_with_resources(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    /// Start one owner-branded query authoring session with one common
    /// tighten-only resource policy and caller-owned cancellation signal.
    pub fn query_with_resources(
        &self,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<QuerySession<'_, S>> {
        let registry = self.match_registry().ok_or_else(schema_not_bound)?;
        let installed = self
            .installed_schema()
            .map(Arc::as_ref)
            .ok_or_else(schema_not_bound)?;
        Ok(QuerySession::new(
            installed,
            Arc::clone(registry),
            QueryExecution::Local(self),
            resources,
            cancellation,
        ))
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    fn new(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        execution: QueryExecution<'db, S>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            cancellation,
            installed,
            execution,
            resources: resources.effective(),
            session: OrmSessionHandle::new(Arc::clone(&registry)),
            registry,
            nonce: QUERY_SESSION_NONCE.fetch_add(1, Ordering::Relaxed),
            bindings: Vec::new(),
            closed: AtomicBool::new(false),
            marker: PhantomData,
        }
    }

    pub(crate) fn borrowed(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        transaction: &'db type_bridge_orm::session::context::TransactionContext,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self::new(
            installed,
            registry,
            QueryExecution::Borrowed(transaction),
            resources,
            cancellation,
        )
    }

    pub(crate) fn remote(
        installed: &'db InstalledRuntimeProjection,
        registry: Arc<DescriptorRegistry>,
        remote: &'db crate::remote::RemoteDatabase<S>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self::new(
            installed,
            registry,
            QueryExecution::Remote(remote),
            resources,
            cancellation,
        )
    }

    fn begin_invocation(&self) -> Result<QueryExecutionDeadline> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let deadline = QueryExecutionDeadline::for_limits(self.resources);
        self.check_invocation(deadline, ModelValidationPhase::Input)?;
        Ok(deadline)
    }

    fn check_invocation(
        &self,
        deadline: QueryExecutionDeadline,
        phase: ModelValidationPhase,
    ) -> Result<()> {
        self.ensure_open(phase)?;
        deadline
            .check(&self.cancellation)
            .map_err(|error| Error::from_sdk_execution(error, phase))
    }

    fn ensure_open(&self, phase: ModelValidationPhase) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(Error::from_sdk_execution(
                type_bridge_orm::query_resource_closed_diagnostic(),
                phase,
            ))
        } else {
            Ok(())
        }
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    /// Explicitly close this authoring session.
    ///
    /// Closing is idempotent. Existing query lineages retain their immutable
    /// values but reject later composition and execution before provider I/O.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Return whether this session was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn push_binding<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &mut self,
        handle: OrmBindingHandle,
    ) -> Result<Binding<S, M, Mode>> {
        let index = u32::try_from(self.bindings.len()).map_err(|source| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "too_many_bindings",
                vec![],
                "query session binding capacity exceeded",
                Some(Box::new(source)),
            )
        })?;
        self.bindings.push(handle);
        Ok(Binding {
            key: BindingKey {
                nonce: self.nonce,
                index,
            },
            marker: PhantomData,
        })
    }

    /// Allocate a fresh exact-match binding for one concrete complete
    /// generated model. Abstract models have no exact binding constructor.
    pub fn exact<M>(&mut self) -> Result<Binding<S, M, Exact>>
    where
        M: ThingModel<Schema = S> + CompleteModel,
    {
        self.ensure_open(ModelValidationPhase::Input)?;
        let label = model_label(M::TYPE_ID_JSON)?;
        let handle = self.session.exact(&label).map_err(Error::from_orm)?;
        self.push_binding(handle)
    }

    /// Allocate a fresh subtype-inclusive binding for one generated subtype
    /// root; results materialize as the generated leaf or closed family.
    pub fn subtypes<M>(&mut self) -> Result<Binding<S, M, Subtypes>>
    where
        M: ThingModel<Schema = S> + SubtypeRootModel,
    {
        self.ensure_open(ModelValidationPhase::Input)?;
        let label = model_label(M::TYPE_ID_JSON)?;
        let handle = self.session.subtypes(&label).map_err(Error::from_orm)?;
        self.push_binding(handle)
    }

    pub(crate) fn handle_by_key(&self, key: BindingKey) -> Result<&OrmBindingHandle> {
        self.ensure_open(ModelValidationPhase::Input)?;
        if key.nonce != self.nonce {
            return Err(cross_session_handle());
        }
        self.bindings
            .get(key.index as usize)
            .ok_or_else(cross_session_handle)
    }

    /// Brand one generated projected attribute wrapper as a scalar
    /// schema-function input. Generated packages expose domain-specific
    /// constructors and keep this generic seam out of their public vocabulary.
    #[doc(hidden)]
    pub fn __function_input<Value>(&self, value: &Value) -> Result<FunctionInput<S, Value::Domain>>
    where
        Value: Model<Schema = S> + QueryValued,
    {
        self.ensure_open(ModelValidationPhase::Input)?;
        let attribute_type = from_canonical_json::<TypeId>(Value::TYPE_ID_JSON.as_bytes())
            .map_err(|source| {
                Error::model_validation(
                    ModelValidationPhase::Input,
                    "invalid_function_attribute_identity",
                    vec!["function".into()],
                    "generated function input attribute identity is not canonical",
                    Some(Box::new(source)),
                )
            })?;
        if attribute_type.kind() != TypeKind::Attribute {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "function_input_not_attribute",
                vec!["function".into()],
                "generated scalar function input must be a projected attribute wrapper",
                None,
            ));
        }
        let expression = FunctionInputExpr {
            attribute_type,
            value: encoded_query_operand(value.into_encoded_scalar()),
        };
        self.lower_function_input(&expression)?;
        Ok(FunctionInput {
            nonce: self.nonce,
            expression,
            marker: PhantomData,
        })
    }

    /// Construct one exact projected scalar function call from a generated
    /// nominal function token and already session-branded arguments.
    #[doc(hidden)]
    pub fn __call_function<Arguments, Output>(
        &self,
        function: FunctionToken<S, Arguments, Output>,
        arguments: impl IntoIterator<Item = FunctionArgument<S>>,
    ) -> Result<FunctionCall<S, Output>> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        if arguments
            .iter()
            .any(|argument| argument.nonce != self.nonce)
        {
            return Err(cross_session_handle());
        }
        let expression = FunctionCallExpr {
            function_id: function.__function_id().to_owned(),
            arguments: arguments
                .into_iter()
                .map(|argument| argument.expression)
                .collect(),
        };
        self.lower_function_call(&expression)?;
        Ok(FunctionCall {
            nonce: self.nonce,
            expression,
            marker: PhantomData,
        })
    }

    fn lower_function_input(
        &self,
        input: &FunctionInputExpr,
    ) -> Result<type_bridge_orm::FunctionValueHandle> {
        let projected = ProjectedAttributeValue::try_from_attribute_value(
            self.installed,
            input.attribute_type.clone(),
            &input.value,
        )
        .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))?;
        self.session
            .function_value(&projected)
            .map_err(Error::from_orm)
    }

    fn lower_function_argument(
        &self,
        argument: &FunctionArgumentExpr,
    ) -> Result<OrmFunctionArgumentHandle> {
        match argument {
            FunctionArgumentExpr::Binding(binding) => {
                Ok(self.handle_by_key(*binding)?.function_argument())
            }
            FunctionArgumentExpr::Value(value) => {
                Ok(self.lower_function_input(value)?.function_argument())
            }
            FunctionArgumentExpr::Call(call) => {
                Ok(self.lower_function_call(call)?.function_argument())
            }
        }
    }

    fn lower_function_call(&self, call: &FunctionCallExpr) -> Result<OrmFunctionCallHandle> {
        let id = FunctionId::new(call.function_id.clone()).map_err(|source| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_function_identity",
                vec!["function".into()],
                "generated function identity is not canonical",
                Some(Box::new(source)),
            )
        })?;
        let function = self.session.function(&id).map_err(Error::from_orm)?;
        let arguments = call
            .arguments
            .iter()
            .map(|argument| self.lower_function_argument(argument))
            .collect::<Result<Vec<_>>>()?;
        function.call(arguments).map_err(Error::from_orm)
    }

    fn installed(&self) -> Result<&InstalledRuntimeProjection> {
        Ok(self.installed)
    }

    fn field_name_for(&self, owner_label: &str, attribute_label: &str) -> Result<String> {
        let descriptor = self.registry.get(owner_label).ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "unknown_field_owner",
                vec!["type".into()],
                format!("field owner '{owner_label}' is not registered in this session"),
                None,
            )
        })?;
        let found = match &descriptor {
            TypeDescriptorRef::Entity(entity) => entity
                .owned_attributes
                .iter()
                .find(|attribute| attribute.attr_name == attribute_label)
                .map(|attribute| attribute.field_name.clone()),
            TypeDescriptorRef::Relation(relation) => relation
                .owned_attributes
                .iter()
                .find(|attribute| attribute.attr_name == attribute_label)
                .map(|attribute| attribute.field_name.clone()),
        };
        found.ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "unknown_field",
                vec!["type".into()],
                format!("owner '{owner_label}' has no field for attribute '{attribute_label}'"),
                None,
            )
        })
    }

    pub(crate) fn lower_field(
        &self,
        key: BindingKey,
        owns_id_json: &'static str,
    ) -> Result<OrmFieldHandle> {
        let handle = self.handle_by_key(key)?;
        let (owner_label, attribute_label) = parse_owns_identity(owns_id_json)?;
        let field_name = self.field_name_for(&owner_label, &attribute_label)?;
        handle
            .field_owned_by(&owner_label, &field_name)
            .map_err(Error::from_orm)
    }

    fn lower_order(&self, order: &Order<S>) -> Result<OrmOrderHandle> {
        let field = self.lower_field(order.key, order.owns_id_json)?;
        Ok(field.order(order.direction, order.missing))
    }

    fn lower_predicate(&self, expr: &PredicateExpr) -> Result<OrmPredicateHandle> {
        match expr {
            PredicateExpr::FieldValue {
                binding,
                owns_id_json,
                operator,
                value,
            } => {
                let field = self.lower_field(*binding, owns_id_json)?;
                Ok(field.compare_value(*operator, value.clone()))
            }
            PredicateExpr::FieldField {
                left_binding,
                left_owns_id_json,
                operator,
                right_binding,
                right_owns_id_json,
            } => {
                let left = self.lower_field(*left_binding, left_owns_id_json)?;
                let right = self.lower_field(*right_binding, right_owns_id_json)?;
                left.compare_field(*operator, &right)
                    .map_err(Error::from_orm)
            }
            PredicateExpr::FieldPresence {
                binding,
                owns_id_json,
                present,
            } => Ok(self.lower_field(*binding, owns_id_json)?.presence(*present)),
            PredicateExpr::FunctionField {
                call,
                operator,
                binding,
                owns_id_json,
            } => self
                .lower_function_call(call)?
                .compare_field(*operator, &self.lower_field(*binding, owns_id_json)?)
                .map_err(Error::from_orm),
            PredicateExpr::FunctionValue {
                call,
                operator,
                value,
            } => self
                .lower_function_call(call)?
                .compare_value(*operator, &self.lower_function_input(value)?)
                .map_err(Error::from_orm),
            PredicateExpr::FunctionCall {
                left,
                operator,
                right,
            } => self
                .lower_function_call(left)?
                .compare_call(*operator, &self.lower_function_call(right)?)
                .map_err(Error::from_orm),
            PredicateExpr::BindingIid { binding, iid } => self
                .handle_by_key(*binding)?
                .iid(iid.clone())
                .map_err(Error::from_orm),
            PredicateExpr::BindingIidIn { binding, iids } => self
                .handle_by_key(*binding)?
                .iid_in(iids.clone())
                .map_err(Error::from_orm),
            PredicateExpr::Connects {
                relation,
                role_id_json,
                player,
            } => {
                let relation_handle = self.handle_by_key(*relation)?;
                let role = parse_role_identity(role_id_json)?;
                let role_handle = relation_handle
                    .role_owned_by(role.declaring_relation().as_str(), role.label().as_str())
                    .map_err(Error::from_orm)?;
                let player_handle = self.handle_by_key(*player)?;
                role_handle.connects(player_handle).map_err(Error::from_orm)
            }
            PredicateExpr::Reachable {
                relation_type_id_json,
                role_from_id_json,
                role_to_id_json,
                source,
                target,
                min_depth,
                max_depth,
            } => {
                let relation = model_label(relation_type_id_json)?;
                let role_from = parse_role_identity(role_from_id_json)?;
                let role_to = parse_role_identity(role_to_id_json)?;
                self.session
                    .reachable(
                        &relation,
                        role_from.label().as_str(),
                        role_to.label().as_str(),
                        self.handle_by_key(*source)?,
                        self.handle_by_key(*target)?,
                        *min_depth,
                        *max_depth,
                    )
                    .map_err(Error::from_orm)
            }
            PredicateExpr::And(terms) => self.lower_composed(terms, |left, right| {
                left.and(right).map_err(Error::from_orm)
            }),
            PredicateExpr::Or(terms) => {
                self.lower_composed(terms, |left, right| left.or(right).map_err(Error::from_orm))
            }
            PredicateExpr::Not(inner) => Ok(self.lower_predicate(inner)?.not()),
        }
    }

    fn lower_composed(
        &self,
        terms: &[PredicateExpr],
        combine: impl Fn(&OrmPredicateHandle, &OrmPredicateHandle) -> Result<OrmPredicateHandle>,
    ) -> Result<OrmPredicateHandle> {
        let mut lowered = terms.iter().map(|term| self.lower_predicate(term));
        let mut combined = lowered.next().ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "empty_predicate",
                vec![],
                "boolean composition requires at least one predicate",
                None,
            )
        })??;
        for term in lowered {
            combined = combine(&combined, &term?)?;
        }
        Ok(combined)
    }

    fn client_row_for(&self, thing: &HydratedThing) -> Result<HydratedRow> {
        use type_bridge_orm::match_request::model::ThingKind as OrmThingKind;
        let installed = self.installed()?;
        let type_name = self
            .registry
            .descriptor_type_name(thing.concrete_descriptor())
            .ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "invalid_installed_projection",
                    vec!["type".into()],
                    "selected concrete descriptor is absent from the installed registry",
                    None,
                )
            })?;
        let mut attributes = Vec::new();
        for attribute in thing.attributes() {
            let provider_name = self
                .registry
                .provider_attribute_name(attribute.field())
                .ok_or_else(|| {
                    Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "invalid_installed_projection",
                        vec![],
                        "selected field identity has no provider attribute name",
                        None,
                    )
                })?;
            for value in attribute.values() {
                attributes.push((
                    provider_name.clone(),
                    canonicalize_selected_value(value.clone())?,
                ));
            }
        }
        match thing.kind() {
            OrmThingKind::Entity => {
                let id = TypeId::new(type_bridge_contract::id::TypeKind::Entity, &type_name)
                    .map_err(|source| {
                        Error::model_validation(
                            ModelValidationPhase::Hydration,
                            "invalid_discovered_type",
                            vec!["type".into()],
                            "selected entity type label is invalid",
                            Some(Box::new(source)),
                        )
                    })?;
                hydrate_entity(
                    DynamicEntityRow {
                        iid: Some(thing.concept_id().as_str().to_owned()),
                        type_name: Some(type_name),
                        attributes,
                    },
                    &id,
                    installed,
                )
            }
            OrmThingKind::Relation => {
                let id = TypeId::new(type_bridge_contract::id::TypeKind::Relation, &type_name)
                    .map_err(|source| {
                        Error::model_validation(
                            ModelValidationPhase::Hydration,
                            "invalid_discovered_type",
                            vec!["type".into()],
                            "selected relation type label is invalid",
                            Some(Box::new(source)),
                        )
                    })?;
                let mut role_players = Vec::new();
                for role in thing.roles() {
                    for player in role.players() {
                        let mut raw = Vec::new();
                        for attribute in player.attributes() {
                            let provider_name = self
                                .registry
                                .provider_attribute_name(attribute.field())
                                .ok_or_else(|| {
                                    Error::model_validation(
                                        ModelValidationPhase::Hydration,
                                        "invalid_installed_projection",
                                        vec![],
                                        "player field identity has no provider attribute name",
                                        None,
                                    )
                                })?;
                            for value in attribute.values() {
                                raw.push((
                                    provider_name.clone(),
                                    plain_json(&canonicalize_selected_value(value.clone())?),
                                ));
                            }
                        }
                        let player_type_name = self
                            .registry
                            .descriptor_type_name(player.concrete_descriptor())
                            .ok_or_else(|| {
                                Error::model_validation(
                                    ModelValidationPhase::Hydration,
                                    "invalid_installed_projection",
                                    vec!["roles".into(), role.role().name.clone()],
                                    "selected role-player descriptor is absent from the installed registry",
                                    None,
                                )
                            })?;
                        role_players.push(DynamicRolePlayer {
                            role_name: role.role().name.clone(),
                            player_iid: Some(player.concept_id().as_str().to_owned()),
                            player_type_name: Some(player_type_name),
                            attributes: raw,
                        });
                    }
                }
                hydrate_relation(
                    DynamicRelationRow {
                        iid: Some(thing.concept_id().as_str().to_owned()),
                        type_name: Some(type_name),
                        attributes,
                        role_players,
                    },
                    &id,
                    installed,
                )
            }
        }
    }
}

fn canonicalize_selected_value(value: AttributeValue) -> Result<AttributeValue> {
    let malformed = || {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "hydrated_attribute_value_type",
            vec![],
            "selected attribute value is outside its canonical scalar domain",
            None,
        )
    };
    match value {
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(|value| AttributeValue::Date(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTime(value) => normalize_provider_fraction(value)
            .parse::<CanonicalDateTime>()
            .map(|value| AttributeValue::DateTime(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTimeTZ(value) => normalize_provider_datetime_tz(value)
            .parse::<CanonicalDateTimeTz>()
            .map(|value| AttributeValue::DateTimeTZ(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::Decimal(value) => parse_decimal(&value)
            .map(|value| AttributeValue::Decimal(value.canonical_string()))
            .ok_or_else(malformed),
        AttributeValue::Duration(value) => {
            let value = normalize_provider_fraction(value);
            match value.parse::<CanonicalDuration>() {
                Ok(value) => Ok(AttributeValue::Duration(value.to_string())),
                Err(_) => CompatibilityValueV2::released_duration(value.clone())
                    .map(|_| AttributeValue::Duration(value))
                    .map_err(|_| malformed()),
            }
        }
        value => Ok(value),
    }
}

fn encoded_group_scalar(value: &AttributeValue) -> Result<EncodedScalar> {
    let value = canonicalize_selected_value(value.clone())?;
    let map = |error| map_validation_error(error, ModelValidationPhase::Hydration);
    match value {
        AttributeValue::String(value) => Ok(EncodedScalar::String(value)),
        AttributeValue::Long(value) => Ok(EncodedScalar::Long(value)),
        AttributeValue::Double(value) => crate::__codegen::CanonicalDouble::try_new(value)
            .map(EncodedScalar::Double)
            .map_err(map),
        AttributeValue::Boolean(value) => Ok(EncodedScalar::Boolean(value)),
        AttributeValue::Date(value) => crate::__codegen::Date::try_new(value)
            .map(EncodedScalar::Date)
            .map_err(map),
        AttributeValue::DateTime(value) => crate::__codegen::DateTime::try_new(value)
            .map(EncodedScalar::DateTime)
            .map_err(map),
        AttributeValue::DateTimeTZ(value) => crate::__codegen::DateTimeTz::try_new(value)
            .map(EncodedScalar::DateTimeTz)
            .map_err(map),
        AttributeValue::Decimal(value) => crate::__codegen::Decimal::try_new(value)
            .map(EncodedScalar::Decimal)
            .map_err(map),
        AttributeValue::Duration(value) => crate::__codegen::Duration::try_new(value)
            .map(EncodedScalar::Duration)
            .map_err(map),
    }
}

fn normalize_provider_datetime_tz(value: String) -> String {
    let mut normalized = normalize_provider_fraction(value);
    for zero_offset in ["+00:00:00", "-00:00:00", "+00:00", "-00:00"] {
        if normalized.ends_with(zero_offset) {
            normalized.truncate(normalized.len() - zero_offset.len());
            normalized.push('Z');
            break;
        }
    }
    normalized
}

fn normalize_provider_fraction(value: String) -> String {
    let Some(dot) = value.find('.') else {
        return value;
    };
    let fraction_end = value[dot + 1..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(value.len(), |offset| dot + 1 + offset);
    let trimmed_end = value[dot + 1..fraction_end].trim_end_matches('0').len() + dot + 1;
    if trimmed_end == fraction_end {
        return value;
    }
    let mut normalized = String::with_capacity(value.len());
    normalized.push_str(
        &value[..if trimmed_end == dot + 1 {
            dot
        } else {
            trimmed_end
        }],
    );
    normalized.push_str(&value[fraction_end..]);
    normalized
}

fn plain_json(value: &AttributeValue) -> serde_json::Value {
    match value {
        AttributeValue::String(value) => serde_json::Value::String(value.clone()),
        AttributeValue::Long(value) => serde_json::Value::from(*value),
        AttributeValue::Double(value) => {
            serde_json::Number::from_f64(*value).map_or(serde_json::Value::Null, Into::into)
        }
        AttributeValue::Boolean(value) => serde_json::Value::Bool(*value),
        AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => serde_json::Value::String(value.clone()),
    }
}

/// Sealed conversion from client literals and generated value wrappers into
/// canonical query operands.
pub trait QueryOperand: operand_sealed::Sealed {
    #[doc(hidden)]
    type Domain;

    #[doc(hidden)]
    fn into_operand(self) -> AttributeValue;
}

/// Sealed marker for canonically ordered query operands.
pub trait OrderedOperand: QueryOperand {}

mod operand_sealed {
    pub trait Sealed {}
}

macro_rules! operand {
    ($ty:ty, $domain:ty, $self_:ident => $convert:expr, ordered: $ordered:tt) => {
        impl operand_sealed::Sealed for $ty {}
        impl QueryOperand for $ty {
            type Domain = $domain;

            fn into_operand($self_) -> AttributeValue {
                $convert
            }
        }
        operand!(@ordered $ty, $ordered);
    };
    (@ordered $ty:ty, true) => {
        impl OrderedOperand for $ty {}
    };
    (@ordered $ty:ty, false) => {};
}

fn encoded_query_operand(value: EncodedScalar) -> AttributeValue {
    match value {
        EncodedScalar::String(value) => AttributeValue::String(value),
        EncodedScalar::Long(value) => AttributeValue::Long(value),
        EncodedScalar::Double(value) => AttributeValue::Double(value.get()),
        EncodedScalar::Boolean(value) => AttributeValue::Boolean(value),
        EncodedScalar::Date(value) => AttributeValue::Date(value.as_str().to_owned()),
        EncodedScalar::DateTime(value) => AttributeValue::DateTime(value.as_str().to_owned()),
        EncodedScalar::DateTimeTz(value) => AttributeValue::DateTimeTZ(value.as_str().to_owned()),
        EncodedScalar::Decimal(value) => AttributeValue::Decimal(value.as_str().to_owned()),
        EncodedScalar::Duration(value) => AttributeValue::Duration(value.as_str().to_owned()),
    }
}

impl<T: QueryValued> operand_sealed::Sealed for T {}
impl<T: QueryValued> QueryOperand for T {
    type Domain = T::Domain;

    fn into_operand(self) -> AttributeValue {
        encoded_query_operand(self.into_encoded_scalar())
    }
}

impl OrderedOperand for i64 {}

operand!(
    crate::value::Text,
    String,
    self => AttributeValue::String(self.into_string()),
    ordered: false
);
operand!(
    crate::value::Double,
    crate::__codegen::CanonicalDouble,
    self => AttributeValue::Double(self.get()),
    ordered: true
);
operand!(
    crate::value::Decimal,
    crate::__codegen::Decimal,
    self => AttributeValue::Decimal(self.into_string()),
    ordered: true
);
operand!(
    crate::value::Date,
    crate::__codegen::Date,
    self => AttributeValue::Date(self.into_string()),
    ordered: true
);
operand!(
    crate::value::DateTime,
    crate::__codegen::DateTime,
    self => AttributeValue::DateTime(self.into_string()),
    ordered: true
);
operand!(
    crate::value::DateTimeTz,
    crate::__codegen::DateTimeTz,
    self => AttributeValue::DateTimeTZ(self.into_string()),
    ordered: true
);
operand!(
    crate::value::Duration,
    crate::__codegen::Duration,
    self => AttributeValue::Duration(self.into_string()),
    ordered: true
);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PredicateExpr {
    FieldValue {
        binding: BindingKey,
        owns_id_json: &'static str,
        operator: ComparisonOp,
        value: AttributeValue,
    },
    FieldField {
        left_binding: BindingKey,
        left_owns_id_json: &'static str,
        operator: ComparisonOp,
        right_binding: BindingKey,
        right_owns_id_json: &'static str,
    },
    FieldPresence {
        binding: BindingKey,
        owns_id_json: &'static str,
        present: bool,
    },
    FunctionField {
        call: FunctionCallExpr,
        operator: ComparisonOp,
        binding: BindingKey,
        owns_id_json: &'static str,
    },
    FunctionValue {
        call: FunctionCallExpr,
        operator: ComparisonOp,
        value: FunctionInputExpr,
    },
    FunctionCall {
        left: FunctionCallExpr,
        operator: ComparisonOp,
        right: FunctionCallExpr,
    },
    BindingIid {
        binding: BindingKey,
        iid: String,
    },
    BindingIidIn {
        binding: BindingKey,
        iids: Vec<String>,
    },
    Connects {
        relation: BindingKey,
        role_id_json: &'static str,
        player: BindingKey,
    },
    Reachable {
        relation_type_id_json: &'static str,
        role_from_id_json: &'static str,
        role_to_id_json: &'static str,
        source: BindingKey,
        target: BindingKey,
        min_depth: u8,
        max_depth: u8,
    },
    And(Vec<PredicateExpr>),
    Or(Vec<PredicateExpr>),
    Not(Box<PredicateExpr>),
}

/// One schema-branded, composable query predicate.
///
/// Operators are domain-restricted at construction; predicates compose with
/// `&`, `|`, and `!` (or the named [`Predicate::and`], [`Predicate::or`],
/// and [`Predicate::not`]) and are validated against the owning session
/// before any executor invocation.
#[derive(Debug, PartialEq)]
pub struct Predicate<S: Schema> {
    pub(crate) expr: PredicateExpr,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Clone for Predicate<S> {
    fn clone(&self) -> Self {
        Self {
            expr: self.expr.clone(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema> Predicate<S> {
    fn new(expr: PredicateExpr) -> Self {
        Self {
            expr,
            marker: PhantomData,
        }
    }

    /// Conjunction; equivalent to `self & other`.
    #[must_use]
    pub fn and(self, other: Predicate<S>) -> Predicate<S> {
        self & other
    }

    /// Disjunction; equivalent to `self | other`.
    #[must_use]
    pub fn or(self, other: Predicate<S>) -> Predicate<S> {
        self | other
    }

    /// Negation; equivalent to `!self`.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Predicate<S> {
        !self
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    /// Require a bounded directed walk between two generated endpoint
    /// bindings through one exact generated relation.
    ///
    /// Each hop follows `role_from -> role_to`. Bounds are inclusive and a
    /// zero-hop branch requires identical endpoint concepts. Generated role
    /// compatibility and player-union evidence reject inactive roles and
    /// invalid endpoint models at compile time; bounds, session ownership,
    /// and installed-schema compatibility are validated before provider I/O.
    #[allow(clippy::too_many_arguments)]
    pub fn reachable<
        R,
        FromOwner,
        FromPlayers,
        ToOwner,
        ToPlayers,
        Source,
        SourceMode,
        Target,
        TargetMode,
    >(
        &self,
        relation: TypeToken<R>,
        role_from: RoleToken<FromOwner, FromPlayers>,
        role_to: RoleToken<ToOwner, ToPlayers>,
        source: Binding<S, Source, SourceMode>,
        target: Binding<S, Target, TargetMode>,
        min_depth: u8,
        max_depth: u8,
    ) -> Result<Predicate<S>>
    where
        R: RelationModel<Schema = S>
            + CompleteModel
            + RoleTokenCompatible<FromOwner, FromPlayers>
            + RoleTokenCompatible<ToOwner, ToPlayers>,
        FromOwner: RelationModel<Schema = S>,
        ToOwner: RelationModel<Schema = S>,
        FromPlayers: RolePlayerBinding<Source, SourceMode>,
        ToPlayers: RolePlayerBinding<Target, TargetMode>,
        Source: ThingModel<Schema = S>,
        SourceMode: SelectionMode,
        Target: ThingModel<Schema = S>,
        TargetMode: SelectionMode,
    {
        let predicate = Predicate::new(PredicateExpr::Reachable {
            relation_type_id_json: relation.type_id_json(),
            role_from_id_json: role_from.role_id_json(),
            role_to_id_json: role_to.role_id_json(),
            source: source.key,
            target: target.key,
            min_depth,
            max_depth,
        });
        self.lower_predicate(&predicate.expr)?;
        Ok(predicate)
    }
}

impl<S: Schema> std::ops::BitAnd for Predicate<S> {
    type Output = Predicate<S>;
    fn bitand(self, other: Predicate<S>) -> Predicate<S> {
        let mut terms = match self.expr {
            PredicateExpr::And(terms) => terms,
            expr => vec![expr],
        };
        match other.expr {
            PredicateExpr::And(more) => terms.extend(more),
            expr => terms.push(expr),
        }
        Predicate::new(PredicateExpr::And(terms))
    }
}

impl<S: Schema> std::ops::BitOr for Predicate<S> {
    type Output = Predicate<S>;
    fn bitor(self, other: Predicate<S>) -> Predicate<S> {
        let mut terms = match self.expr {
            PredicateExpr::Or(terms) => terms,
            expr => vec![expr],
        };
        match other.expr {
            PredicateExpr::Or(more) => terms.extend(more),
            expr => terms.push(expr),
        }
        Predicate::new(PredicateExpr::Or(terms))
    }
}

impl<S: Schema> std::ops::Not for Predicate<S> {
    type Output = Predicate<S>;
    fn not(self) -> Predicate<S> {
        Predicate::new(PredicateExpr::Not(Box::new(self.expr)))
    }
}

/// One generated field resolved against one session binding occurrence.
///
/// The token retains its declaring owner; owner/binding compatibility is
/// enforced against the installed registry when the predicate is lowered,
/// before any I/O.
pub struct BoundField<S: Schema, Owner: Model<Schema = S>, V> {
    key: BindingKey,
    owns_id_json: &'static str,
    marker: PhantomData<fn() -> (Owner, V)>,
}

impl<S: Schema, Owner: Model<Schema = S>, V> Copy for BoundField<S, Owner, V> {}
impl<S: Schema, Owner: Model<Schema = S>, V> Clone for BoundField<S, Owner, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema, M: ThingModel<Schema = S>, Mode: SelectionMode> Binding<S, M, Mode> {
    /// Resolve one generated owned field against this binding occurrence.
    ///
    /// The declaring owner must be this binding's model or a generated
    /// nominal ancestor; unrelated same-spelled owners fail to type-check,
    /// and the installed registry re-validates the admission at lowering.
    #[must_use]
    pub fn field<Owner, V>(self, token: FieldToken<Owner, V>) -> BoundField<S, Owner, V>
    where
        Owner: Model<Schema = S>,
        M: crate::__codegen::NominalUpcast<Owner>,
    {
        BoundField {
            key: self.key,
            owns_id_json: token.owns_id_json(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, M: RelationModel<Schema = S> + ThingModel<Schema = S>, Mode: SelectionMode>
    Binding<S, M, Mode>
{
    /// Resolve one active generated relation role against this relation
    /// binding occurrence. Only relation bindings expose roles;
    /// specialized-away ancestor tokens have no generated compatibility
    /// evidence.
    #[must_use]
    pub fn role<Owner, Players>(
        self,
        token: RoleToken<Owner, Players>,
    ) -> BoundRole<S, Owner, Players>
    where
        Owner: RelationModel<Schema = S>,
        M: RoleTokenCompatible<Owner, Players>,
    {
        BoundRole {
            key: self.key,
            role_id_json: token.role_id_json(),
            marker: PhantomData,
        }
    }
}

impl<S: Schema, Owner: Model<Schema = S>, V> BoundField<S, Owner, V> {
    pub(crate) fn reduction_input(self) -> (BindingKey, &'static str) {
        (self.key, self.owns_id_json)
    }

    fn value_predicate(self, operator: ComparisonOp, value: AttributeValue) -> Predicate<S> {
        Predicate::new(PredicateExpr::FieldValue {
            binding: self.key,
            owns_id_json: self.owns_id_json,
            operator,
            value,
        })
    }

    /// Equality against a canonical literal of the field's scalar domain.
    #[must_use]
    pub fn eq<O>(self, operand: O) -> Predicate<S>
    where
        V: QueryValued,
        O: QueryOperand<Domain = V::Domain>,
    {
        self.value_predicate(ComparisonOp::Equal, operand.into_operand())
    }

    /// Inequality against a canonical literal of the field's scalar domain.
    #[must_use]
    pub fn ne<O>(self, operand: O) -> Predicate<S>
    where
        V: QueryValued,
        O: QueryOperand<Domain = V::Domain>,
    {
        self.value_predicate(ComparisonOp::NotEqual, operand.into_operand())
    }

    /// Strictly-less ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn lt(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::LessThan, operand.into_operand())
    }

    /// Less-or-equal ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn le(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::LessThanOrEqual, operand.into_operand())
    }

    /// Strictly-greater ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn gt(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::GreaterThan, operand.into_operand())
    }

    /// Greater-or-equal ordering against a canonically ordered literal;
    /// admitted only for canonically ordered field domains.
    #[must_use]
    pub fn ge(self, operand: impl OrderedOperand) -> Predicate<S>
    where
        V: crate::__codegen::OrderedValued,
    {
        self.value_predicate(ComparisonOp::GreaterThanOrEqual, operand.into_operand())
    }

    /// Text containment against bounded canonical text; admitted only for
    /// text field domains.
    #[must_use]
    pub fn contains(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::Contains,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Anchored text prefix against bounded canonical text; admitted only
    /// for text field domains.
    #[must_use]
    pub fn starts_with(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::StartsWith,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Anchored text suffix against bounded canonical text; admitted only
    /// for text field domains.
    #[must_use]
    pub fn ends_with(self, text: crate::value::Text) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::EndsWith,
            AttributeValue::String(text.into_string()),
        )
    }

    /// Regular-expression match against a client-owned validated pattern;
    /// admitted only for text field domains.
    #[must_use]
    pub fn regex(self, pattern: crate::value::Regex) -> Predicate<S>
    where
        V: crate::__codegen::TextValued,
    {
        self.value_predicate(
            ComparisonOp::Regex,
            AttributeValue::String(pattern.into_string()),
        )
    }

    /// Require at least one owned value for this generated field.
    #[must_use]
    pub fn is_present(self) -> Predicate<S> {
        Predicate::new(PredicateExpr::FieldPresence {
            binding: self.key,
            owns_id_json: self.owns_id_json,
            present: true,
        })
    }

    /// Require no owned value for this generated field.
    #[must_use]
    pub fn is_missing(self) -> Predicate<S> {
        Predicate::new(PredicateExpr::FieldPresence {
            binding: self.key,
            owns_id_json: self.owns_id_json,
            present: false,
        })
    }

    fn field_predicate<Owner2, V2>(
        self,
        operator: ComparisonOp,
        other: BoundField<S, Owner2, V2>,
    ) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
    {
        Predicate::new(PredicateExpr::FieldField {
            left_binding: self.key,
            left_owns_id_json: self.owns_id_json,
            operator,
            right_binding: other.key,
            right_owns_id_json: other.owns_id_json,
        })
    }

    /// Compare for equality against another bound field in the same canonical
    /// scalar domain; the comparison carries no literal.
    #[must_use]
    pub fn eq_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued,
        V2: QueryValued<Domain = V::Domain>,
    {
        self.field_predicate(ComparisonOp::Equal, other)
    }

    /// Compare for inequality against another bound field in the same
    /// canonical scalar domain.
    #[must_use]
    pub fn ne_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued,
        V2: QueryValued<Domain = V::Domain>,
    {
        self.field_predicate(ComparisonOp::NotEqual, other)
    }

    /// Compare as less than another bound field in the same ordered canonical
    /// scalar domain.
    #[must_use]
    pub fn lt_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued + crate::__codegen::OrderedValued,
        V2: QueryValued<Domain = V::Domain> + crate::__codegen::OrderedValued,
    {
        self.field_predicate(ComparisonOp::LessThan, other)
    }

    /// Compare as less than or equal to another bound field in the same
    /// ordered canonical scalar domain.
    #[must_use]
    pub fn le_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued + crate::__codegen::OrderedValued,
        V2: QueryValued<Domain = V::Domain> + crate::__codegen::OrderedValued,
    {
        self.field_predicate(ComparisonOp::LessThanOrEqual, other)
    }

    /// Compare as greater than another bound field in the same ordered
    /// canonical scalar domain.
    #[must_use]
    pub fn gt_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued + crate::__codegen::OrderedValued,
        V2: QueryValued<Domain = V::Domain> + crate::__codegen::OrderedValued,
    {
        self.field_predicate(ComparisonOp::GreaterThan, other)
    }

    /// Compare as greater than or equal to another bound field in the same
    /// ordered canonical scalar domain.
    #[must_use]
    pub fn ge_field<Owner2, V2>(self, other: BoundField<S, Owner2, V2>) -> Predicate<S>
    where
        Owner2: Model<Schema = S>,
        V: QueryValued + crate::__codegen::OrderedValued,
        V2: QueryValued<Domain = V::Domain> + crate::__codegen::OrderedValued,
    {
        self.field_predicate(ComparisonOp::GreaterThanOrEqual, other)
    }

    /// Order ascending by this bound field; missing keys fail closed unless
    /// an explicit missing-value policy is admitted.
    #[must_use]
    pub fn asc(self) -> Order<S> {
        Order {
            key: self.key,
            owns_id_json: self.owns_id_json,
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
            marker: PhantomData,
        }
    }

    /// Order descending by this bound field; missing keys fail closed unless
    /// an explicit missing-value policy is admitted.
    #[must_use]
    pub fn desc(self) -> Order<S> {
        Order {
            key: self.key,
            owns_id_json: self.owns_id_json,
            direction: SortDirection::Descending,
            missing: MissingOrder::Reject,
            marker: PhantomData,
        }
    }
}

/// One stable public ordering term over a bound field.
#[derive(Debug)]
pub struct Order<S: Schema> {
    key: BindingKey,
    owns_id_json: &'static str,
    direction: SortDirection,
    missing: MissingOrder,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Copy for Order<S> {}
impl<S: Schema> Clone for Order<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema> Order<S> {
    /// Admit missing keys and place them before all present keys.
    #[must_use]
    pub fn missing_first(mut self) -> Self {
        self.missing = MissingOrder::First;
        self
    }

    /// Admit missing keys and place them after all present keys.
    #[must_use]
    pub fn missing_last(mut self) -> Self {
        self.missing = MissingOrder::Last;
        self
    }
}

/// One typed collection selection used inside a distinct-root page shape.
pub struct Collected<S: Schema, B: Selectable<S>> {
    selection: B,
    distinct: bool,
    order: Vec<Order<S>>,
}

impl<S: Schema, B: Selectable<S>> Clone for Collected<S, B> {
    fn clone(&self) -> Self {
        Self {
            selection: self.selection,
            distinct: self.distinct,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema, B: Selectable<S>> Collected<S, B> {
    /// Deduplicate collection members by TypeDB concept identity.
    #[must_use]
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Append one stable order term owned by this collected binding.
    pub fn order_by(mut self, order: Order<S>) -> Result<Self> {
        if order.key != self.selection.binding_key() {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "collection_order_binding_mismatch",
                vec![],
                "collection ordering must reference the collected binding",
                None,
            ));
        }
        self.order.push(order);
        Ok(self)
    }
}

/// One generated relation role resolved against one relation binding
/// occurrence.
pub struct BoundRole<S: Schema, Owner: Model<Schema = S>, Players> {
    key: BindingKey,
    role_id_json: &'static str,
    marker: PhantomData<fn() -> (Owner, Players)>,
}

impl<S: Schema, Owner: Model<Schema = S>, Players> Copy for BoundRole<S, Owner, Players> {}
impl<S: Schema, Owner: Model<Schema = S>, Players> Clone for BoundRole<S, Owner, Players> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema, Owner: Model<Schema = S>, Players> BoundRole<S, Owner, Players> {
    /// Require this relation role to connect an admitted generated player
    /// binding.
    #[must_use]
    pub fn connects<MP: ThingModel<Schema = S>, ModeP: SelectionMode>(
        self,
        player: Binding<S, MP, ModeP>,
    ) -> Predicate<S>
    where
        Players: RolePlayerBinding<MP, ModeP>,
    {
        Predicate::new(PredicateExpr::Connects {
            relation: self.key,
            role_id_json: self.role_id_json,
            player: player.key,
        })
    }
}

mod selectable_sealed {
    pub trait Sealed {}
}

/// Sealed resolution from one selected binding to its typed query output.
pub trait Selectable<S: Schema>: selectable_sealed::Sealed + Copy {
    /// The materialized output type for one selected row.
    type Output;
    #[doc(hidden)]
    fn binding_key(self) -> BindingKey;
    #[doc(hidden)]
    fn materialize_output(row: &HydratedRow) -> std::result::Result<Self::Output, ValidationError>;

    #[doc(hidden)]
    fn __selection_handle(self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        Ok(session.handle_by_key(self.binding_key())?.one())
    }

    #[doc(hidden)]
    fn __materialize_slot(
        self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        let SlotValue::One(thing) = slot else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a collection slot for a singular selection",
                None,
            ));
        };
        let row = session.client_row_for(thing)?;
        Self::materialize_output(&row)
            .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
    }

    #[doc(hidden)]
    fn __materialize_slot_with_checkpoint(
        self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        checkpoint()?;
        let output = self.__materialize_slot(session, slot)?;
        checkpoint()?;
        Ok(output)
    }
}

impl<S: Schema, M: ThingModel<Schema = S> + CompleteModel> selectable_sealed::Sealed
    for Binding<S, M, Exact>
{
}
impl<S: Schema, M: ThingModel<Schema = S> + CompleteModel> Selectable<S> for Binding<S, M, Exact> {
    type Output = M;
    fn binding_key(self) -> BindingKey {
        self.key()
    }
    fn materialize_output(row: &HydratedRow) -> std::result::Result<M, ValidationError> {
        M::materialize(row, &HydrationCapability::new())
    }
}

impl<S: Schema, M: ThingModel<Schema = S> + SubtypeRootModel> selectable_sealed::Sealed
    for Binding<S, M, Subtypes>
{
}
impl<S: Schema, M: ThingModel<Schema = S> + SubtypeRootModel> Selectable<S>
    for Binding<S, M, Subtypes>
{
    type Output = M::Subtypes;
    fn binding_key(self) -> BindingKey {
        self.key()
    }
    fn materialize_output(row: &HydratedRow) -> std::result::Result<M::Subtypes, ValidationError> {
        M::__tb_dispatch_subtype(row, &HydrationCapability::new())
    }
}

mod selected_slot_sealed {
    pub trait Sealed<S> {}
}

/// Sealed resolution from one singular or collected selection to its typed
/// slot output.
#[doc(hidden)]
pub trait SelectedSlot<S: Schema>: selected_slot_sealed::Sealed<S> + Clone {
    type Output;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle>;

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output>;

    #[doc(hidden)]
    fn __materialize_slot_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        checkpoint()?;
        let output = self.__materialize_slot(session, slot)?;
        checkpoint()?;
        Ok(output)
    }
}

impl<S: Schema, B: Selectable<S>> selected_slot_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SelectedSlot<S> for B {
    type Output = B::Output;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        (*self).__selection_handle(session)
    }

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        (*self).__materialize_slot(session, slot)
    }

    fn __materialize_slot_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        (*self).__materialize_slot_with_checkpoint(session, slot, checkpoint)
    }
}

impl<S: Schema, B: Selectable<S>> selected_slot_sealed::Sealed<S> for Collected<S, B> {}
impl<S: Schema, B: Selectable<S>> SelectedSlot<S> for Collected<S, B> {
    type Output = Vec<B::Output>;

    fn __selection_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmSelectionHandle> {
        let binding = session.handle_by_key(self.selection.binding_key())?;
        let mut selection = binding
            .collect()
            .distinct(self.distinct)
            .map_err(Error::from_orm)?;
        for order in &self.order {
            selection = selection
                .order_by(session.lower_order(order)?)
                .map_err(Error::from_orm)?;
        }
        Ok(selection)
    }

    fn __materialize_slot(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
    ) -> Result<Self::Output> {
        let SlotValue::Many(things) = slot else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a singular slot for a collection selection",
                None,
            ));
        };
        let mut outputs = Vec::with_capacity(things.len());
        for thing in things {
            let row = session.client_row_for(thing)?;
            outputs.push(
                B::materialize_output(&row).map_err(|error| {
                    map_validation_error(error, ModelValidationPhase::Hydration)
                })?,
            );
        }
        Ok(outputs)
    }

    fn __materialize_slot_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        slot: &SlotValue,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        let SlotValue::Many(things) = slot else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a singular slot for a collection selection",
                None,
            ));
        };
        checkpoint()?;
        let mut outputs = Vec::with_capacity(things.len());
        for thing in things {
            checkpoint()?;
            let row = session.client_row_for(thing)?;
            outputs.push(
                B::materialize_output(&row).map_err(|error| {
                    map_validation_error(error, ModelValidationPhase::Hydration)
                })?,
            );
        }
        checkpoint()?;
        Ok(outputs)
    }
}

mod selected_shape_sealed {
    pub trait Sealed<S> {}
}

/// Sealed typed selected-output shape accepted by the one query facade.
///
/// Implementations are supplied for one binding, positional tuples through
/// the canonical sixteen-slot ceiling, and derive-backed named rows.
pub trait SelectedShape<S: Schema>: selected_shape_sealed::Sealed<S> + Clone {
    /// One fully materialized public row.
    type Output;

    #[doc(hidden)]
    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle>;

    #[doc(hidden)]
    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output>;

    #[doc(hidden)]
    fn __materialize_row_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        checkpoint()?;
        let output = self.__materialize_row(session, row)?;
        checkpoint()?;
        Ok(output)
    }
}

impl<S: Schema, B: Selectable<S>> selected_shape_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SelectedShape<S> for B {
    type Output = B::Output;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        session
            .session
            .positional([SelectedSlot::__selection_handle(self, session)?])
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        let [slot] = row.slots() else {
            return Err(selected_shape_arity_error(1, row.slots().len()));
        };
        SelectedSlot::__materialize_slot(self, session, slot)
    }
}

impl<S: Schema, B: Selectable<S>> selected_shape_sealed::Sealed<S> for Collected<S, B> {}
impl<S: Schema, B: Selectable<S>> SelectedShape<S> for Collected<S, B> {
    type Output = Vec<B::Output>;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        session
            .session
            .positional([SelectedSlot::__selection_handle(self, session)?])
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        let [slot] = row.slots() else {
            return Err(selected_shape_arity_error(1, row.slots().len()));
        };
        SelectedSlot::__materialize_slot(self, session, slot)
    }

    fn __materialize_row_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        let [slot] = row.slots() else {
            return Err(selected_shape_arity_error(1, row.slots().len()));
        };
        SelectedSlot::__materialize_slot_with_checkpoint(self, session, slot, checkpoint)
    }
}

mod singular_selected_shape_sealed {
    pub trait Sealed<S> {}
}

/// Sealed marker for a selected shape containing singular slots only.
pub trait SingularSelectedShape<S: Schema>:
    SelectedShape<S> + singular_selected_shape_sealed::Sealed<S>
{
}

impl<S: Schema, B: Selectable<S>> singular_selected_shape_sealed::Sealed<S> for B {}
impl<S: Schema, B: Selectable<S>> SingularSelectedShape<S> for B {}

#[doc(hidden)]
pub trait SelectedTuple<S: Schema>: Clone {
    const ARITY: usize;

    type Output;

    fn __selection_handles(&self, session: &QuerySession<'_, S>)
    -> Result<Vec<OrmSelectionHandle>>;

    fn __materialize_slots(
        &self,
        session: &QuerySession<'_, S>,
        slots: &[SlotValue],
    ) -> Result<Self::Output>;

    #[doc(hidden)]
    fn __materialize_slots_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        slots: &[SlotValue],
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        checkpoint()?;
        let output = self.__materialize_slots(session, slots)?;
        checkpoint()?;
        Ok(output)
    }
}

#[doc(hidden)]
pub trait SingularSelectedTuple<S: Schema>: SelectedTuple<S> {}

macro_rules! selected_tuple {
    ($length:literal; $(($type:ident, $index:tt)),+ $(,)?) => {
        impl<S: Schema, $($type: SelectedSlot<S>),+> SelectedTuple<S> for ($($type,)+) {
            const ARITY: usize = $length;

            type Output = ($(<$type as SelectedSlot<S>>::Output,)+);

            fn __selection_handles(
                &self,
                session: &QuerySession<'_, S>,
            ) -> Result<Vec<OrmSelectionHandle>> {
                Ok(vec![$(SelectedSlot::__selection_handle(&self.$index, session)?),+])
            }

            fn __materialize_slots(
                &self,
                session: &QuerySession<'_, S>,
                slots: &[SlotValue],
            ) -> Result<Self::Output> {
                let slots: &[SlotValue; $length] = slots
                    .try_into()
                    .map_err(|_| selected_shape_arity_error($length, slots.len()))?;
                Ok(($(SelectedSlot::__materialize_slot(
                    &self.$index,
                    session,
                    &slots[$index],
                )?,)+))
            }

            fn __materialize_slots_with_checkpoint(
                &self,
                session: &QuerySession<'_, S>,
                slots: &[SlotValue],
                checkpoint: &dyn Fn() -> Result<()>,
            ) -> Result<Self::Output> {
                let slots: &[SlotValue; $length] = slots
                    .try_into()
                    .map_err(|_| selected_shape_arity_error($length, slots.len()))?;
                Ok(($(SelectedSlot::__materialize_slot_with_checkpoint(
                    &self.$index,
                    session,
                    &slots[$index],
                    checkpoint,
                )?,)+))
            }
        }

        impl<S: Schema, $($type: SelectedSlot<S>),+> selected_shape_sealed::Sealed<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: SelectedSlot<S>),+> SelectedShape<S> for ($($type,)+) {
            type Output = <Self as SelectedTuple<S>>::Output;

            fn __shape_handle(
                &self,
                session: &QuerySession<'_, S>,
            ) -> Result<OrmShapeHandle> {
                session
                    .session
                    .positional(self.__selection_handles(session)?)
                    .map_err(Error::from_orm)
            }

            fn __materialize_row(
                &self,
                session: &QuerySession<'_, S>,
                row: &MatchRow,
            ) -> Result<Self::Output> {
                self.__materialize_slots(session, row.slots())
            }


            fn __materialize_row_with_checkpoint(
                &self,
                session: &QuerySession<'_, S>,
                row: &MatchRow,
                checkpoint: &dyn Fn() -> Result<()>,
            ) -> Result<Self::Output> {
                self.__materialize_slots_with_checkpoint(session, row.slots(), checkpoint)
            }
        }

        impl<S: Schema, $($type: Selectable<S>),+> singular_selected_shape_sealed::Sealed<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: Selectable<S>),+> SingularSelectedShape<S>
            for ($($type,)+)
        {
        }

        impl<S: Schema, $($type: Selectable<S>),+> SingularSelectedTuple<S>
            for ($($type,)+)
        {
        }
    };
}

selected_tuple!(1; (A, 0));
selected_tuple!(2; (A, 0), (B, 1));
selected_tuple!(3; (A, 0), (B, 1), (C, 2));
selected_tuple!(4; (A, 0), (B, 1), (C, 2), (D, 3));
selected_tuple!(5; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
selected_tuple!(6; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
selected_tuple!(7; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
selected_tuple!(8; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
selected_tuple!(9; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8));
selected_tuple!(10; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9));
selected_tuple!(11; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10));
selected_tuple!(12; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11));
selected_tuple!(13; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12));
selected_tuple!(14; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13));
selected_tuple!(15; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13), (O, 14));
selected_tuple!(16; (A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11), (M, 12), (N, 13), (O, 14), (P, 15));

/// Construction contract generated by `#[derive(type_bridge::SelectedRow)]`.
#[doc(hidden)]
pub trait SelectedRowSpec<Outputs>: Sized {
    fn __from_selected_outputs(outputs: Outputs) -> Self;
}

/// A declaration-ordered named selected shape produced by `SelectedRow`.
pub struct NamedSelection<S: Schema, Row, Slots> {
    slots: Slots,
    names: &'static [&'static str],
    marker: PhantomData<fn() -> (S, Row)>,
}

impl<S: Schema, Row, Slots: Clone> Clone for NamedSelection<S, Row, Slots> {
    fn clone(&self) -> Self {
        Self {
            slots: self.slots.clone(),
            names: self.names,
            marker: PhantomData,
        }
    }
}

impl<S: Schema, Row, Slots: SelectedTuple<S>> NamedSelection<S, Row, Slots> {
    #[doc(hidden)]
    pub fn __new(slots: Slots, names: &'static [&'static str]) -> Result<Self> {
        if names.len() != Slots::ARITY {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_selected_shape",
                vec![],
                format!(
                    "named selected shape has {} names for {} slots",
                    names.len(),
                    Slots::ARITY
                ),
                None,
            ));
        }
        Ok(Self {
            slots,
            names,
            marker: PhantomData,
        })
    }
}

impl<S: Schema, Row, Slots> selected_shape_sealed::Sealed<S> for NamedSelection<S, Row, Slots> {}

impl<S, Row, Slots> SelectedShape<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
    type Output = Row;

    fn __shape_handle(&self, session: &QuerySession<'_, S>) -> Result<OrmShapeHandle> {
        let selections = self.slots.__selection_handles(session)?;
        session
            .session
            .named(
                self.names
                    .iter()
                    .copied()
                    .zip(selections)
                    .map(|(name, selection)| (name.to_owned(), selection)),
            )
            .map_err(Error::from_orm)
    }

    fn __materialize_row(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
    ) -> Result<Self::Output> {
        Ok(Row::__from_selected_outputs(
            self.slots.__materialize_slots(session, row.slots())?,
        ))
    }

    fn __materialize_row_with_checkpoint(
        &self,
        session: &QuerySession<'_, S>,
        row: &MatchRow,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Self::Output> {
        Ok(Row::__from_selected_outputs(
            self.slots
                .__materialize_slots_with_checkpoint(session, row.slots(), checkpoint)?,
        ))
    }
}

impl<S, Row, Slots> singular_selected_shape_sealed::Sealed<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SingularSelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
}

impl<S, Row, Slots> SingularSelectedShape<S> for NamedSelection<S, Row, Slots>
where
    S: Schema,
    Slots: SingularSelectedTuple<S>,
    Row: SelectedRowSpec<Slots::Output>,
{
}

fn selected_shape_arity_error(expected: usize, actual: usize) -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "wrong_result_shape",
        vec![],
        format!("selected row has {actual} slots; expected {expected}"),
        None,
    )
}

/// Bounded options for one ordered row fetch.
#[derive(Debug)]
pub struct RowsOptions<S: Schema> {
    limit: u64,
    offset: u64,
    order: Vec<Order<S>>,
}

impl<S: Schema> Clone for RowsOptions<S> {
    fn clone(&self) -> Self {
        Self {
            limit: self.limit,
            offset: self.offset,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema> RowsOptions<S> {
    /// Create resource-bounded row options with a nonzero limit.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            offset: 0,
            order: Vec::new(),
        }
    }

    /// Skip the first `offset` distinct rows.
    #[must_use]
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Append one stable public ordering term.
    #[must_use]
    pub fn order_by(mut self, order: Order<S>) -> Self {
        self.order.push(order);
        self
    }
}

/// Bounded options for one ordered distinct-root page.
#[derive(Debug)]
pub struct PageOptions<S: Schema> {
    limit: u64,
    offset: u64,
    include_total: bool,
    order: Vec<Order<S>>,
}

impl<S: Schema> Clone for PageOptions<S> {
    fn clone(&self) -> Self {
        Self {
            limit: self.limit,
            offset: self.offset,
            include_total: self.include_total,
            order: self.order.clone(),
        }
    }
}

impl<S: Schema> PageOptions<S> {
    /// Create resource-bounded page options with a nonzero terminal limit.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            offset: 0,
            include_total: false,
            order: Vec::new(),
        }
    }

    /// Skip the first `offset` distinct roots.
    #[must_use]
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Request a same-snapshot total distinct-root count.
    #[must_use]
    pub fn include_total(mut self, include_total: bool) -> Self {
        self.include_total = include_total;
        self
    }

    /// Append one stable root-ordering term.
    #[must_use]
    pub fn order_by(mut self, order: Order<S>) -> Self {
        self.order.push(order);
        self
    }
}

/// One immutable owned distinct-root page.
#[derive(Clone, Debug)]
pub struct Page<T> {
    items: Vec<T>,
    offset: u64,
    limit: u64,
    total: Option<u64>,
}

impl<T> Page<T> {
    /// Borrow page items in stable root order.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Return the requested root offset.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Return the requested root limit.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Return the same-snapshot total when it was requested.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    /// Consume the page and return its owned items.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

/// One persistent, reusable singular-shape query lineage.
///
/// Each authoring method returns a new lineage and leaves its ancestor
/// usable.
pub struct Query<'s, 'db, S: Schema, Shape: SelectedShape<S>> {
    session: &'s QuerySession<'db, S>,
    selection: Shape,
    hidden: Vec<BindingKey>,
    predicates: Vec<Predicate<S>>,
    allowed_cross_joins: Vec<(BindingKey, BindingKey)>,
    closed: AtomicBool,
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Clone for Query<'s, 'db, S, Shape> {
    fn clone(&self) -> Self {
        Self {
            session: self.session,
            selection: self.selection.clone(),
            hidden: self.hidden.clone(),
            predicates: self.predicates.clone(),
            allowed_cross_joins: self.allowed_cross_joins.clone(),
            closed: AtomicBool::new(self.closed.load(Ordering::Acquire)),
        }
    }
}

impl<'db, S: Schema> QuerySession<'db, S> {
    /// Begin one persistent query lineage from a singular selected shape.
    pub fn query<Shape: SelectedShape<S>>(
        &self,
        selection: Shape,
    ) -> Result<Query<'_, 'db, S, Shape>> {
        selection.__shape_handle(self)?;
        Ok(Query {
            session: self,
            selection,
            hidden: Vec::new(),
            predicates: Vec::new(),
            allowed_cross_joins: Vec::new(),
            closed: AtomicBool::new(false),
        })
    }
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Query<'s, 'db, S, Shape> {
    /// Explicitly close this immutable query handle.
    ///
    /// Closing is idempotent and affects only this handle. Clones, ancestors,
    /// descendants, siblings, and the authoring session have independent
    /// query lifecycles.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Return whether this query handle was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn ensure_open(&self, phase: ModelValidationPhase) -> Result<()> {
        self.session.ensure_open(phase)?;
        if self.closed.load(Ordering::Acquire) {
            Err(Error::from_sdk_execution(
                type_bridge_orm::query_resource_closed_diagnostic(),
                phase,
            ))
        } else {
            Ok(())
        }
    }

    /// Attach one generated binding for predicates without selecting it.
    pub fn match_<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &self,
        binding: Binding<S, M, Mode>,
    ) -> Result<Self> {
        self.ensure_open(ModelValidationPhase::Input)?;
        self.session.handle_by_key(binding.key())?;
        let mut next = self.clone();
        if !next.hidden.contains(&binding.key()) {
            next.hidden.push(binding.key());
        }
        Ok(next)
    }

    /// Attach one predicate; repeated calls form a conjunction in call order.
    pub fn where_(&self, predicate: Predicate<S>) -> Result<Self> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let mut next = self.clone();
        next.predicates.push(predicate);
        Ok(next)
    }

    /// Attach predicates as one implicit conjunction in source order.
    pub fn where_all(&self, predicates: impl IntoIterator<Item = Predicate<S>>) -> Result<Self> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let mut next = self.clone();
        next.predicates.extend(predicates);
        Ok(next)
    }

    /// Explicitly permit one topology-level cross join between two attached
    /// generated bindings. The returned lineage is immutable and reusable.
    pub fn allow_cross_join<L: Selectable<S>, R: Selectable<S>>(
        &self,
        left: L,
        right: R,
    ) -> Result<Self> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let left = left.binding_key();
        let right = right.binding_key();
        self.session.handle_by_key(left)?;
        self.session.handle_by_key(right)?;
        if left == right {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "self_cross_join",
                vec![],
                "cross-join permission requires two distinct generated bindings",
                None,
            ));
        }
        let pair = if left.index < right.index {
            (left, right)
        } else {
            (right, left)
        };
        let mut next = self.clone();
        if !next.allowed_cross_joins.contains(&pair) {
            next.allowed_cross_joins.push(pair);
        }
        Ok(next)
    }

    fn lineage(&self) -> Result<OrmQueryHandle> {
        self.lineage_with_hidden(&[])
    }

    fn lineage_with_hidden(&self, hidden: &[BindingKey]) -> Result<OrmQueryHandle> {
        self.ensure_open(ModelValidationPhase::Input)?;
        let shape = self.selection.__shape_handle(self.session)?;
        let mut query = self.session.session.query(shape).map_err(Error::from_orm)?;
        let mut hidden_keys = self.hidden.clone();
        for key in hidden {
            if !hidden_keys.contains(key) {
                hidden_keys.push(*key);
            }
        }
        for key in hidden_keys {
            let hidden = self.session.handle_by_key(key)?;
            query = query.add_hidden(hidden.clone()).map_err(Error::from_orm)?;
        }
        for (left, right) in &self.allowed_cross_joins {
            let left = self.session.handle_by_key(*left)?;
            let right = self.session.handle_by_key(*right)?;
            query = query
                .allow_cross_join(left, right)
                .map_err(Error::from_orm)?;
        }
        for predicate in &self.predicates {
            let lowered = self.session.lower_predicate(&predicate.expr)?;
            query = query.where_predicate(lowered).map_err(Error::from_orm)?;
        }
        Ok(query)
    }

    pub(crate) fn validated_rows(
        &self,
        order: &[Order<S>],
        window: Window,
    ) -> Result<ValidatedMatchRequest> {
        let lineage = self.lineage()?;
        let mut lowered_orders = Vec::with_capacity(order.len());
        for term in order {
            lowered_orders.push(self.session.lower_order(term)?);
        }
        lineage
            .validate_fetch_rows(&lowered_orders, window, RowCardinality::BoundedMany)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_one(&self) -> Result<ValidatedMatchRequest> {
        self.lineage()?
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::ExactlyOne,
            )
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_page<R: Selectable<S>>(
        &self,
        root: R,
        order: &[Order<S>],
        window: Window,
        include_total: bool,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        let lineage = self.lineage()?;
        let mut lowered_orders = Vec::with_capacity(order.len());
        for term in order {
            lowered_orders.push(self.session.lower_order(term)?);
        }
        lineage
            .validate_page_by(root, &lowered_orders, window, include_total)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_count_by<R: Selectable<S>>(
        &self,
        root: R,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        self.lineage()?
            .validate_count_by(root)
            .map_err(Error::from_orm)
    }

    pub(crate) fn validated_exists_by<R: Selectable<S>>(
        &self,
        root: R,
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(root.binding_key())?;
        self.lineage()?
            .validate_exists_by(root)
            .map_err(Error::from_orm)
    }

    fn materialize_rows(
        &self,
        rows: &[MatchRow],
        deadline: QueryExecutionDeadline,
    ) -> Result<Vec<Shape::Output>> {
        let checkpoint = || {
            self.session
                .check_invocation(deadline, ModelValidationPhase::Hydration)
        };
        checkpoint()?;
        let mut outputs = Vec::with_capacity(rows.len());
        for row in rows {
            checkpoint()?;
            outputs.push(self.selection.__materialize_row_with_checkpoint(
                self.session,
                row,
                &checkpoint,
            )?);
        }
        checkpoint()?;
        Ok(outputs)
    }

    pub(crate) fn outputs_from_rows(
        &self,
        validated: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
        deadline: QueryExecutionDeadline,
    ) -> Result<Vec<Shape::Output>> {
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        let rows = match result
            .for_request(validated)
            .map_err(|error| Error::from_orm_hydration(error.into()))?
        {
            MatchResult::Rows { rows } => rows,
            _ => {
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "provider returned a non-row result for a row fetch",
                    None,
                ));
            }
        };
        self.materialize_rows(rows, deadline)
    }

    pub(crate) fn output_page(
        &self,
        validated: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
        deadline: QueryExecutionDeadline,
    ) -> Result<Page<Shape::Output>> {
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        let (entries, window, total) = match result
            .for_request(validated)
            .map_err(|error| Error::from_orm_hydration(error.into()))?
        {
            MatchResult::Page {
                entries,
                window,
                total,
                ..
            } => (entries, *window, *total),
            _ => {
                return Err(Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "provider returned a non-page result for a page fetch",
                    None,
                ));
            }
        };
        Ok(Page {
            items: self.materialize_rows(entries, deadline)?,
            offset: window.offset,
            limit: window.limit,
            total,
        })
    }

    async fn execute(
        &self,
        validated: ValidatedMatchRequest,
        deadline: QueryExecutionDeadline,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult)> {
        self.ensure_open(ModelValidationPhase::Input)?;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Input)?;
        let result = match &self.session.execution {
            QueryExecution::Borrowed(transaction) => {
                transaction
                    .execute_match_with_limits(
                        &self.session.registry,
                        &validated,
                        self.session
                            .resources
                            .direct_with_deadline(self.session.cancellation.clone(), deadline),
                    )
                    .await
            }
            QueryExecution::Local(database) => {
                database
                    .inner_orm()
                    .execute_match_with_limits(
                        &self.session.registry,
                        &validated,
                        self.session
                            .resources
                            .direct_with_deadline(self.session.cancellation.clone(), deadline),
                    )
                    .await
            }
            QueryExecution::Remote(remote) => {
                let result = remote
                    .execute_match(
                        &self.session.registry,
                        validated,
                        self.session.resources,
                        self.session.cancellation.clone(),
                        deadline,
                    )
                    .await?;
                self.ensure_open(ModelValidationPhase::Hydration)?;
                return Ok(result);
            }
        };
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        self.ensure_open(ModelValidationPhase::Hydration)?;
        Ok((validated, result.map_err(Error::from_orm_hydration)?))
    }
}

impl<'s, 'db, S, Shape> Query<'s, 'db, S, Shape>
where
    S: Schema,
    Shape: SingularSelectedShape<S>,
{
    /// Return exactly one distinct selected identity, failing `no_result` on
    /// an empty stream and `not_unique` on more than one.
    pub async fn one(&self) -> Result<Shape::Output> {
        let deadline = self.session.begin_invocation()?;
        let validated = self.validated_one()?;
        let (validated, result) = self.execute(validated, deadline).await?;
        let mut outputs = self.outputs_from_rows(&validated, &result, deadline)?;
        match outputs.len() {
            0 => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "no_result",
                vec![],
                "query selected no distinct identity",
                None,
            )),
            1 => Ok(outputs.remove(0)),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "not_unique",
                vec![],
                "query selected more than one distinct identity",
                None,
            )),
        }
    }

    /// Return a resource-bounded ordered sequence of distinct selected
    /// identities; the limit must be nonzero.
    pub async fn rows(&self, options: RowsOptions<S>) -> Result<Vec<Shape::Output>> {
        let deadline = self.session.begin_invocation()?;
        if options.limit == 0 {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "zero_limit",
                vec![],
                "row fetches require a nonzero limit",
                None,
            ));
        }
        let validated = self.validated_rows(
            &options.order,
            Window {
                offset: options.offset,
                limit: options.limit,
            },
        )?;
        let (validated, result) = self.execute(validated, deadline).await?;
        self.outputs_from_rows(&validated, &result, deadline)
    }

    /// Return the first distinct selected identity under a stable order.
    pub async fn first(&self, order: Order<S>) -> Result<Option<Shape::Output>> {
        let deadline = self.session.begin_invocation()?;
        let validated = self.validated_rows(
            &[order],
            Window {
                offset: 0,
                limit: 1,
            },
        )?;
        let (validated, result) = self.execute(validated, deadline).await?;
        Ok(self.outputs_from_rows(&validated, &result, deadline)?.pop())
    }
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>> Query<'s, 'db, S, Shape> {
    /// Return one resource-bounded page grouped by distinct root identity.
    pub async fn page_by<R: Selectable<S>>(
        &self,
        root: R,
        options: PageOptions<S>,
    ) -> Result<Page<Shape::Output>> {
        let deadline = self.session.begin_invocation()?;
        if options.limit == 0 {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "zero_limit",
                vec![],
                "page fetches require a nonzero limit",
                None,
            ));
        }
        let validated = self.validated_page(
            root,
            &options.order,
            Window {
                offset: options.offset,
                limit: options.limit,
            },
            options.include_total,
        )?;
        let (validated, result) = self.execute(validated, deadline).await?;
        self.output_page(&validated, &result, deadline)
    }

    /// Count distinct identities of one selected root binding.
    pub async fn count_by<R: Selectable<S>>(&self, root: R) -> Result<u64> {
        let deadline = self.session.begin_invocation()?;
        let validated = self.validated_count_by(root)?;
        let (validated, result) = self.execute(validated, deadline).await?;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        match result
            .for_request(&validated)
            .map_err(|error| Error::from_orm_hydration(error.into()))?
        {
            MatchResult::Count { value, .. } => Ok(*value),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-count result for a count",
                None,
            )),
        }
    }

    /// Test whether any distinct identity of one selected root binding
    /// exists.
    pub async fn exists_by<R: Selectable<S>>(&self, root: R) -> Result<bool> {
        let deadline = self.session.begin_invocation()?;
        let validated = self.validated_exists_by(root)?;
        let (validated, result) = self.execute(validated, deadline).await?;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        match result
            .for_request(&validated)
            .map_err(|error| Error::from_orm_hydration(error.into()))?
        {
            MatchResult::Exists { value, .. } => Ok(*value),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-existence result for an existence test",
                None,
            )),
        }
    }
}

impl<'s, 'db, S: Schema, B: Selectable<S>> Query<'s, 'db, S, B> {
    /// Count distinct selected identities.
    pub async fn count(&self) -> Result<u64> {
        self.count_by(self.selection).await
    }

    /// Test whether any distinct selected identity exists.
    pub async fn exists(&self) -> Result<bool> {
        self.exists_by(self.selection).await
    }

    fn lowered_reduce_terms(
        &self,
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<Vec<OrmFieldHandle>> {
        let mut lowered = Vec::new();
        for (_, input) in terms {
            if let Some((key, owns_id_json)) = input {
                lowered.push(self.session.lower_field(*key, owns_id_json)?);
            }
        }
        Ok(lowered)
    }

    pub(crate) fn validated_reduce(
        &self,
        group: Option<BindingKey>,
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(self.selection.binding_key())?;
        let hidden_groups = group
            .filter(|key| *key != self.selection.binding_key())
            .into_iter()
            .collect::<Vec<_>>();
        let lineage = self.lineage_with_hidden(&hidden_groups)?;
        let group_handle = group
            .map(|key| self.session.handle_by_key(key))
            .transpose()?;
        let lowered_inputs = self.lowered_reduce_terms(terms)?;
        let mut inputs = lowered_inputs.iter();
        let mut pairs = Vec::with_capacity(terms.len());
        for (reduction, input) in terms {
            let handle = if input.is_some() {
                Some(inputs.next().expect("one lowered handle per input"))
            } else {
                None
            };
            pairs.push((*reduction, handle));
        }
        lineage
            .validate_reduce_by(root, group_handle, &pairs)
            .map_err(Error::from_orm)
    }

    fn validated_reduce_by_field<Owner: Model<Schema = S>, V>(
        &self,
        group: BoundField<S, Owner, V>,
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(self.selection.binding_key())?;
        let hidden_groups = (group.key != self.selection.binding_key())
            .then_some(group.key)
            .into_iter()
            .collect::<Vec<_>>();
        let lineage = self.lineage_with_hidden(&hidden_groups)?;
        let group = self.session.lower_field(group.key, group.owns_id_json)?;
        let lowered_inputs = self.lowered_reduce_terms(terms)?;
        let mut inputs = lowered_inputs.iter();
        let mut pairs = Vec::with_capacity(terms.len());
        for (reduction, input) in terms {
            let handle = if input.is_some() {
                Some(inputs.next().expect("one lowered handle per input"))
            } else {
                None
            };
            pairs.push((*reduction, handle));
        }
        lineage
            .validate_reduce_by_field(root, &group, &pairs)
            .map_err(Error::from_orm)
    }

    fn validated_reduce_by_fields(
        &self,
        groups: &[(BindingKey, &'static str)],
        terms: &[(
            type_bridge_orm::match_request::Reduction,
            Option<(BindingKey, &'static str)>,
        )],
    ) -> Result<ValidatedMatchRequest> {
        let root = self.session.handle_by_key(self.selection.binding_key())?;
        let mut hidden_keys = Vec::new();
        for (key, _) in groups {
            if *key != self.selection.binding_key() && !hidden_keys.contains(key) {
                hidden_keys.push(*key);
            }
        }
        let lineage = self.lineage_with_hidden(&hidden_keys)?;
        let lowered_groups = groups
            .iter()
            .map(|(key, owns_id_json)| self.session.lower_field(*key, owns_id_json))
            .collect::<Result<Vec<_>>>()?;
        let group_refs = lowered_groups.iter().collect::<Vec<_>>();
        let lowered_inputs = self.lowered_reduce_terms(terms)?;
        let mut inputs = lowered_inputs.iter();
        let mut pairs = Vec::with_capacity(terms.len());
        for (reduction, input) in terms {
            let handle = if input.is_some() {
                Some(inputs.next().expect("one lowered handle per input"))
            } else {
                None
            };
            pairs.push((*reduction, handle));
        }
        lineage
            .validate_reduce_by_fields(root, &group_refs, &pairs)
            .map_err(Error::from_orm)
    }

    fn decoded_reduction_rows<'result>(
        &self,
        validated: &ValidatedMatchRequest,
        result: &'result ValidatedMatchResult,
        deadline: QueryExecutionDeadline,
    ) -> Result<&'result [type_bridge_orm::match_request::ReductionRow]> {
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        match result
            .for_request(validated)
            .map_err(|error| Error::from_orm_hydration(error.into()))?
        {
            MatchResult::Reduction { rows, .. }
            | MatchResult::FieldReduction { rows, .. }
            | MatchResult::FieldTupleReduction { rows, .. } => Ok(rows),
            _ => Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "provider returned a non-reduction result for an aggregate",
                None,
            )),
        }
    }

    /// Reduce the distinct selected stream to one typed tuple of aggregate
    /// values.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<T::Output> {
        let deadline = self.session.begin_invocation()?;
        let term_list = terms.terms();
        let validated = self.validated_reduce(None, &term_list)?;
        let (validated, result) = self.execute(validated, deadline).await?;
        let rows = self.decoded_reduction_rows(&validated, &result, deadline)?;
        let [row] = rows else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "wrong_result_shape",
                vec![],
                "ungrouped aggregates require exactly one reduction row",
                None,
            ));
        };
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        let output = T::decode(row.values())?;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        Ok(output)
    }

    /// Group the distinct selected stream by another attached binding's
    /// distinct identities before aggregating.
    pub fn group_by<G: Selectable<S>>(&self, group: G) -> Result<GroupedQuery<'s, 'db, S, B, G>> {
        self.ensure_open(ModelValidationPhase::Input)?;
        self.session.handle_by_key(group.binding_key())?;
        Ok(GroupedQuery {
            query: self.clone(),
            group,
        })
    }

    /// Group the distinct selected stream by each witnessed value of one
    /// generated owned field before aggregating.
    pub fn group_by_field<Owner, V>(
        &self,
        group: BoundField<S, Owner, V>,
    ) -> Result<FieldGroupedQuery<'s, 'db, S, B, Owner, V>>
    where
        Owner: Model<Schema = S>,
        V: GroupedQueryValue,
    {
        self.ensure_open(ModelValidationPhase::Input)?;
        self.session.lower_field(group.key, group.owns_id_json)?;
        Ok(FieldGroupedQuery {
            query: self.clone(),
            group,
        })
    }

    /// Group the distinct selected stream by the Cartesian tuple of multiple
    /// generated owned fields' witnessed values before aggregating.
    pub fn group_by_fields<G>(&self, groups: G) -> Result<FieldTupleGroupedQuery<'s, 'db, S, B, G>>
    where
        G: FieldGroupTuple<S>,
    {
        self.ensure_open(ModelValidationPhase::Input)?;
        for (key, owns_id_json) in groups.fields() {
            self.session.lower_field(key, owns_id_json)?;
        }
        Ok(FieldTupleGroupedQuery {
            query: self.clone(),
            groups,
        })
    }
}

/// One query lineage grouped by a second attached binding for aggregation.
pub struct GroupedQuery<'s, 'db, S: Schema, B: Selectable<S>, G: Selectable<S>> {
    query: Query<'s, 'db, S, B>,
    group: G,
}

impl<'s, 'db, S: Schema, B: Selectable<S>, G: Selectable<S>> GroupedQuery<'s, 'db, S, B, G> {
    /// Explicitly close this independently owned grouped-query lineage.
    pub fn close(&self) {
        self.query.close();
    }

    /// Return whether this grouped query was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.query.is_closed()
    }

    /// Reduce each witnessed distinct group identity to one typed tuple,
    /// returning materialized group keys with their aggregate values.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<Vec<(G::Output, T::Output)>> {
        let deadline = self.query.session.begin_invocation()?;
        let term_list = terms.terms();
        let validated = self
            .query
            .validated_reduce(Some(self.group.binding_key()), &term_list)?;
        let (validated, result) = self.query.execute(validated, deadline).await?;
        let rows = self
            .query
            .decoded_reduction_rows(&validated, &result, deadline)?;
        let mut outputs = Vec::with_capacity(rows.len());
        for row in rows {
            self.query
                .session
                .check_invocation(deadline, ModelValidationPhase::Hydration)?;
            let thing = row.group().ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "grouped aggregates require group evidence per row",
                    None,
                )
            })?;
            let client_row = self.query.session.client_row_for(thing)?;
            let key = G::materialize_output(&client_row).map_err(|error| {
                crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
            })?;
            outputs.push((key, T::decode(row.values())?));
        }
        self.query
            .session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        Ok(outputs)
    }
}

/// One query lineage grouped by a generated owned field value for
/// aggregation.
pub struct FieldGroupedQuery<
    's,
    'db,
    S: Schema,
    B: Selectable<S>,
    Owner: Model<Schema = S>,
    V: GroupedQueryValue,
> {
    query: Query<'s, 'db, S, B>,
    group: BoundField<S, Owner, V>,
}

impl<'s, 'db, S, B, Owner, V> FieldGroupedQuery<'s, 'db, S, B, Owner, V>
where
    S: Schema,
    B: Selectable<S>,
    Owner: Model<Schema = S>,
    V: GroupedQueryValue,
{
    /// Explicitly close this independently owned grouped-query lineage.
    pub fn close(&self) {
        self.query.close();
    }

    /// Return whether this grouped query was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.query.is_closed()
    }

    /// Reduce each witnessed distinct field value to one typed tuple,
    /// returning its exact generated attribute wrapper with the aggregates.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<Vec<(V, T::Output)>> {
        let deadline = self.query.session.begin_invocation()?;
        let term_list = terms.terms();
        let validated = self
            .query
            .validated_reduce_by_field(self.group, &term_list)?;
        let (validated, result) = self.query.execute(validated, deadline).await?;
        let rows = self
            .query
            .decoded_reduction_rows(&validated, &result, deadline)?;
        let mut outputs = Vec::with_capacity(rows.len());
        for row in rows {
            self.query
                .session
                .check_invocation(deadline, ModelValidationPhase::Hydration)?;
            let value = row.field_group().ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "field-grouped aggregates require scalar group evidence per row",
                    None,
                )
            })?;
            let key = V::from_group_scalar(encoded_group_scalar(value)?)
                .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))?;
            outputs.push((key, T::decode(row.values())?));
        }
        self.query
            .session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        Ok(outputs)
    }
}

/// A sealed tuple of two through sixteen generated owned fields used as one
/// typed grouped-reduction key.
pub trait FieldGroupTuple<S: Schema>: field_group_tuple_sealed::Sealed<S> + Copy {
    /// Exact generated attribute-wrapper tuple returned for each group row.
    type Output;

    #[doc(hidden)]
    fn fields(&self) -> Vec<(BindingKey, &'static str)>;

    #[doc(hidden)]
    fn decode(values: &[AttributeValue]) -> Result<Self::Output>;
}

mod field_group_tuple_sealed {
    pub trait Sealed<S> {}
}

macro_rules! field_group_tuple {
    ($(($owner:ident, $value:ident, $index:tt)),+) => {
        impl<S, $($owner, $value),+> field_group_tuple_sealed::Sealed<S>
            for ($(BoundField<S, $owner, $value>,)+)
        where
            S: Schema,
            $($owner: Model<Schema = S>, $value: GroupedQueryValue),+
        {
        }

        impl<S, $($owner, $value),+> FieldGroupTuple<S>
            for ($(BoundField<S, $owner, $value>,)+)
        where
            S: Schema,
            $($owner: Model<Schema = S>, $value: GroupedQueryValue),+
        {
            type Output = ($($value,)+);

            fn fields(&self) -> Vec<(BindingKey, &'static str)> {
                vec![$((self.$index.key, self.$index.owns_id_json)),+]
            }

            fn decode(values: &[AttributeValue]) -> Result<Self::Output> {
                let expected = [$(stringify!($owner)),+].len();
                if values.len() != expected {
                    return Err(Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "wrong_result_shape",
                        vec![],
                        "tuple-field-grouped aggregate key has the wrong arity",
                        None,
                    ));
                }
                Ok(($(
                    $value::from_group_scalar(encoded_group_scalar(&values[$index])?)
                        .map_err(|error| {
                            map_validation_error(error, ModelValidationPhase::Hydration)
                        })?,
                )+))
            }
        }
    };
}

field_group_tuple!((O1, V1, 0), (O2, V2, 1));
field_group_tuple!((O1, V1, 0), (O2, V2, 1), (O3, V3, 2));
field_group_tuple!((O1, V1, 0), (O2, V2, 1), (O3, V3, 2), (O4, V4, 3));
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10),
    (O12, V12, 11)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10),
    (O12, V12, 11),
    (O13, V13, 12)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10),
    (O12, V12, 11),
    (O13, V13, 12),
    (O14, V14, 13)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10),
    (O12, V12, 11),
    (O13, V13, 12),
    (O14, V14, 13),
    (O15, V15, 14)
);
field_group_tuple!(
    (O1, V1, 0),
    (O2, V2, 1),
    (O3, V3, 2),
    (O4, V4, 3),
    (O5, V5, 4),
    (O6, V6, 5),
    (O7, V7, 6),
    (O8, V8, 7),
    (O9, V9, 8),
    (O10, V10, 9),
    (O11, V11, 10),
    (O12, V12, 11),
    (O13, V13, 12),
    (O14, V14, 13),
    (O15, V15, 14),
    (O16, V16, 15)
);

/// One query lineage grouped by a generated owned-field tuple for
/// aggregation.
pub struct FieldTupleGroupedQuery<'s, 'db, S: Schema, B: Selectable<S>, G: FieldGroupTuple<S>> {
    query: Query<'s, 'db, S, B>,
    groups: G,
}

impl<'s, 'db, S, B, G> FieldTupleGroupedQuery<'s, 'db, S, B, G>
where
    S: Schema,
    B: Selectable<S>,
    G: FieldGroupTuple<S>,
{
    /// Explicitly close this independently owned grouped-query lineage.
    pub fn close(&self) {
        self.query.close();
    }

    /// Return whether this grouped query was explicitly closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.query.is_closed()
    }

    /// Reduce each witnessed distinct field-value tuple to one typed tuple,
    /// returning exact generated attribute wrappers with the aggregates.
    pub async fn aggregate<T: crate::aggregate::AggregateTuple<S>>(
        &self,
        terms: T,
    ) -> Result<Vec<(G::Output, T::Output)>> {
        let deadline = self.query.session.begin_invocation()?;
        let term_list = terms.terms();
        let group_fields = self.groups.fields();
        let validated = self
            .query
            .validated_reduce_by_fields(&group_fields, &term_list)?;
        let (validated, result) = self.query.execute(validated, deadline).await?;
        let rows = self
            .query
            .decoded_reduction_rows(&validated, &result, deadline)?;
        let mut outputs = Vec::with_capacity(rows.len());
        for row in rows {
            self.query
                .session
                .check_invocation(deadline, ModelValidationPhase::Hydration)?;
            let values = row.field_groups().ok_or_else(|| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "wrong_result_shape",
                    vec![],
                    "tuple-field-grouped aggregates require tuple group evidence per row",
                    None,
                )
            })?;
            outputs.push((G::decode(values)?, T::decode(row.values())?));
        }
        self.query
            .session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        Ok(outputs)
    }
}
