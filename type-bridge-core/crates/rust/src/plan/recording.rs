//! Typed handles over the shared plan recorder.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::QueryOperand;
use type_bridge_contract::query_plan::{
    ArithOp, Expr as CoreExpr, PlanBuilder as CoreBuilder, Site,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, EncodedScalar, FieldToken,
    IntoEncodedScalar, Model, NominalUpcast, QueryValued, RelationModel, RolePlayerBinding,
    RoleToken, RoleTokenCompatible, ThingModel, ValidationPath,
};
use crate::Result;
use crate::error::{Error, ModelValidationPhase};
use crate::query::{Exact, SelectionMode, Subtypes};
use crate::schema::Schema;

static NEXT_RECORDING: AtomicU64 = AtomicU64::new(1);

fn context_error() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "plan_context_missing",
        vec![],
        "the handle belongs to another or closed recording",
        None,
    )
}

#[doc(hidden)]
pub enum ScalarInput {
    Encoded(EncodedScalar),
    Double(f64),
}

impl ScalarInput {
    fn canonical(self) -> Result<CanonicalValue> {
        match self {
            Self::Encoded(EncodedScalar::String(value)) => {
                type_bridge_contract::value::CanonicalString::new(value)
                    .map(CanonicalValue::String)
                    .map_err(Error::from_contract_diagnostic)
            }
            Self::Encoded(value) => value
                .to_canonical_value(&ValidationPath::root())
                .map_err(super::literal_error),
            Self::Double(value) => type_bridge_contract::value::CanonicalDouble::new(value)
                .map(CanonicalValue::Double)
                .map_err(Error::from_contract_diagnostic),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A language scalar accepted by a generated plan field.
pub trait Scalar: sealed::Sealed + Sized {
    #[doc(hidden)]
    const VALUE_TYPE: ValueTypeTag;
    #[doc(hidden)]
    fn into_plan_scalar(self) -> ScalarInput;
}

macro_rules! scalars {
    ($($ty:ty => $tag:ident),* $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl Scalar for $ty {
            const VALUE_TYPE: ValueTypeTag = ValueTypeTag::$tag;
            fn into_plan_scalar(self) -> ScalarInput { ScalarInput::Encoded(self.into_encoded_scalar()) }
        }
    )*};
}
scalars!(i64 => Long, bool => Boolean, CanonicalDouble => Double,
    Decimal => Decimal, Date => Date, DateTime => DateTime, DateTimeTz => DateTimeTz,
    Duration => Duration);
impl sealed::Sealed for String {}
impl Scalar for String {
    const VALUE_TYPE: ValueTypeTag = ValueTypeTag::String;
    fn into_plan_scalar(self) -> ScalarInput {
        ScalarInput::Encoded(EncodedScalar::String(self))
    }
}
impl sealed::Sealed for f64 {}
impl Scalar for f64 {
    const VALUE_TYPE: ValueTypeTag = ValueTypeTag::Double;
    fn into_plan_scalar(self) -> ScalarInput {
        ScalarInput::Double(self)
    }
}

/// Canonical generated attribute domains mapped to plan-language scalars.
#[doc(hidden)]
pub trait Domain {
    type Scalar: Scalar;
}
macro_rules! domains {
    ($($ty:ty => $value:ty),* $(,)?) => {$(impl Domain for $ty { type Scalar = $value; })*};
}
domains!(String => String, i64 => i64, bool => bool, CanonicalDouble => f64,
    Decimal => Decimal, Date => Date, DateTime => DateTime, DateTimeTz => DateTimeTz,
    Duration => Duration);

struct Recorder {
    id: u64,
    core: CoreBuilder,
    error: Option<Error>,
    open: bool,
    writes: bool,
    bulk: bool,
}

impl Recorder {
    fn check<T>(&mut self, value: Result<T>) -> Option<T> {
        match value {
            Ok(value) => Some(value),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }
    fn owns(&mut self, id: u64) -> bool {
        if self.open && self.id == id {
            true
        } else {
            self.error.get_or_insert_with(context_error);
            false
        }
    }
    fn assignment(&mut self, site: Site, value: UntypedArgument) -> QueryOperand {
        match value {
            UntypedArgument::Expression { id, core } => {
                self.owns(id);
                QueryOperand::Binding {
                    binding: self.core.let_value(site, core),
                }
            }
            value => self.operand(site, value),
        }
    }
    fn operand(&mut self, site: Site, value: UntypedArgument) -> QueryOperand {
        match value {
            UntypedArgument::Scalar(value) => match self.check(value.canonical()) {
                Some(value) => self.core.bind_value(site, value),
                None => QueryOperand::Literal {
                    value: CanonicalValue::Boolean(false),
                },
            },
            UntypedArgument::Value(value) => {
                self.owns(value.id);
                QueryOperand::Binding {
                    binding: value.binding,
                }
            }
            UntypedArgument::Expression { .. } => {
                self.error.get_or_insert_with(context_error);
                QueryOperand::Literal {
                    value: CanonicalValue::Boolean(false),
                }
            }
        }
    }
}

/// The explicit recorder supplied to a Rust plan-building closure.
pub struct Recording<S: Schema> {
    inner: Rc<RefCell<Recorder>>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Recording<S> {
    /// Create a recorder with immutable caller-selected limits.
    #[must_use]
    pub fn new(limits: QueryLimits) -> Self {
        type_bridge_orm::debug::init();
        Self {
            inner: Rc::new(RefCell::new(Recorder {
                id: NEXT_RECORDING.fetch_add(1, Ordering::Relaxed),
                core: CoreBuilder::with_limits(limits),
                error: None,
                open: true,
                writes: false,
                bulk: false,
            })),
            marker: PhantomData,
        }
    }

    /// Select the recorded matched-row count.
    pub fn matched(&mut self) -> super::Plan<super::Matched, S> {
        self.select(SelectionKind::Matched)
    }

    /// Count matched read rows.
    pub fn count(&mut self) -> super::Plan<i64, S> {
        self.select(SelectionKind::Count)
    }

    /// Test whether any read row matches.
    pub fn exists(&mut self) -> super::Plan<bool, S> {
        self.select(SelectionKind::Exists)
    }

    /// Select a tuple of borrowed model handles and scalar values.
    ///
    /// For example, `p.returning((&item, item.stock()))` selects the model
    /// and its scalar in that order. Model proofs resolve before commit.
    pub fn returning<Columns: ReturningColumns<S>>(
        &mut self,
        columns: Columns,
    ) -> super::Plan<Vec<Columns::Output>, S> {
        let references = columns.references();
        let mut recorder = self.inner.borrow_mut();
        let references = recorder.check(references).unwrap_or_default();
        for reference in &references {
            recorder.owns(reference.id);
        }
        let columns = references
            .iter()
            .map(|reference| reference.binding)
            .collect();
        let models = references
            .iter()
            .filter_map(|reference| {
                reference
                    .descriptor
                    .map(|descriptor| (reference.binding, descriptor))
            })
            .collect();
        drop(recorder);
        self.select(SelectionKind::Rows { columns, models })
    }

    fn select<R>(&self, kind: SelectionKind) -> super::Plan<R, S> {
        let inner = self.inner.borrow();
        super::Plan::held(Selection {
            id: inner.id,
            kind,
            writes: inner.writes,
        })
    }

    /// Close and validate a recording against its generated schema package.
    ///
    /// # Errors
    /// Returns the first invalid handle, scalar, token or core validation error.
    pub fn build(
        self,
        schema: &crate::SchemaPackage<S>,
    ) -> Result<super::Plan<super::PlanAnswer, S>> {
        let mut recorder = match Rc::try_unwrap(self.inner) {
            Ok(inner) => inner.into_inner(),
            Err(_) => return Err(context_error()),
        };
        recorder.open = false;
        if let Some(error) = recorder.error {
            return Err(error);
        }
        let authority = schema.plan_authority()?;
        if !recorder.writes {
            recorder.core.count();
        }
        let plan = recorder
            .core
            .build(
                authority.declared_schema(),
                authority.managed_state().managed_semantic_schema().clone(),
            )
            .map_err(Error::from_contract_diagnostic)?;
        Ok(super::Plan::from_core(plan))
    }

    /// Require a condition involving bound values in this recording.
    pub fn require(&mut self, condition: Condition) {
        let mut inner = self.inner.borrow_mut();
        if inner.owns(condition.id) {
            inner
                .core
                .compare(condition.left, condition.comparator, condition.right);
        }
    }

    /// Begin a generated insert or put without allocating a handle cache.
    #[doc(hidden)]
    pub fn create<M: ThingModel<Schema = S>>(&mut self, put: bool) -> Created<M> {
        let mut inner = self.inner.borrow_mut();
        let binding = match inner.check(super::type_id::<M>()) {
            Some(model) if put => inner.core.put(model),
            Some(model) => inner.core.insert(model),
            None => BindingId::new(0).expect("zero binding"),
        };
        inner.writes = true;
        Created {
            reference: PlayerReference {
                id: inner.id,
                binding,
            },
            marker: PhantomData,
        }
    }

    /// Record standalone entity rows; the core groups shapes and splits limits.
    #[doc(hidden)]
    pub fn bulk<M: ThingModel<Schema = S>>(
        &mut self,
        put: bool,
        columns: &[&'static str],
        rows: Vec<Vec<Option<ScalarInput>>>,
    ) -> Created<M> {
        let created = self.create::<M>(put);
        let mut recorder = self.inner.borrow_mut();
        let columns = columns
            .iter()
            .map(|field| super::owns_id(field))
            .collect::<Result<Vec<_>>>();
        let rows = rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| value.map(ScalarInput::canonical).transpose())
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>();
        if let (Some(columns), Some(rows)) = (recorder.check(columns), recorder.check(rows)) {
            recorder
                .core
                .bind_model_columns(created.reference.binding, columns, rows);
            recorder.bulk = true;
        }
        created
    }

    /// Match exactly one generated model domain.
    #[doc(hidden)]
    pub fn matching<M: ThingModel<Schema = S>>(
        &mut self,
        subtypes: bool,
        iid: Option<&crate::Iid>,
    ) -> Handle<M> {
        self.matching_mode::<M, Exact>(subtypes, iid)
    }

    /// Match a generated root including its known subtypes.
    #[doc(hidden)]
    pub fn subtypes<M: ThingModel<Schema = S>>(
        &mut self,
        iid: Option<&crate::Iid>,
    ) -> Handle<M, Subtypes> {
        self.matching_mode::<M, Subtypes>(true, iid)
    }

    fn matching_mode<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &mut self,
        subtypes: bool,
        iid: Option<&crate::Iid>,
    ) -> Handle<M, Mode> {
        let mut inner = self.inner.borrow_mut();
        let binding = match inner.check(super::type_id::<M>()) {
            Some(model) => {
                if subtypes {
                    inner.core.subtypes(model)
                } else {
                    inner.core.exact(model)
                }
            }
            None => BindingId::new(0).expect("zero binding"),
        };
        if let Some(iid) = iid {
            inner.core.by_iid(binding, iid.as_str());
        }
        Handle {
            inner: Rc::new(HandleInner {
                recording: Rc::downgrade(&self.inner),
                id: inner.id,
                binding,
                fields: RefCell::new(BTreeMap::new()),
            }),
            marker: PhantomData,
        }
    }
}

/// A binding used by generated role arguments in the same recording.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PlayerReference {
    id: u64,
    binding: BindingId,
}

/// Nominal model and selection information retained by generated handles.
#[doc(hidden)]
pub trait BoundModel {
    type Model: ThingModel;
    type Mode: SelectionMode;
    fn player_reference(&self) -> Result<PlayerReference>;
}

/// A player admitted by a generated relation role.
#[doc(hidden)]
pub trait Player<Accepted> {
    fn player_reference(&self) -> Result<PlayerReference>;
}
impl<T: BoundModel, Accepted: RolePlayerBinding<T::Model, T::Mode>> Player<Accepted> for T {
    fn player_reference(&self) -> Result<PlayerReference> {
        BoundModel::player_reference(self)
    }
}

/// Generated model and subtype result families that may be used as role players.
#[doc(hidden)]
pub trait ModelResult {
    type Model: ThingModel;
    type Mode: SelectionMode;
}
impl<T: ModelResult> BoundModel for super::Plan<Vec<T>, <T::Model as Model>::Schema> {
    type Model = T::Model;
    type Mode = T::Mode;
    fn player_reference(&self) -> Result<PlayerReference> {
        match &self.inner {
            super::PlanState::Held(Selection {
                id,
                kind: SelectionKind::Models { binding, .. },
                ..
            }) => Ok(PlayerReference {
                id: *id,
                binding: *binding,
            }),
            _ => Err(context_error()),
        }
    }
}

/// A new generated model binding, populated by its generated insert struct.
#[doc(hidden)]
pub struct Created<M: ThingModel> {
    reference: PlayerReference,
    marker: PhantomData<fn() -> M>,
}
impl<M: ThingModel> BoundModel for Created<M> {
    type Model = M;
    type Mode = Exact;
    fn player_reference(&self) -> Result<PlayerReference> {
        Ok(self.reference)
    }
}
impl<M: ThingModel> Created<M> {
    /// Populate one scalar field through implicit given inputs.
    pub fn field<T: Scalar>(
        &self,
        recording: &mut Recording<M::Schema>,
        owns: &'static str,
        value: impl Into<Argument<T>>,
    ) {
        let mut inner = recording.inner.borrow_mut();
        if !inner.owns(self.reference.id) {
            return;
        }
        let Some(field) = inner.check(super::owns_id(owns)) else {
            return;
        };
        let operand = inner.assignment(
            Site::new(self.reference.binding, field.attribute().label().as_str()),
            value.into().erase(),
        );
        inner
            .core
            .thing_field(self.reference.binding, field, operand);
    }

    /// Populate one nominal generated role.
    pub fn player<Owner, Accepted>(
        &self,
        recording: &mut Recording<M::Schema>,
        role: RoleToken<Owner, Accepted>,
        player: &dyn Player<Accepted>,
    ) where
        M: RoleTokenCompatible<Owner, Accepted>,
        Owner: RelationModel<Schema = M::Schema>,
    {
        let reference = player.player_reference();
        let mut inner = recording.inner.borrow_mut();
        let Some(player) = inner.check(reference) else {
            return;
        };
        if !inner.owns(self.reference.id) || !inner.owns(player.id) {
            return;
        }
        let Some(role) = inner.check(super::role_id(role.role_id_json())) else {
            return;
        };
        inner
            .core
            .thing_player(self.reference.binding, role, player.binding);
    }

    /// Select this new model as the plan result.
    pub fn models(
        &self,
        recording: &mut Recording<M::Schema>,
        roles: &'static [(&'static str, bool)],
    ) -> super::Plan<Vec<M>, M::Schema> {
        recording.inner.borrow_mut().owns(self.reference.id);
        recording.select(SelectionKind::Models {
            binding: self.reference.binding,
            roles,
            descriptor_json: M::TYPE_ID_JSON,
        })
    }
}

