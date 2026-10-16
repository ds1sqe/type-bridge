#![deny(missing_docs)]
//! Typed `query-plan/v3` programs built from generated model tokens.
//!
//! A [`PlanBuilder`] declares things, field values, inputs and `let`
//! values, then the writes over them. [`PlanBuilder::build`] validates the
//! program against the generated schema package and returns a [`Plan`],
//! which a staged [`Transaction`](crate::staged::Transaction) runs at
//! commit. The program is one pipeline: a write plan that matches nothing
//! writes nothing.
//!
//! The rules are the same in every SDK, so the same program gives the same
//! canonical bytes in Rust, Python, TypeScript and C:
//!
//! - Handles are numbered in creation order.
//! - `exact`, `subtypes`, `field`, `links`, `where_` and `let_` add to the
//!   plan's single leading match, in call order.
//! - Writes run in call order after the match. Consecutive writes of one
//!   kind (`update`, `insert`, `put`, `delete`) share one stage.
//! - Things and fields are named from their labels (`item`, `item_stock`),
//!   with `_2`, `_3`, ... on collision. `input` and `let_` names must be
//!   fresh.
//!
//! Handles belong to the builder that created them.
//!
//! ```ignore
//! let mut plan = Plan::builder();
//! let item = plan.exact::<Item>();
//! let stock = plan.field(item, ItemType::stock);
//! plan.where_(stock.gt(0_i64));
//! let next = plan.let_("next_stock", stock - 1_i64);
//! plan.update(item, ItemType::stock, next);
//! let plan = plan.build(&models::SCHEMA)?;
//! ```

use std::marker::PhantomData;

use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId};
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::v3::{
    ArithOp, Builtin, Expr as PlanExpr, PlanBuilder as CoreBuilder, QueryPlanV3,
};
use type_bridge_contract::query_plan::{InputColumnId, QueryOperand};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, FieldToken, Model,
    NominalUpcast, QueryValued, RelationModel, RolePlayerBinding, RoleToken, RoleTokenCompatible,
    ThingModel, ValidationError, ValidationPath,
};
use crate::Result;
use crate::error::{Error, ModelValidationPhase};
use crate::query::{Exact, SelectionMode, Subtypes};
use crate::schema::{Schema, SchemaPackage};

fn identity_error(code: &'static str) -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        code,
        vec![],
        "generated token identity is not canonical",
        None,
    )
}

fn literal_error(error: ValidationError) -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "plan_literal_invalid",
        vec![],
        "a plan literal is not a canonical value",
        Some(Box::new(error)),
    )
}

fn type_id<M: Model>() -> Result<TypeId> {
    from_canonical_json(M::TYPE_ID_JSON.as_bytes())
        .map_err(|_| identity_error("invalid_model_identity"))
}

fn owns_id(owns_id_json: &str) -> Result<OwnsFactId> {
    let invalid = || identity_error("invalid_field_identity");
    let value: serde_json::Value = serde_json::from_str(owns_id_json).map_err(|_| invalid())?;
    let owner: TypeId = value
        .get("owner")
        .cloned()
        .and_then(|owner| serde_json::from_value(owner).ok())
        .ok_or_else(invalid)?;
    let attribute = value
        .get("attribute")
        .and_then(serde_json::Value::as_str)
        .and_then(|attribute| AttributeId::new(attribute).ok())
        .ok_or_else(invalid)?;
    OwnsFactId::new(owner, attribute).map_err(|_| invalid())
}

fn role_id(role_id_json: &str) -> Result<RoleId> {
    from_canonical_json(role_id_json.as_bytes())
        .map_err(|_| identity_error("invalid_role_identity"))
}

fn canonical<T: QueryValued>(value: &T) -> std::result::Result<CanonicalValue, ValidationError> {
    value
        .into_encoded_scalar()
        .to_canonical_value(&ValidationPath::root())
}

mod sealed {
    pub trait Sealed {
        const VALUE_TYPE: super::ValueTypeTag;
    }
}

/// A scalar domain an input column can carry: the `Domain` of a generated
/// value wrapper or of a plain Rust scalar.
pub trait PlanScalar: sealed::Sealed {}