struct HandleInner {
    recording: Weak<RefCell<Recorder>>,
    id: u64,
    binding: BindingId,
    fields: RefCell<BTreeMap<&'static str, CachedField>>,
}

#[derive(Clone, Copy)]
struct CachedField {
    binding: BindingId,
    name: &'static str,
}

fn attribute_name(identity: &'static str) -> Result<&'static str> {
    let invalid = || super::identity_error("invalid_field_identity");
    let fields: BTreeMap<&str, &serde_json::value::RawValue> =
        serde_json::from_str(identity).map_err(|_| invalid())?;
    let field = fields.get("attribute").ok_or_else(invalid)?;
    serde_json::from_str(field.get()).map_err(|_| invalid())
}

/// Resolution of exact and subtype handles to their model result type.
#[doc(hidden)]
pub trait ModelSelection<M: ThingModel>: SelectionMode {
    type Output;
}
impl<M: ThingModel> ModelSelection<M> for Exact {
    type Output = M;
}
impl<M: crate::__codegen::SubtypeRootModel> ModelSelection<M> for Subtypes {
    type Output = M::Subtypes;
}

/// A bound generated model inside a recording.
pub struct Handle<M: ThingModel, Mode: SelectionMode = Exact> {
    inner: Rc<HandleInner>,
    marker: PhantomData<fn() -> (M, Mode)>,
}
impl<M: ThingModel, Mode: SelectionMode> Clone for Handle<M, Mode> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            marker: PhantomData,
        }
    }
}

impl<M: ThingModel, Mode: SelectionMode> BoundModel for Handle<M, Mode> {
    type Model = M;
    type Mode = Mode;
    fn player_reference(&self) -> Result<PlayerReference> {
        self.inner
            .recording
            .upgrade()
            .filter(|inner| inner.borrow().open)
            .ok_or_else(context_error)?;
        Ok(PlayerReference {
            id: self.inner.id,
            binding: self.inner.binding,
        })
    }
}

impl HandleInner {
    fn set(&self, recording: &Rc<RefCell<Recorder>>, owns: &'static str, value: UntypedArgument) {
        let mut recorder = recording.borrow_mut();
        if !recorder.owns(self.id) {
            return;
        }
        let Some(field) = recorder.check(super::owns_id(owns)) else {
            return;
        };
        let site = Site::new(self.binding, field.attribute().label().as_str());
        let value = recorder.assignment(site, value);
        recorder.core.update(self.binding, field, value);
        recorder.writes = true;
    }

    fn value(&self, owns: &'static str) -> BoundValue {
        let mut field = CachedField {
            binding: BindingId::new(0).expect("zero binding"),
            name: "value",
        };
        if let Some(recording) = self.recording.upgrade() {
            let mut recorder = recording.borrow_mut();
            if recorder.owns(self.id) {
                let cached = self.fields.borrow().get(owns).copied();
                if let Some(existing) = cached {
                    field = existing;
                } else if let Some(name) = recorder.check(attribute_name(owns))
                    && let Some(attribute) = recorder.check(
                        type_bridge_contract::id::AttributeId::new(name)
                            .map_err(Error::from_contract_diagnostic),
                    )
                {
                    field = CachedField {
                        binding: recorder.core.field_value(self.binding, attribute),
                        name,
                    };
                    self.fields.borrow_mut().insert(owns, field);
                }
            }
        }
        BoundValue {
            recording: self.recording.clone(),
            id: self.id,
            binding: field.binding,
            owner: self.binding,
            field: field.name,
        }
    }
}