macro_rules! plan_scalar {
    ($($domain:ty => $tag:ident),* $(,)?) => {$(
        impl sealed::Sealed for $domain {
            const VALUE_TYPE: ValueTypeTag = ValueTypeTag::$tag;
        }
        impl PlanScalar for $domain {}
    )*};
}
plan_scalar!(
    String => String,
    i64 => Long,
    CanonicalDouble => Double,
    bool => Boolean,
    Decimal => Decimal,
    Date => Date,
    DateTime => DateTime,
    DateTimeTz => DateTimeTz,
    Duration => Duration,
);

/// A matched or created thing of model `M`.
pub struct Thing<M: ThingModel, Mode: SelectionMode = Exact> {
    binding: BindingId,
    marker: PhantomData<fn() -> (M, Mode)>,
}
impl<M: ThingModel, Mode: SelectionMode> Clone for Thing<M, Mode> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M: ThingModel, Mode: SelectionMode> Copy for Thing<M, Mode> {}
impl<M: ThingModel, Mode: SelectionMode> std::fmt::Debug for Thing<M, Mode> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Thing")
            .field(&self.binding.get())
            .finish()
    }
}

/// A scalar value bound by [`PlanBuilder::field`] or [`PlanBuilder::let_`].
#[derive(Clone, Copy, Debug)]
pub struct Value {
    binding: BindingId,
}

/// A typed input column; rows supply its values through [`Input::value`].
pub struct Input<V> {
    column: InputColumnId,
    marker: PhantomData<fn() -> V>,
}
impl<V> Clone for Input<V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<V> Copy for Input<V> {}
impl<V> std::fmt::Debug for Input<V> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Input")
            .field(&self.column.get())
            .finish()
    }
}

impl<V: QueryValued> Input<V> {
    /// This column's value in one input row.
    #[must_use]
    pub fn value(self, value: V) -> InputValue {
        InputValue {
            column: self.column,
            value: canonical(&value).map(Some),
        }
    }
}
impl<V> Input<V> {
    /// An absent value for an optional column.
    #[must_use]
    pub fn absent(self) -> InputValue {
        InputValue {
            column: self.column,
            value: Ok(None),
        }
    }
}

/// One column's value in an input row; see [`PlanBuilder::row`].
#[derive(Clone, Debug)]
pub struct InputValue {
    column: InputColumnId,
    value: std::result::Result<Option<CanonicalValue>, ValidationError>,
}

/// A scalar operand: a [`Value`], an [`Input`], or a literal.
#[derive(Clone, Debug)]
pub struct Operand(std::result::Result<QueryOperand, ValidationError>);

impl From<Value> for Operand {
    fn from(value: Value) -> Self {
        Self(Ok(QueryOperand::Binding {
            binding: value.binding,
        }))
    }
}
impl<V> From<Input<V>> for Operand {
    fn from(input: Input<V>) -> Self {
        Self(Ok(QueryOperand::Input {
            column: input.column,
        }))
    }
}
impl<T: QueryValued> From<T> for Operand {
    fn from(value: T) -> Self {
        Self(canonical(&value).map(|value| QueryOperand::Literal { value }))
    }
}

/// A comparison between two operands, added with [`PlanBuilder::where_`].
#[derive(Clone, Debug)]
pub struct Condition {
    left: Operand,
    comparator: ValueComparator,
    right: Operand,
}

macro_rules! comparisons {
    ($($name:ident => $comparator:ident, $doc:literal;)*) => {$(
        #[doc = $doc]
        #[must_use]
        pub fn $name(self, other: impl Into<Operand>) -> Condition {
            Condition {
                left: self.into(),
                comparator: ValueComparator::$comparator,
                right: other.into(),
            }
        }
    )*};
}
impl Value {
    comparisons! {
        eq => Equal, "Require this value to equal `other`.";
        ne => NotEqual, "Require this value to differ from `other`.";
        lt => Less, "Require this value to be less than `other`.";
        le => LessOrEqual, "Require this value to be at most `other`.";
        gt => Greater, "Require this value to be greater than `other`.";
        ge => GreaterOrEqual, "Require this value to be at least `other`.";
    }
}

/// A value expression for [`PlanBuilder::let_`]. Build it with `+`, `-`,
/// `*`, `/` and `%` over values, inputs and literals, or with the builtins.
#[derive(Clone, Debug)]
pub struct Expr(std::result::Result<PlanExpr, ValidationError>);

impl<T: Into<Operand>> From<T> for Expr {
    fn from(operand: T) -> Self {
        Self(operand.into().0.map(PlanExpr::Operand))
    }
}

impl Expr {
    fn arith(op: ArithOp, lhs: Self, rhs: Self) -> Self {
        Self(lhs.0.and_then(|lhs| {
            rhs.0.map(|rhs| PlanExpr::Arith {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            })
        }))
    }
    fn call(func: Builtin, args: Vec<Self>) -> Self {
        Self(
            args.into_iter()
                .map(|arg| arg.0)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map(|args| PlanExpr::Call { func, args }),
        )
    }
    /// `base` to the power `exponent`.
    #[must_use]
    pub fn pow(base: impl Into<Self>, exponent: impl Into<Self>) -> Self {
        Self::arith(ArithOp::Pow, base.into(), exponent.into())
    }
    /// Absolute value.
    #[must_use]
    pub fn abs(value: impl Into<Self>) -> Self {
        Self::call(Builtin::Abs, vec![value.into()])
    }
    /// Smallest integer not below a decimal or double.
    #[must_use]
    pub fn ceil(value: impl Into<Self>) -> Self {
        Self::call(Builtin::Ceil, vec![value.into()])
    }
    /// Largest integer not above a decimal or double.
    #[must_use]
    pub fn floor(value: impl Into<Self>) -> Self {
        Self::call(Builtin::Floor, vec![value.into()])
    }
    /// Nearest integer to a decimal or double.
    #[must_use]
    pub fn round(value: impl Into<Self>) -> Self {
        Self::call(Builtin::Round, vec![value.into()])
    }
    /// The smaller of two values of one numeric domain.
    #[must_use]
    pub fn min(left: impl Into<Self>, right: impl Into<Self>) -> Self {
        Self::call(Builtin::Min, vec![left.into(), right.into()])
    }
    /// The larger of two values of one numeric domain.
    #[must_use]
    pub fn max(left: impl Into<Self>, right: impl Into<Self>) -> Self {
        Self::call(Builtin::Max, vec![left.into(), right.into()])
    }
    /// The length of a string.
    #[must_use]
    pub fn length(value: impl Into<Self>) -> Self {
        Self::call(Builtin::Length, vec![value.into()])
    }
}

macro_rules! arithmetic {
    ($($trait:ident :: $method:ident => $op:ident;)*) => {$(
        impl<R: Into<Expr>> std::ops::$trait<R> for Expr {
            type Output = Expr;
            fn $method(self, rhs: R) -> Expr {
                Expr::arith(ArithOp::$op, self, rhs.into())
            }
        }
        impl<R: Into<Expr>> std::ops::$trait<R> for Value {
            type Output = Expr;
            fn $method(self, rhs: R) -> Expr {
                Expr::arith(ArithOp::$op, self.into(), rhs.into())
            }
        }
        impl<V, R: Into<Expr>> std::ops::$trait<R> for Input<V> {
            type Output = Expr;
            fn $method(self, rhs: R) -> Expr {
                Expr::arith(ArithOp::$op, self.into(), rhs.into())
            }
        }
    )*};
}
arithmetic! {
    Add::add => Add;
    Sub::sub => Sub;
    Mul::mul => Mul;
    Div::div => Div;
    Rem::rem => Mod;
}

/// A projected handle for [`PlanBuilder::returning`].
#[derive(Clone, Copy, Debug)]
pub struct Column(BindingId);
impl<M: ThingModel, Mode: SelectionMode> From<Thing<M, Mode>> for Column {
    fn from(thing: Thing<M, Mode>) -> Self {
        Self(thing.binding)
    }
}
impl From<Value> for Column {
    fn from(value: Value) -> Self {
        Self(value.binding)
    }
}

/// A validated `query-plan/v3` program for schema `S`.
///
/// Stage it on a [`Transaction`](crate::staged::Transaction) from
/// [`Database::transaction`](crate::Database::transaction) or
/// [`ProxyDatabase::transaction`](crate::ProxyDatabase::transaction).
pub struct Plan<S: Schema> {
    plan: QueryPlanV3,
    marker: PhantomData<fn() -> S>,
}
impl<S: Schema> Clone for Plan<S> {
    fn clone(&self) -> Self {
        Self {
            plan: self.plan.clone(),
            marker: PhantomData,
        }
    }
}
impl<S: Schema> std::fmt::Debug for Plan<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Plan")
            .field("writes", &self.plan.writes())
            .finish_non_exhaustive()
    }
}