impl<M: ThingModel, Mode: SelectionMode> Handle<M, Mode> {
    /// Select this generated model after the recorded writes.
    #[doc(hidden)]
    pub fn models(
        &self,
        recording: &mut Recording<M::Schema>,
        roles: &'static [(&'static str, bool)],
    ) -> super::Plan<Vec<<Mode as ModelSelection<M>>::Output>, M::Schema>
    where
        Mode: ModelSelection<M>,
    {
        recording.inner.borrow_mut().owns(self.inner.id);
        recording.select(SelectionKind::Models {
            binding: self.inner.binding,
            roles,
            descriptor_json: M::TYPE_ID_JSON,
        })
    }
    /// Refuse repeated fields in a generated named-match builder.
    #[doc(hidden)]
    pub fn duplicate_match_field(&self) {
        if let Some(recording) = self.inner.recording.upgrade() {
            recording.borrow_mut().error.get_or_insert_with(|| {
                Error::model_validation(
                    ModelValidationPhase::Input,
                    "plan_duplicate_match_field",
                    vec![],
                    "a named match field was supplied more than once",
                    None,
                )
            });
        }
    }

    /// Match a generated key through the shared implicit-input path.
    #[doc(hidden)]
    pub fn key<T: Scalar>(self, owns: &'static str, value: T) -> Self {
        let field = self.value::<T>(owns);
        if let Some(recording) = self.inner.recording.upgrade() {
            let mut inner = recording.borrow_mut();
            if inner.owns(self.inner.id) {
                let operand = inner.operand(
                    Site::new(self.inner.binding, "key"),
                    UntypedArgument::Scalar(value.into_plan_scalar()),
                );
                inner.core.compare(
                    QueryOperand::Binding {
                        binding: field.binding.binding,
                    },
                    ValueComparator::Equal,
                    operand,
                );
            }
        }
        self
    }

    /// Delete this matched model after any earlier recorded operations.
    pub fn delete(&self, recording: &mut Recording<M::Schema>) {
        let mut inner = recording.inner.borrow_mut();
        if inner.owns(self.inner.id) {
            inner.core.delete(self.inner.binding);
            inner.writes = true;
        }
    }

    /// Remove one generated ownership, preserving its owner.
    #[doc(hidden)]
    pub fn unset(&self, recording: &mut Recording<M::Schema>, owns: &'static str) {
        let mut inner = recording.inner.borrow_mut();
        if !inner.owns(self.inner.id) {
            return;
        }
        if let Some(field) = inner.check(super::owns_id(owns)) {
            inner.core.delete_field(self.inner.binding, field);
            inner.writes = true;
        }
    }

    /// Add or remove one generated unordered role player.
    #[doc(hidden)]
    pub fn link<Owner, Accepted>(
        &self,
        recording: &mut Recording<M::Schema>,
        role: RoleToken<Owner, Accepted>,
        player: &dyn Player<Accepted>,
        unlink: bool,
    ) where
        M: RoleTokenCompatible<Owner, Accepted>,
        Owner: RelationModel<Schema = M::Schema>,
    {
        let reference = player.player_reference();
        let mut inner = recording.inner.borrow_mut();
        let Some(player) = inner.check(reference) else {
            return;
        };
        if !inner.owns(self.inner.id) || !inner.owns(player.id) {
            return;
        }
        if let Some(role) = inner.check(super::role_id(role.role_id_json())) {
            if unlink {
                inner
                    .core
                    .delete_player(self.inner.binding, role, player.binding);
            } else {
                inner.core.link(self.inner.binding, role, player.binding);
            }
            inner.writes = true;
        }
    }

    /// Bind and cache a generated scalar field on first access.
    #[doc(hidden)]
    pub fn value<T: Scalar>(&self, owns: &'static str) -> Value<T> {
        Value {
            binding: self.inner.value(owns),
            marker: PhantomData,
        }
    }

    /// Require a generated class-field or bound-value condition.
    pub fn filter<C: FilterFor<M, Mode>>(self, condition: C) -> Self {
        condition.apply(&self);
        self
    }

    /// Replace a scalar owned value, recording an expression as one implicit let.
    #[doc(hidden)]
    pub fn set<T: Scalar>(
        &self,
        recording: &mut Recording<M::Schema>,
        owns: &'static str,
        value: impl Into<Argument<T>>,
    ) {
        self.inner.set(&recording.inner, owns, value.into().erase());
    }
}