impl<S: Schema> Plan<S> {
    /// Start an empty program.
    #[must_use]
    pub fn builder() -> PlanBuilder<S> {
        PlanBuilder::new()
    }

    /// The plan's canonical `query-plan/v3` bytes, identical in every SDK.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan exceeds the canonical codec limits.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.plan
            .canonical_bytes()
            .map_err(Error::from_contract_diagnostic)
    }

    /// Whether the plan writes, which makes an inferred transaction kind
    /// `Write`.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.plan.writes()
    }

    pub(crate) fn inner(&self) -> &QueryPlanV3 {
        &self.plan
    }
}

/// Builds one [`Plan`]. See the [module documentation](self) for the rules.
///
/// Methods never fail; the first invalid call is returned by
/// [`Self::build`].
pub struct PlanBuilder<S: Schema> {
    core: CoreBuilder,
    error: Option<Error>,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> Default for PlanBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Schema> std::fmt::Debug for PlanBuilder<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlanBuilder")
            .finish_non_exhaustive()
    }
}

impl<S: Schema> PlanBuilder<S> {
    /// An empty program.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: CoreBuilder::new(),
            error: None,
            marker: PhantomData,
        }
    }

    fn check<T>(&mut self, result: Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }

    fn operand(&mut self, operand: impl Into<Operand>) -> QueryOperand {
        let operand = operand.into().0.map_err(literal_error);
        self.check(operand).unwrap_or(QueryOperand::Literal {
            value: CanonicalValue::Boolean(false),
        })
    }

    fn thing<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &mut self,
        binding: impl FnOnce(&mut CoreBuilder, TypeId) -> BindingId,
    ) -> Thing<M, Mode> {
        let binding = match self.check(type_id::<M>()) {
            Some(type_id) => binding(&mut self.core, type_id),
            None => BindingId::new(0).expect("zero is a valid binding"),
        };
        Thing {
            binding,
            marker: PhantomData,
        }
    }

    /// Match things of exactly model `M`.
    pub fn exact<M: ThingModel<Schema = S>>(&mut self) -> Thing<M, Exact> {
        self.thing(CoreBuilder::exact)
    }

    /// Match things of model `M` or any of its subtypes.
    pub fn subtypes<M: ThingModel<Schema = S>>(&mut self) -> Thing<M, Subtypes> {
        self.thing(CoreBuilder::subtypes)
    }

    /// Bind the value of `thing`'s field. Things without the field do not
    /// match.
    pub fn field<M, Mode, Owner, V>(
        &mut self,
        thing: Thing<M, Mode>,
        token: FieldToken<Owner, V>,
    ) -> Value
    where
        M: ThingModel<Schema = S> + NominalUpcast<Owner>,
        Mode: SelectionMode,
        Owner: Model<Schema = S>,
    {
        let binding = match self.check(owns_id(token.owns_id_json())) {
            Some(field) => self.core.field(thing.binding, field.attribute().clone()),
            None => BindingId::new(0).expect("zero is a valid binding"),
        };
        Value { binding }
    }

    /// Declare an input column named `name`; each [`Self::row`] supplies a
    /// value. Many rows run the one plan over all of them.
    pub fn input<V>(&mut self, name: &str) -> Input<V>
    where
        V: QueryValued,
        V::Domain: PlanScalar,
    {
        let column = self
            .core
            .input(name, <V::Domain as sealed::Sealed>::VALUE_TYPE, false);
        Input {
            column,
            marker: PhantomData,
        }
    }

    /// Declare an optional input column; rows may give it
    /// [`Input::absent`].
    pub fn optional_input<V>(&mut self, name: &str) -> Input<V>
    where
        V: QueryValued,
        V::Domain: PlanScalar,
    {
        let column = self
            .core
            .input(name, <V::Domain as sealed::Sealed>::VALUE_TYPE, true);
        Input {
            column,
            marker: PhantomData,
        }
    }

    /// Require `condition` of every matched row.
    pub fn where_(&mut self, condition: Condition) -> &mut Self {
        let left = self.operand(condition.left);
        let right = self.operand(condition.right);
        self.core.compare(left, condition.comparator, right);
        self
    }

    /// Require `relation` to link `player` in `role`.
    pub fn links<R, RMode, Owner, Players, P, PMode>(
        &mut self,
        relation: Thing<R, RMode>,
        role: RoleToken<Owner, Players>,
        player: Thing<P, PMode>,
    ) -> &mut Self
    where
        R: ThingModel<Schema = S> + RoleTokenCompatible<Owner, Players>,
        RMode: SelectionMode,
        Owner: RelationModel<Schema = S>,
        Players: RolePlayerBinding<P, PMode>,
        P: ThingModel<Schema = S>,
        PMode: SelectionMode,
    {
        if let Some(role) = self.check(role_id(role.role_id_json())) {
            self.core.links(relation.binding, role, player.binding);
        }
        self
    }

    /// Bind `name` to `expr`, evaluated for each matched row.
    pub fn let_(&mut self, name: &str, expr: impl Into<Expr>) -> Value {
        let expr = expr.into().0.map_err(literal_error);
        let expr = self
            .check(expr)
            .unwrap_or(PlanExpr::Operand(QueryOperand::Literal {
                value: CanonicalValue::Boolean(false),
            }));
        Value {
            binding: self.core.let_(name, expr),
        }
    }

    /// Replace `thing`'s field value with `value`.
    pub fn update<M, Mode, Owner, V>(
        &mut self,
        thing: Thing<M, Mode>,
        token: FieldToken<Owner, V>,
        value: impl Into<Operand>,
    ) -> &mut Self
    where
        M: ThingModel<Schema = S> + NominalUpcast<Owner>,
        Mode: SelectionMode,
        Owner: Model<Schema = S>,
    {
        let operand = self.operand(value);
        if let Some(field) = self.check(owns_id(token.owns_id_json())) {
            self.core.update(thing.binding, field, operand);
        }
        self
    }

    /// Make `player` the `role` player of `relation`.
    pub fn update_player<R, RMode, Owner, Players, P, PMode>(
        &mut self,
        relation: Thing<R, RMode>,
        role: RoleToken<Owner, Players>,
        player: Thing<P, PMode>,
    ) -> &mut Self
    where
        R: ThingModel<Schema = S> + RoleTokenCompatible<Owner, Players>,
        RMode: SelectionMode,
        Owner: RelationModel<Schema = S>,
        Players: RolePlayerBinding<P, PMode>,
        P: ThingModel<Schema = S>,
        PMode: SelectionMode,
    {
        if let Some(role) = self.check(role_id(role.role_id_json())) {
            self.core
                .update_player(relation.binding, role, player.binding);
        }
        self
    }

    /// Create a thing of model `M`; give it fields and players on the
    /// returned [`NewThing`].
    pub fn insert<M: ThingModel<Schema = S>>(&mut self) -> NewThing<'_, S, M> {
        let thing = self.thing(CoreBuilder::insert);
        NewThing {
            builder: self,
            thing,
        }
    }

    /// Match a thing of model `M` with the fields and players given on the
    /// returned [`NewThing`], or create it.
    pub fn put<M: ThingModel<Schema = S>>(&mut self) -> NewThing<'_, S, M> {
        let thing = self.thing(CoreBuilder::put);
        NewThing {
            builder: self,
            thing,
        }
    }

    /// Delete `thing`.
    pub fn delete<M: ThingModel<Schema = S>, Mode: SelectionMode>(
        &mut self,
        thing: Thing<M, Mode>,
    ) -> &mut Self {
        self.core.delete(thing.binding);
        self
    }

    /// Delete `thing`'s field, keeping the thing.
    pub fn delete_field<M, Mode, Owner, V>(
        &mut self,
        thing: Thing<M, Mode>,
        token: FieldToken<Owner, V>,
    ) -> &mut Self
    where
        M: ThingModel<Schema = S> + NominalUpcast<Owner>,
        Mode: SelectionMode,
        Owner: Model<Schema = S>,
    {
        if let Some(field) = self.check(owns_id(token.owns_id_json())) {
            self.core.delete_field(thing.binding, field);
        }
        self
    }

    /// Remove `player` from `relation`'s `role`.
    pub fn delete_player<R, RMode, Owner, Players, P, PMode>(
        &mut self,
        relation: Thing<R, RMode>,
        role: RoleToken<Owner, Players>,
        player: Thing<P, PMode>,
    ) -> &mut Self
    where
        R: ThingModel<Schema = S> + RoleTokenCompatible<Owner, Players>,
        RMode: SelectionMode,
        Owner: RelationModel<Schema = S>,
        Players: RolePlayerBinding<P, PMode>,
        P: ThingModel<Schema = S>,
        PMode: SelectionMode,
    {
        if let Some(role) = self.check(role_id(role.role_id_json())) {
            self.core
                .delete_player(relation.binding, role, player.binding);
        }
        self
    }

    /// Add one input row. Every column appears at most once; a missing
    /// optional column is absent.
    pub fn row(&mut self, values: impl IntoIterator<Item = InputValue>) -> &mut Self {
        let mut row: Vec<Option<CanonicalValue>> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for value in values {
            let index = usize::from(value.column.get());
            if !seen.insert(index) {
                self.check::<()>(Err(Error::model_validation(
                    ModelValidationPhase::Input,
                    "plan_input_duplicate",
                    vec![],
                    "an input row gives one column twice",
                    None,
                )));
                return self;
            }
            let Some(value) = self.check(value.value.map_err(literal_error)) else {
                return self;
            };
            if row.len() <= index {
                row.resize(index + 1, None);
            }
            row[index] = value;
        }
        self.core.row(row);
        self
    }

    /// Project `columns`, in order. A write plan then answers these rows
    /// from the same pipeline instead of its matched count.
    pub fn returning(&mut self, columns: impl IntoIterator<Item = impl Into<Column>>) -> &mut Self {
        self.core
            .returning(columns.into_iter().map(|column| column.into().0).collect());
        self
    }

    /// Answer the number of rows a read plan matches.
    pub fn count(&mut self) -> &mut Self {
        self.core.count();
        self
    }

    /// Answer whether a read plan matches any row.
    pub fn exists(&mut self) -> &mut Self {
        self.core.exists();
        self
    }

    /// Validate the program against the generated schema package.
    ///
    /// A plan with writes answers its matched row count, or the
    /// [`Self::returning`] rows. A read answers rows, or
    /// [`Self::count`] / [`Self::exists`].
    ///
    /// # Errors
    ///
    /// Returns the first invalid builder call, a package without compiled
    /// schema authority, or the plan validation failure (for example a
    /// field the thing does not own, or a key field update).
    pub fn build(self, schema: &SchemaPackage<S>) -> Result<Plan<S>> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let (declared, semantics) = schema.plan_authority()?;
        let plan = self
            .core
            .build(&declared, semantics)
            .map_err(Error::from_contract_diagnostic)?;
        Ok(Plan {
            plan,
            marker: PhantomData,
        })
    }
}