#[derive(Clone)]
struct BoundValue {
    recording: Weak<RefCell<Recorder>>,
    id: u64,
    binding: BindingId,
    owner: BindingId,
    field: &'static str,
}

/// A typed bound scalar; caller operands become implicit given inputs.
pub struct Value<T: Scalar> {
    binding: BoundValue,
    marker: PhantomData<fn() -> T>,
}
impl<T: Scalar> Clone for Value<T> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
            marker: PhantomData,
        }
    }
}

enum UntypedArgument {
    Scalar(ScalarInput),
    Value(BoundValue),
    Expression { id: u64, core: CoreExpr },
}

/// One scalar, bound value or expression supplied to a generated write.
pub enum Argument<T: Scalar> {
    /// A caller scalar, passed through given inputs.
    Scalar(T),
    /// A value already bound by the same recording.
    Value(Value<T>),
    /// An arithmetic expression evaluated by the provider.
    Expression(Expr<T>),
}
impl<T: Scalar> Argument<T> {
    fn erase(self) -> UntypedArgument {
        match self {
            Self::Scalar(value) => UntypedArgument::Scalar(value.into_plan_scalar()),
            Self::Value(value) => UntypedArgument::Value(value.binding),
            Self::Expression(value) => UntypedArgument::Expression {
                id: value.id,
                core: value.core,
            },
        }
    }
}

impl<T: Scalar> From<T> for Argument<T> {
    fn from(value: T) -> Self {
        Self::Scalar(value)
    }
}
impl<T: Scalar> From<Value<T>> for Argument<T> {
    fn from(value: Value<T>) -> Self {
        Self::Value(value)
    }
}
impl<T: Scalar> From<Expr<T>> for Argument<T> {
    fn from(value: Expr<T>) -> Self {
        Self::Expression(value)
    }
}
impl From<&str> for Argument<String> {
    fn from(value: &str) -> Self {
        Self::Scalar(value.to_owned())
    }
}

/// A scalar or bound value admitted on the right of a comparison.
pub struct Comparable<T: Scalar>(Argument<T>);
impl<T: Scalar> From<T> for Comparable<T> {
    fn from(value: T) -> Self {
        Self(Argument::Scalar(value))
    }
}
impl<T: Scalar> From<Value<T>> for Comparable<T> {
    fn from(value: Value<T>) -> Self {
        Self(Argument::Value(value))
    }
}
impl From<&str> for Comparable<String> {
    fn from(value: &str) -> Self {
        Self(Argument::Scalar(value.to_owned()))
    }
}

/// A comparison between values owned by a recording.
pub struct Condition {
    id: u64,
    left: QueryOperand,
    comparator: ValueComparator,
    right: QueryOperand,
}

impl<T: Scalar> Value<T> {
    fn compare(&self, comparator: ValueComparator, other: impl Into<Comparable<T>>) -> Condition {
        let mut right = QueryOperand::Literal {
            value: CanonicalValue::Boolean(false),
        };
        if let Some(recording) = self.binding.recording.upgrade() {
            let mut recorder = recording.borrow_mut();
            if recorder.owns(self.binding.id) {
                right = recorder.operand(
                    Site::new(self.binding.owner, self.binding.field),
                    other.into().0.erase(),
                );
            }
        }
        Condition {
            id: self.binding.id,
            left: QueryOperand::Binding {
                binding: self.binding.binding,
            },
            comparator,
            right,
        }
    }
    /// Compare equality in the provider.
    pub fn eq(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::Equal, other)
    }
    /// Compare inequality in the provider.
    pub fn ne(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::NotEqual, other)
    }
    /// Compare less than in the provider.
    pub fn lt(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::Less, other)
    }
    /// Compare at most in the provider.
    pub fn le(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::LessOrEqual, other)
    }
    /// Compare greater than in the provider.
    pub fn gt(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::Greater, other)
    }
    /// Compare at least in the provider.
    pub fn ge(&self, other: impl Into<Comparable<T>>) -> Condition {
        self.compare(ValueComparator::GreaterOrEqual, other)
    }
}

impl Value<String> {
    /// Match a substring in the provider.
    pub fn contains(&self, other: impl Into<Comparable<String>>) -> Condition {
        self.compare(ValueComparator::Contains, other)
    }
    /// Match a prefix in the provider.
    pub fn starts_with(&self, other: impl Into<Comparable<String>>) -> Condition {
        self.compare(ValueComparator::StartsWith, other)
    }
    /// Match a suffix in the provider.
    pub fn ends_with(&self, other: impl Into<Comparable<String>>) -> Condition {
        self.compare(ValueComparator::EndsWith, other)
    }
    /// Match the provider's string pattern.
    pub fn like(&self, other: impl Into<Comparable<String>>) -> Condition {
        self.compare(ValueComparator::Like, other)
    }
}

/// A class token condition resolved against the handle being filtered.
pub struct FieldCondition<Owner: Model, T: Scalar> {
    owns: &'static str,
    comparator: ValueComparator,
    right: Comparable<T>,
    marker: PhantomData<fn() -> Owner>,
}

/// Admission of a condition to a nominal generated handle.
#[doc(hidden)]
pub trait FilterFor<M: ThingModel, Mode: SelectionMode = Exact> {
    fn apply(self, handle: &Handle<M, Mode>);
}
impl<M: ThingModel, Mode: SelectionMode> FilterFor<M, Mode> for Condition {
    fn apply(self, handle: &Handle<M, Mode>) {
        if let Some(recording) = handle.inner.recording.upgrade() {
            let mut recorder = recording.borrow_mut();
            if recorder.owns(self.id) && recorder.owns(handle.inner.id) {
                recorder
                    .core
                    .compare(self.left, self.comparator, self.right);
            }
        }
    }
}
impl<M, Owner, T, Mode> FilterFor<M, Mode> for FieldCondition<Owner, T>
where
    M: ThingModel + NominalUpcast<Owner>,
    Owner: Model<Schema = M::Schema>,
    T: Scalar,
    Mode: SelectionMode,
{
    fn apply(self, handle: &Handle<M, Mode>) {
        let value = handle.value::<T>(self.owns);
        value.compare(self.comparator, self.right).apply(handle);
    }
}

macro_rules! token_comparisons {
    ($($name:ident => $op:ident),* $(,)?) => {$(
        #[doc = concat!("Construct a plan condition using `", stringify!($name), "` on this generated field.")]
        pub fn $name(self, right: impl Into<Comparable<<V::Domain as Domain>::Scalar>>) -> FieldCondition<Owner, <V::Domain as Domain>::Scalar> {
            FieldCondition { owns: self.owns_id_json(), comparator: ValueComparator::$op, right: right.into(), marker: PhantomData }
        }
    )*};
}
impl<Owner: Model, V: QueryValued> FieldToken<Owner, V>
where
    V::Domain: Domain,
{
    token_comparisons!(eq => Equal, ne => NotEqual, lt => Less, le => LessOrEqual, gt => Greater, ge => GreaterOrEqual);
}

impl<Owner: Model, V: QueryValued> FieldToken<Owner, V>
where
    V::Domain: Domain<Scalar = String>,
{
    token_comparisons!(contains => Contains, starts_with => StartsWith, ends_with => EndsWith, like => Like);
}

/// A typed arithmetic expression in a recording.
pub struct Expr<T: Scalar> {
    recording: Weak<RefCell<Recorder>>,
    id: u64,
    core: CoreExpr,
    owner: BindingId,
    field: &'static str,
    marker: PhantomData<fn() -> T>,
}

impl<T: Scalar> Value<T> {
    fn expression(&self, op: ArithOp, right: Argument<T>) -> Expr<T> {
        let mut rhs = CoreExpr::Operand(QueryOperand::Literal {
            value: CanonicalValue::Boolean(false),
        });
        if let Some(recording) = self.binding.recording.upgrade() {
            let mut recorder = recording.borrow_mut();
            if recorder.owns(self.binding.id) {
                rhs = match right {
                    Argument::Expression(value) => {
                        recorder.owns(value.id);
                        value.core
                    }
                    other => CoreExpr::Operand(recorder.operand(
                        Site::new(self.binding.owner, self.binding.field),
                        other.erase(),
                    )),
                };
            }
        }
        Expr {
            recording: self.binding.recording.clone(),
            id: self.binding.id,
            core: CoreExpr::Arith {
                op,
                lhs: Box::new(CoreExpr::Operand(QueryOperand::Binding {
                    binding: self.binding.binding,
                })),
                rhs: Box::new(rhs),
            },
            owner: self.binding.owner,
            field: self.binding.field,
            marker: PhantomData,
        }
    }
}