/// A thing being inserted or put; add its fields and players, then take
/// its handle with [`Self::thing`].
pub struct NewThing<'a, S: Schema, M: ThingModel<Schema = S>> {
    builder: &'a mut PlanBuilder<S>,
    thing: Thing<M, Exact>,
}

impl<S: Schema, M: ThingModel<Schema = S>> std::fmt::Debug for NewThing<'_, S, M> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("NewThing")
            .field(&self.thing)
            .finish()
    }
}

impl<S: Schema, M: ThingModel<Schema = S>> NewThing<'_, S, M> {
    /// Give the new thing a field value.
    pub fn field<Owner, V>(self, token: FieldToken<Owner, V>, value: impl Into<Operand>) -> Self
    where
        M: NominalUpcast<Owner>,
        Owner: Model<Schema = S>,
    {
        let operand = self.builder.operand(value);
        if let Some(field) = self.builder.check(owns_id(token.owns_id_json())) {
            self.builder
                .core
                .thing_field(self.thing.binding, field, operand);
        }
        self
    }

    /// Give the new relation a role player.
    pub fn player<Owner, Players, P, PMode>(
        self,
        role: RoleToken<Owner, Players>,
        player: Thing<P, PMode>,
    ) -> Self
    where
        M: RoleTokenCompatible<Owner, Players>,
        Owner: RelationModel<Schema = S>,
        Players: RolePlayerBinding<P, PMode>,
        P: ThingModel<Schema = S>,
        PMode: SelectionMode,
    {
        if let Some(role) = self.builder.check(role_id(role.role_id_json())) {
            self.builder
                .core
                .thing_player(self.thing.binding, role, player.binding);
        }
        self
    }

    /// The new thing's handle.
    #[must_use]
    pub fn thing(self) -> Thing<M, Exact> {
        self.thing
    }
}

#[cfg(test)]
mod tests;