impl<T: Scalar> Expr<T> {
    fn arithmetic(self, op: ArithOp, right: Argument<T>) -> Self {
        let mut rhs = CoreExpr::Operand(QueryOperand::Literal {
            value: CanonicalValue::Boolean(false),
        });
        if let Some(recording) = self.recording.upgrade() {
            let mut recorder = recording.borrow_mut();
            if recorder.owns(self.id) {
                rhs = match right {
                    Argument::Expression(value) => {
                        recorder.owns(value.id);
                        value.core
                    }
                    other => CoreExpr::Operand(
                        recorder.operand(Site::new(self.owner, self.field), other.erase()),
                    ),
                };
            }
        }
        Self {
            core: CoreExpr::Arith {
                op,
                lhs: Box::new(self.core),
                rhs: Box::new(rhs),
            },
            ..self
        }
    }
}

macro_rules! arithmetic {
    ($ty:ty, $trait:ident, $method:ident, $op:ident) => {
        impl<R: Into<Argument<$ty>>> std::ops::$trait<R> for Value<$ty> {
            type Output = Expr<$ty>;
            fn $method(self, right: R) -> Self::Output {
                self.expression(ArithOp::$op, right.into())
            }
        }
        impl<R: Into<Argument<$ty>>> std::ops::$trait<R> for Expr<$ty> {
            type Output = Self;
            fn $method(self, right: R) -> Self::Output {
                self.arithmetic(ArithOp::$op, right.into())
            }
        }
    };
}
arithmetic!(i64, Add, add, Add);
arithmetic!(i64, Sub, sub, Sub);
arithmetic!(i64, Mul, mul, Mul);
arithmetic!(f64, Add, add, Add);
arithmetic!(f64, Sub, sub, Sub);
arithmetic!(f64, Mul, mul, Mul);

macro_rules! division {
    ($ty:ty) => {
        impl<R: Into<Argument<$ty>>> std::ops::Div<R> for Value<$ty> {
            type Output = Expr<f64>;
            fn div(self, right: R) -> Self::Output {
                let value = self.expression(ArithOp::Div, right.into());
                Expr {
                    recording: value.recording,
                    id: value.id,
                    core: value.core,
                    owner: value.owner,
                    field: value.field,
                    marker: PhantomData,
                }
            }
        }
        impl<R: Into<Argument<$ty>>> std::ops::Div<R> for Expr<$ty> {
            type Output = Expr<f64>;
            fn div(self, right: R) -> Self::Output {
                let value = self.arithmetic(ArithOp::Div, right.into());
                Expr {
                    recording: value.recording,
                    id: value.id,
                    core: value.core,
                    owner: value.owner,
                    field: value.field,
                    marker: PhantomData,
                }
            }
        }
    };
}
division!(i64);
division!(f64);

#[doc(hidden)]
pub struct ReturningReference {
    id: u64,
    binding: BindingId,
    descriptor: Option<&'static str>,
}
mod returning_sealed {
    pub trait Column<S> {}
    pub trait Columns<S> {}
}
/// Sealed typed model or scalar selected by an authored tuple.
#[doc(hidden)]
pub trait ReturningColumn<S: Schema>: returning_sealed::Column<S> {
    type Output;
    fn reference(self) -> Result<ReturningReference>;
}
impl<S: Schema, T: Scalar> returning_sealed::Column<S> for Value<T> {}
impl<S: Schema, T: Scalar> ReturningColumn<S> for Value<T> {
    type Output = T;
    fn reference(self) -> Result<ReturningReference> {
        Ok(ReturningReference {
            id: self.binding.id,
            binding: self.binding.binding,
            descriptor: None,
        })
    }
}
impl<S: Schema, T: Scalar> returning_sealed::Column<S> for &Value<T> {}
impl<S: Schema, T: Scalar> ReturningColumn<S> for &Value<T> {
    type Output = T;
    fn reference(self) -> Result<ReturningReference> {
        <Value<T> as ReturningColumn<S>>::reference((*self).clone())
    }
}
impl<S: Schema, H: BoundModel> returning_sealed::Column<S> for &H
where
    H::Model: ThingModel<Schema = S>,
    H::Mode: ModelSelection<H::Model>,
{
}
impl<S: Schema, H: BoundModel> ReturningColumn<S> for &H
where
    H::Model: ThingModel<Schema = S>,
    H::Mode: ModelSelection<H::Model>,
{
    type Output = <H::Mode as ModelSelection<H::Model>>::Output;
    fn reference(self) -> Result<ReturningReference> {
        let reference = self.player_reference()?;
        Ok(ReturningReference {
            id: reference.id,
            binding: reference.binding,
            descriptor: Some(H::Model::TYPE_ID_JSON),
        })
    }
}
/// Sealed tuple of typed authored selection columns.
#[doc(hidden)]
pub trait ReturningColumns<S: Schema>: returning_sealed::Columns<S> {
    type Output;
    fn references(self) -> Result<Vec<ReturningReference>>;
}
macro_rules! returning_columns {
    ($($ty:ident:$index:tt),+) => {
        impl<S: Schema, $($ty: ReturningColumn<S>),+> returning_sealed::Columns<S> for ($($ty,)+) {}
        impl<S: Schema, $($ty: ReturningColumn<S>),+> ReturningColumns<S> for ($($ty,)+) {
            type Output = ($($ty::Output,)+);
            fn references(self) -> Result<Vec<ReturningReference>> { Ok(vec![$(self.$index.reference()?,)+]) }
        }
    };
}
returning_columns!(A:0);
returning_columns!(A:0,B:1);
returning_columns!(A:0,B:1,C:2);
returning_columns!(A:0,B:1,C:2,D:3);
returning_columns!(A:0,B:1,C:2,D:3,E:4);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14);
returning_columns!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14,P:15);

#[derive(Clone)]
pub(super) struct Selection {
    id: u64,
    kind: SelectionKind,
    pub(super) writes: bool,
}

#[derive(Clone)]
enum SelectionKind {
    Matched,
    Count,
    Exists,
    Rows {
        columns: Vec<BindingId>,
        models: Vec<(BindingId, &'static str)>,
    },
    Models {
        binding: BindingId,
        roles: &'static [(&'static str, bool)],
        descriptor_json: &'static str,
    },
}

/// Conversion of a closure's selected result to its plan type.
#[doc(hidden)]
pub trait IntoSelection<R, S: Schema> {
    fn into_selection(self) -> super::Plan<R, S>;
}
impl<R, S: Schema> IntoSelection<R, S> for super::Plan<R, S> {
    fn into_selection(self) -> super::Plan<R, S> {
        self
    }
}
impl<S: Schema> IntoSelection<super::Matched, S> for () {
    fn into_selection(self) -> super::Plan<super::Matched, S> {
        super::Plan::held(Selection {
            id: 0,
            kind: SelectionKind::Matched,
            writes: false,
        })
    }
}

/// Immutable query limits captured before a generated closure is recorded.
pub struct PlanAuthoring<R, S: Schema> {
    limits: QueryLimits,
    marker: PhantomData<fn() -> (R, S)>,
}

impl<R, S: Schema> PlanAuthoring<R, S> {
    pub(super) fn new(limits: QueryLimits) -> Self {
        Self {
            limits,
            marker: PhantomData,
        }
    }

    /// Record a generated program, then validate only its selected projection.
    ///
    /// # Errors
    /// Returns a closed or foreign selection, or the first recording failure.
    pub fn build<B: super::PlanSchema, Output>(
        self,
        body: impl FnOnce(&mut Recording<B>) -> Output,
    ) -> Result<super::Plan<R, S>>
    where
        S: super::StageSchema<B>,
        Output: IntoSelection<R, B>,
    {
        let mut recording = Recording::<B>::new(self.limits);
        let selected = body(&mut recording).into_selection();
        let super::PlanState::Held(selection) = selected.inner else {
            return Err(context_error());
        };
        let mut recorder = match Rc::try_unwrap(recording.inner) {
            Ok(inner) => inner.into_inner(),
            Err(_) => return Err(context_error()),
        };
        if selection.id != 0 && !recorder.owns(selection.id) {
            return Err(context_error());
        }
        recorder.open = false;
        if let Some(error) = recorder.error {
            return Err(error);
        }
        let model = match &selection.kind {
            SelectionKind::Models {
                binding,
                descriptor_json,
                ..
            } => Some((*binding, *descriptor_json)),
            _ => None,
        };
        let tuple_models = match &selection.kind {
            SelectionKind::Rows { models, .. } => models.clone(),
            _ => vec![],
        };
        let models = model.is_some();
        match selection.kind {
            SelectionKind::Matched => {
                if !recorder.writes {
                    recorder.core.count();
                }
            }
            SelectionKind::Count => {
                recorder.core.count();
            }
            SelectionKind::Exists => {
                recorder.core.exists();
            }
            SelectionKind::Rows { columns, .. } => {
                recorder.core.returning(columns);
            }
            SelectionKind::Models { binding, roles, .. } => {
                let roles = roles
                    .iter()
                    .map(|(role, required)| {
                        super::role_id(role).map(|role| {
                            type_bridge_contract::query_plan::DocumentRole::new(role, *required)
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                recorder.core.fetch_held(binding, roles);
            }
        }
        let authority = B::package().plan_authority()?;
        if recorder.bulk {
            let plans = recorder
                .core
                .build_plans(
                    authority.declared_schema(),
                    authority.managed_state().managed_semantic_schema().clone(),
                    false,
                    type_bridge_contract::split::SplitLimits::CANONICAL,
                )
                .map_err(Error::from_contract_diagnostic)?;
            if models {
                plans
                    .validate_model_projection()
                    .map_err(Error::from_contract_diagnostic)?;
            }
            return match model {
                Some((binding, descriptor)) => {
                    super::Plan::from_model_plans::<B>(plans, binding, descriptor)
                }
                None => Ok(super::Plan::from_bulk(plans)),
            };
        }
        let plan = recorder
            .core
            .build(
                authority.declared_schema(),
                authority.managed_state().managed_semantic_schema().clone(),
            )
            .map_err(Error::from_contract_diagnostic)?;
        if models {
            plan.validate_model_projection()
                .map_err(Error::from_contract_diagnostic)?;
        }
        if !tuple_models.is_empty() {
            return super::Plan::from_tuple_plan::<B>(plan, &tuple_models);
        }
        match model {
            Some((binding, descriptor)) => super::Plan::from_model_plans::<B>(
                super::BuiltPlans::single(plan),
                binding,
                descriptor,
            ),
            None => Ok(super::Plan::from_core(plan)),
        }
    }
}
