//! ABI 1.7 plan builder: an opaque handle over the core `query-plan/v3`
//! [`PlanBuilder`].
//!
//! Every call forwards to the core builder, so a C program gets the same
//! canonical plan bytes as the same program written with any other SDK
//! builder. C adds only what the other SDKs check in their type systems:
//! labels, constants, values and ids. Identities (types, ownerships, roles),
//! values and expressions are registered once and named by dense `uint32_t`
//! ids, like the generated tokens of the Rust SDK.
//!
//! Calls never fail on content. The first invalid C argument is kept and
//! reported by [`type_bridge_plan_builder_build`] ahead of the core
//! builder's own first invalid call. A call returns `INVALID_ARGUMENT` only
//! for its pointer contract: a null builder or output slot, a view or array
//! with null data and a non-zero length, or an output slot that overlaps the
//! builder handle or an input. Output slots may be unaligned; a fenced
//! output is cleared before the remaining checks.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::str::FromStr;

use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::v3::{ArithOp, Builtin, Expr, PlanBuilder};
use type_bridge_contract::query_plan::{InputColumnId, QueryOperand};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::temporal::{CanonicalDate, CanonicalDateTime, CanonicalDuration};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_schema::parse_provider_datetime_tz;

use crate::abi::{TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_SCHEMA_PACKAGE, direct_output_preflight,
};
use crate::staged::{TypeBridgePlan, contract_error, invalid, prepare_handle_outputs};

/// Thing kinds accepted by [`type_bridge_plan_builder_type`].
pub const TYPE_BRIDGE_PLAN_THING_ENTITY: u32 = 1;
pub const TYPE_BRIDGE_PLAN_THING_RELATION: u32 = 2;

/// Operand kinds: the id names a binding, an input column, a registered
/// value or (where an expression is accepted) a registered expression.
pub const TYPE_BRIDGE_PLAN_OPERAND_NONE: u32 = 0;
pub const TYPE_BRIDGE_PLAN_OPERAND_BINDING: u32 = 1;
pub const TYPE_BRIDGE_PLAN_OPERAND_INPUT: u32 = 2;
pub const TYPE_BRIDGE_PLAN_OPERAND_VALUE: u32 = 3;
pub const TYPE_BRIDGE_PLAN_OPERAND_EXPR: u32 = 4;

/// A row cell with no value (an optional input left absent).
pub const TYPE_BRIDGE_PLAN_VALUE_ABSENT: u32 = u32::MAX;

/// Opaque `query-plan/v3` program under construction. Not thread-safe.
pub struct TypeBridgePlanBuilder {
    core: PlanBuilder,
    types: Vec<Option<TypeId>>,
    fields: Vec<Option<OwnsFactId>>,
    roles: Vec<Option<RoleId>>,
    values: Vec<Option<CanonicalValue>>,
    exprs: Vec<Option<Expr>>,
    error: Option<SdkExecutionDiagnostic>,
}

fn label_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_builder_label_invalid",
        "a type, attribute, role or name argument is not a valid label",
    )
}

fn constant_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_builder_constant_invalid",
        "a kind, comparator, operator or function constant is unknown",
    )
}

fn id_unknown() -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_builder_id_unknown",
        "an id does not name an identity, value or expression of this builder",
    )
}

fn operand_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_builder_operand_invalid",
        "a plain binding, input or value operand is required here",
    )
}

fn value_invalid() -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_builder_value_invalid",
        "a value argument is not in its canonical form",
    )
}

/// Copy a view as UTF-8 text.
///
/// # Safety
/// The view passed the pointer contract: non-null data names `length`
/// readable bytes.
unsafe fn text(view: TypeBridgeByteView) -> Option<String> {
    if view.length > MAX_CANONICAL_STRING_BYTES {
        return None;
    }
    if view.length == 0 {
        return Some(String::new());
    }
    // SAFETY: the caller contract above.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.length) };
    String::from_utf8(bytes.to_vec()).ok()
}

/// Copy `count` possibly unaligned ids.
///
/// # Safety
/// `values` names `count` readable ids, or is null with `count == 0`.
unsafe fn ids(values: *const u32, count: usize) -> Vec<u32> {
    (0..count)
        // SAFETY: the caller contract above; reads may be unaligned.
        .map(|index| unsafe { values.add(index).read_unaligned() })
        .collect()
}

fn push<T>(list: &mut Vec<Option<T>>, value: Option<T>) -> u32 {
    list.push(value);
    u32::try_from(list.len() - 1).unwrap_or(u32::MAX)
}

impl TypeBridgePlanBuilder {
    fn new() -> Self {
        Self {
            core: PlanBuilder::new(),
            types: Vec::new(),
            fields: Vec::new(),
            roles: Vec::new(),
            values: Vec::new(),
            exprs: Vec::new(),
            error: None,
        }
    }

    fn fail(&mut self, error: SdkExecutionDiagnostic) {
        self.error.get_or_insert(error);
    }

    fn get<T: Clone>(&mut self, list: fn(&Self) -> &Vec<Option<T>>, id: u32) -> Option<T> {
        let found = usize::try_from(id)
            .ok()
            .and_then(|index| list(self).get(index).cloned())
            .flatten();
        // A placeholder id fails too; its own invalid call is recorded first.
        if found.is_none() {
            self.fail(id_unknown());
        }
        found
    }

    fn type_id(&mut self, id: u32) -> Option<TypeId> {
        self.get(|b| &b.types, id)
    }

    fn field_id(&mut self, id: u32) -> Option<OwnsFactId> {
        self.get(|b| &b.fields, id)
    }

    fn role_id(&mut self, id: u32) -> Option<RoleId> {
        self.get(|b| &b.roles, id)
    }

    fn binding(&mut self, id: u32) -> Option<BindingId> {
        let binding = u16::try_from(id).ok().and_then(|n| BindingId::new(n).ok());
        if binding.is_none() {
            self.fail(id_unknown());
        }
        binding
    }

    fn operand(&mut self, kind: u32, id: u32) -> Option<QueryOperand> {
        match kind {
            TYPE_BRIDGE_PLAN_OPERAND_BINDING => self
                .binding(id)
                .map(|binding| QueryOperand::Binding { binding }),
            TYPE_BRIDGE_PLAN_OPERAND_INPUT => match u16::try_from(id) {
                Ok(column) => Some(QueryOperand::Input {
                    column: InputColumnId::new(column),
                }),
                Err(_) => {
                    self.fail(id_unknown());
                    None
                }
            },
            TYPE_BRIDGE_PLAN_OPERAND_VALUE => self
                .get(|b| &b.values, id)
                .map(|value| QueryOperand::Literal { value }),
            TYPE_BRIDGE_PLAN_OPERAND_EXPR => {
                self.fail(operand_invalid());
                None
            }
            _ => {
                self.fail(constant_invalid());
                None
            }
        }
    }

    fn expr(&mut self, kind: u32, id: u32) -> Option<Expr> {
        if kind == TYPE_BRIDGE_PLAN_OPERAND_EXPR {
            self.get(|b| &b.exprs, id)
        } else {
            self.operand(kind, id).map(Expr::Operand)
        }
    }

    fn register_type(&mut self, kind: u32, label: Option<String>) -> u32 {
        let kind = match kind {
            TYPE_BRIDGE_PLAN_THING_ENTITY => Some(TypeKind::Entity),
            TYPE_BRIDGE_PLAN_THING_RELATION => Some(TypeKind::Relation),
            _ => None,
        };
        let type_id = match (kind, label) {
            (None, _) => {
                self.fail(constant_invalid());
                None
            }
            (Some(kind), Some(label)) => TypeId::new(kind, label).ok(),
            (Some(_), None) => None,
        };
        if type_id.is_none() {
            self.fail(label_invalid());
        }
        push(&mut self.types, type_id)
    }

    fn register_field(&mut self, owner: u32, attribute: Option<String>) -> u32 {
        let owner = self.type_id(owner);
        let field = match (owner, attribute) {
            (Some(owner), Some(attribute)) => {
                let field = AttributeId::new(attribute)
                    .ok()
                    .and_then(|attribute| OwnsFactId::new(owner, attribute).ok());
                if field.is_none() {
                    self.fail(label_invalid());
                }
                field
            }
            (Some(_), None) => {
                self.fail(label_invalid());
                None
            }
            (None, _) => None,
        };
        push(&mut self.fields, field)
    }

    fn register_role(&mut self, relation: u32, role: Option<String>) -> u32 {
        let relation = self.type_id(relation);
        let role = match (relation, role) {
            (Some(relation), Some(role)) => {
                let role = RoleId::new(relation.label().as_str(), role).ok();
                if role.is_none() {
                    self.fail(label_invalid());
                }
                role
            }
            (Some(_), None) => {
                self.fail(label_invalid());
                None
            }
            (None, _) => None,
        };
        push(&mut self.roles, role)
    }

    fn register_value(&mut self, value: Option<CanonicalValue>) -> u32 {
        if value.is_none() {
            self.fail(value_invalid());
        }
        push(&mut self.values, value)
    }

    fn register_expr(&mut self, expr: Option<Expr>) -> u32 {
        push(&mut self.exprs, expr)
    }

    fn thing(&mut self, type_id: u32, call: fn(&mut PlanBuilder, TypeId) -> BindingId) -> u32 {
        self.type_id(type_id)
            .map_or(0, |type_id| u32::from(call(&mut self.core, type_id).get()))
    }

    fn name(&mut self, name: Option<String>) -> Option<String> {
        if name.is_none() {
            self.fail(label_invalid());
        }
        name
    }
}

/// Fence the output slot against the builder handle and every input range,
/// clear it, then require the builder and every non-empty input pointer.
///
/// # Safety
/// A non-null `out` names one writable, possibly unaligned, `u32` slot.
unsafe fn enter(
    builder: *const TypeBridgePlanBuilder,
    out: Option<*mut u32>,
    inputs: &[(*const c_void, usize)],
) -> Result<(), TypeBridgeStatus> {
    let slot = out.unwrap_or(ptr::null_mut());
    let preflight = match out {
        Some(out) => direct_output_preflight(&[(out.cast(), size_of::<u32>())])?,
        None => direct_output_preflight(&[])?,
    };
    if !builder.is_null() {
        preflight.check_bytes(builder.cast(), size_of::<TypeBridgePlanBuilder>())?;
    }
    for &(pointer, length) in inputs {
        if !pointer.is_null() {
            preflight.check_bytes(pointer, length)?;
        }
    }
    if !slot.is_null() {
        // SAFETY: the slot is writable and overlaps no fenced input.
        unsafe { slot.write_unaligned(0) };
    }
    if builder.is_null()
        || (out.is_some() && slot.is_null())
        || inputs
            .iter()
            .any(|&(pointer, length)| pointer.is_null() && length != 0)
    {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(())
}

fn view_range(view: TypeBridgeByteView) -> (*const c_void, usize) {
    (view.data.cast(), view.length)
}

fn id_range(values: *const u32, count: usize) -> Result<(*const c_void, usize), TypeBridgeStatus> {
    let length = count
        .checked_mul(size_of::<u32>())
        .ok_or(TypeBridgeStatus::InvalidArgument)?;
    Ok((values.cast(), length))
}

/// Run one builder call after its pointer contract holds.
///
/// # Safety
/// `builder` is null or a live handle not used concurrently; a non-null
/// `out` is one writable slot; each input range is readable for the call.
unsafe fn run(
    builder: *mut TypeBridgePlanBuilder,
    out: Option<*mut u32>,
    inputs: &[(*const c_void, usize)],
    call: impl FnOnce(&mut TypeBridgePlanBuilder) -> u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    if let Err(status) = unsafe { enter(builder, out, inputs) } {
        return status;
    }
    guarded(|| {
        // SAFETY: a live handle, fenced against the output slot.
        let id = call(unsafe { &mut *builder });
        if let Some(out) = out {
            // SAFETY: the slot was fenced and cleared above.
            unsafe { out.write_unaligned(id) };
        }
        TypeBridgeStatus::Ok
    })
}

/// Create an empty plan builder.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_new(
    out_builder: *mut *mut TypeBridgePlanBuilder,
) -> TypeBridgeStatus {
    if out_builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: a writable, possibly unaligned, owner slot by contract.
    unsafe { out_builder.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let handle = Box::new(TypeBridgePlanBuilder::new());
        // SAFETY: as above.
        unsafe { out_builder.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}

/// Free a plan builder and null its owner slot. A null slot is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_free(builder: *mut *mut TypeBridgePlanBuilder) {
    // SAFETY: forwarded owner-slot contract.
    unsafe { crate::staged::free_slot(builder) }
}

/// Register a thing type identity: `kind` is ENTITY or RELATION.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_type(
    builder: *mut TypeBridgePlanBuilder,
    kind: u32,
    label: TypeBridgeByteView,
    out_type: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: the view is read only after the pointer contract holds.
    unsafe {
        run(builder, Some(out_type), &[view_range(label)], |b| {
            b.register_type(kind, text(label))
        })
    }
}

/// Register the ownership of `attribute` by the registered type `owner`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_owns(
    builder: *mut TypeBridgePlanBuilder,
    owner: u32,
    attribute: TypeBridgeByteView,
    out_field: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: as in `type_bridge_plan_builder_type`.
    unsafe {
        run(builder, Some(out_field), &[view_range(attribute)], |b| {
            b.register_field(owner, text(attribute))
        })
    }
}

/// Register the role `role` declared by the registered relation type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_role(
    builder: *mut TypeBridgePlanBuilder,
    relation: u32,
    role: TypeBridgeByteView,
    out_role: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: as in `type_bridge_plan_builder_type`.
    unsafe {
        run(builder, Some(out_role), &[view_range(role)], |b| {
            b.register_role(relation, text(role))
        })
    }
}

/// Match things of exactly the registered type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_exact(
    builder: *mut TypeBridgePlanBuilder,
    type_id: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_binding), &[], |b| {
            b.thing(type_id, PlanBuilder::exact)
        })
    }
}

/// Match things of the registered type or any subtype.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_subtypes(
    builder: *mut TypeBridgePlanBuilder,
    type_id: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_binding), &[], |b| {
            b.thing(type_id, PlanBuilder::subtypes)
        })
    }
}

/// Bind the value of `owner`'s registered field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_field(
    builder: *mut TypeBridgePlanBuilder,
    owner: u32,
    field: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_binding), &[], |b| {
            match (b.binding(owner), b.field_id(field)) {
                (Some(owner), Some(field)) => {
                    u32::from(b.core.field(owner, field.attribute().clone()).get())
                }
                _ => 0,
            }
        })
    }
}

/// Require `relation` to link `player` in the registered role.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_links(
    builder: *mut TypeBridgePlanBuilder,
    relation: u32,
    role: u32,
    player: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(relation), Some(role), Some(player)) =
                (b.binding(relation), b.role_id(role), b.binding(player))
            {
                b.core.links(relation, role, player);
            }
            0
        })
    }
}

/// Require `left comparator right` over plain operands.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_compare(
    builder: *mut TypeBridgePlanBuilder,
    left_kind: u32,
    left: u32,
    comparator: u32,
    right_kind: u32,
    right: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            let comparator = match comparator {
                1 => Some(ValueComparator::Equal),
                2 => Some(ValueComparator::NotEqual),
                3 => Some(ValueComparator::Less),
                4 => Some(ValueComparator::LessOrEqual),
                5 => Some(ValueComparator::Greater),
                6 => Some(ValueComparator::GreaterOrEqual),
                _ => {
                    b.fail(constant_invalid());
                    None
                }
            };
            let left = b.operand(left_kind, left);
            let right = b.operand(right_kind, right);
            if let (Some(left), Some(comparator), Some(right)) = (left, comparator, right) {
                b.core.compare(left, comparator, right);
            }
            0
        })
    }
}

fn value_type(kind: i32) -> Option<ValueTypeTag> {
    Some(match kind {
        1 => ValueTypeTag::String,
        2 => ValueTypeTag::Long,
        3 => ValueTypeTag::Double,
        4 => ValueTypeTag::Boolean,
        5 => ValueTypeTag::Date,
        6 => ValueTypeTag::DateTime,
        7 => ValueTypeTag::DateTimeTz,
        8 => ValueTypeTag::Decimal,
        9 => ValueTypeTag::Duration,
        _ => return None,
    })
}

/// Declare a typed input column; `value_type` is a
/// `type_bridge_projected_value_kind_t` and `optional` is zero or one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_input(
    builder: *mut TypeBridgePlanBuilder,
    name: TypeBridgeByteView,
    value_type_kind: i32,
    optional: u8,
    out_input: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: the view is read only after the pointer contract holds.
    unsafe {
        run(builder, Some(out_input), &[view_range(name)], |b| {
            let name = b.name(text(name));
            let optional = match optional {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            };
            match (name, value_type(value_type_kind), optional) {
                (Some(name), Some(value_type), Some(optional)) => {
                    u32::from(b.core.input(&name, value_type, optional).get())
                }
                (None, ..) => 0,
                _ => {
                    b.fail(constant_invalid());
                    0
                }
            }
        })
    }
}

/// Bind `name` to an operand or a registered expression.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_let(
    builder: *mut TypeBridgePlanBuilder,
    name: TypeBridgeByteView,
    kind: u32,
    id: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: the view is read only after the pointer contract holds.
    unsafe {
        run(builder, Some(out_binding), &[view_range(name)], |b| {
            let name = b.name(text(name));
            match (name, b.expr(kind, id)) {
                (Some(name), Some(expr)) => u32::from(b.core.let_(&name, expr).get()),
                _ => 0,
            }
        })
    }
}

/// Register `left op right`; `op` is a TYPE_BRIDGE_PLAN_ARITH_* constant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_expr_arith(
    builder: *mut TypeBridgePlanBuilder,
    op: u32,
    left_kind: u32,
    left: u32,
    right_kind: u32,
    right: u32,
    out_expr: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_expr), &[], |b| {
            let op = match op {
                1 => Some(ArithOp::Add),
                2 => Some(ArithOp::Sub),
                3 => Some(ArithOp::Mul),
                4 => Some(ArithOp::Div),
                5 => Some(ArithOp::Mod),
                6 => Some(ArithOp::Pow),
                _ => {
                    b.fail(constant_invalid());
                    None
                }
            };
            let lhs = b.expr(left_kind, left);
            let rhs = b.expr(right_kind, right);
            let expr = match (op, lhs, rhs) {
                (Some(op), Some(lhs), Some(rhs)) => Some(Expr::Arith {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                }),
                _ => None,
            };
            b.register_expr(expr)
        })
    }
}

/// Register a builtin call; `function` is a TYPE_BRIDGE_PLAN_CALL_* constant.
/// A unary function takes `second_kind` TYPE_BRIDGE_PLAN_OPERAND_NONE.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_expr_call(
    builder: *mut TypeBridgePlanBuilder,
    function: u32,
    first_kind: u32,
    first: u32,
    second_kind: u32,
    second: u32,
    out_expr: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_expr), &[], |b| {
            let func = match function {
                1 => Some(Builtin::Abs),
                2 => Some(Builtin::Ceil),
                3 => Some(Builtin::Floor),
                4 => Some(Builtin::Round),
                5 => Some(Builtin::Min),
                6 => Some(Builtin::Max),
                7 => Some(Builtin::Length),
                _ => {
                    b.fail(constant_invalid());
                    None
                }
            };
            let mut args = Vec::with_capacity(2);
            let mut complete = true;
            for (kind, id) in [(first_kind, first), (second_kind, second)] {
                if kind == TYPE_BRIDGE_PLAN_OPERAND_NONE {
                    continue;
                }
                match b.expr(kind, id) {
                    Some(arg) => args.push(arg),
                    None => complete = false,
                }
            }
            let expr = func
                .filter(|_| complete)
                .map(|func| Expr::Call { func, args });
            b.register_expr(expr)
        })
    }
}

/// Replace `owner`'s registered field value with a plain operand.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_update(
    builder: *mut TypeBridgePlanBuilder,
    owner: u32,
    field: u32,
    operand_kind: u32,
    operand: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(owner), Some(field), Some(operand)) = (
                b.binding(owner),
                b.field_id(field),
                b.operand(operand_kind, operand),
            ) {
                b.core.update(owner, field, operand);
            }
            0
        })
    }
}

/// Set `relation`'s registered role player to `player`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_update_player(
    builder: *mut TypeBridgePlanBuilder,
    relation: u32,
    role: u32,
    player: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(relation), Some(role), Some(player)) =
                (b.binding(relation), b.role_id(role), b.binding(player))
            {
                b.core.update_player(relation, role, player);
            }
            0
        })
    }
}

/// Create a thing of the registered type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_insert(
    builder: *mut TypeBridgePlanBuilder,
    type_id: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_binding), &[], |b| {
            b.thing(type_id, PlanBuilder::insert)
        })
    }
}

/// Match a thing of the registered type with its fields and players, or
/// create it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_put(
    builder: *mut TypeBridgePlanBuilder,
    type_id: u32,
    out_binding: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_binding), &[], |b| {
            b.thing(type_id, PlanBuilder::put)
        })
    }
}

/// Give an inserted or put thing a registered field value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_thing_field(
    builder: *mut TypeBridgePlanBuilder,
    thing: u32,
    field: u32,
    operand_kind: u32,
    operand: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(thing), Some(field), Some(operand)) = (
                b.binding(thing),
                b.field_id(field),
                b.operand(operand_kind, operand),
            ) {
                b.core.thing_field(thing, field, operand);
            }
            0
        })
    }
}

/// Give an inserted or put relation the `player` in a registered role.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_thing_player(
    builder: *mut TypeBridgePlanBuilder,
    thing: u32,
    role: u32,
    player: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(thing), Some(role), Some(player)) =
                (b.binding(thing), b.role_id(role), b.binding(player))
            {
                b.core.thing_player(thing, role, player);
            }
            0
        })
    }
}

/// Delete `thing`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_delete(
    builder: *mut TypeBridgePlanBuilder,
    thing: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let Some(thing) = b.binding(thing) {
                b.core.delete(thing);
            }
            0
        })
    }
}

/// Delete `owner`'s registered field ownership, keeping the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_delete_field(
    builder: *mut TypeBridgePlanBuilder,
    owner: u32,
    field: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(owner), Some(field)) = (b.binding(owner), b.field_id(field)) {
                b.core.delete_field(owner, field);
            }
            0
        })
    }
}

/// Remove `player` from `relation`'s registered role.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_delete_player(
    builder: *mut TypeBridgePlanBuilder,
    relation: u32,
    role: u32,
    player: u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            if let (Some(relation), Some(role), Some(player)) =
                (b.binding(relation), b.role_id(role), b.binding(player))
            {
                b.core.delete_player(relation, role, player);
            }
            0
        })
    }
}

/// Add one input row of `count` value ids, positional by input column.
/// TYPE_BRIDGE_PLAN_VALUE_ABSENT leaves an optional cell empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_row(
    builder: *mut TypeBridgePlanBuilder,
    values: *const u32,
    count: usize,
) -> TypeBridgeStatus {
    let range = match id_range(values, count) {
        Ok(range) => range,
        Err(status) => return status,
    };
    // SAFETY: the array is read only after the pointer contract holds.
    unsafe {
        run(builder, None, &[range], |b| {
            let mut row = Vec::with_capacity(count);
            let mut complete = true;
            for id in ids(values, count) {
                if id == TYPE_BRIDGE_PLAN_VALUE_ABSENT {
                    row.push(None);
                    continue;
                }
                match b.get(|b| &b.values, id) {
                    Some(value) => row.push(Some(value)),
                    None => complete = false,
                }
            }
            if complete {
                b.core.row(row);
            }
            0
        })
    }
}

/// Project `count` bindings, in order. A write plan then answers rows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_returning(
    builder: *mut TypeBridgePlanBuilder,
    bindings: *const u32,
    count: usize,
) -> TypeBridgeStatus {
    let range = match id_range(bindings, count) {
        Ok(range) => range,
        Err(status) => return status,
    };
    // SAFETY: the array is read only after the pointer contract holds.
    unsafe {
        run(builder, None, &[range], |b| {
            let columns: Option<Vec<BindingId>> = ids(bindings, count)
                .into_iter()
                .map(|id| b.binding(id))
                .collect();
            if let Some(columns) = columns {
                b.core.returning(columns);
            }
            0
        })
    }
}

/// Answer the number of matched rows of a read plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_count(
    builder: *mut TypeBridgePlanBuilder,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            b.core.count();
            0
        })
    }
}

/// Answer whether a read plan matches any row.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_exists(
    builder: *mut TypeBridgePlanBuilder,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, None, &[], |b| {
            b.core.exists();
            0
        })
    }
}

/// # Safety
/// As [`run`]; `text` is read only after the pointer contract holds.
unsafe fn text_value(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
    parse: fn(&str) -> Option<CanonicalValue>,
) -> TypeBridgeStatus {
    // SAFETY: forwarded caller contract.
    unsafe {
        run(builder, Some(out_value), &[view_range(input)], |b| {
            let value = text(input).and_then(|text| parse(&text));
            b.register_value(value)
        })
    }
}

/// Register a UTF-8 string value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_string(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            CanonicalString::new(text).ok().map(CanonicalValue::String)
        })
    }
}

/// Register a signed 64-bit value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_long(
    builder: *mut TypeBridgePlanBuilder,
    input: i64,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_value), &[], |b| {
            b.register_value(Some(CanonicalValue::Long(input)))
        })
    }
}

/// Register a finite binary64 value from its exact IEEE-754 bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_double(
    builder: *mut TypeBridgePlanBuilder,
    input_bits: u64,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_value), &[], |b| {
            b.register_value(
                CanonicalDouble::from_bits(input_bits)
                    .ok()
                    .map(CanonicalValue::Double),
            )
        })
    }
}

/// Register a Boolean value from exactly zero or one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_boolean(
    builder: *mut TypeBridgePlanBuilder,
    input: u8,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        run(builder, Some(out_value), &[], |b| {
            b.register_value(match input {
                0 => Some(CanonicalValue::Boolean(false)),
                1 => Some(CanonicalValue::Boolean(true)),
                _ => None,
            })
        })
    }
}

/// Register a date value in canonical lexical form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_date(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            CanonicalDate::from_str(text).ok().map(CanonicalValue::Date)
        })
    }
}

/// Register a timezone-free date-time value in canonical lexical form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_datetime(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            CanonicalDateTime::from_str(text)
                .ok()
                .map(CanonicalValue::DateTime)
        })
    }
}

/// Register a timezone-aware date-time value in canonical lexical form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_datetime_tz(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            parse_provider_datetime_tz(text)
                .ok()
                .map(CanonicalValue::DateTimeTz)
        })
    }
}

/// Register a decimal value, which must already be normalized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_decimal(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            DecimalValue::new(text)
                .ok()
                .filter(|decimal| decimal.as_str() == text)
                .map(CanonicalValue::Decimal)
        })
    }
}

/// Register a duration value in canonical lexical form.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_value_duration(
    builder: *mut TypeBridgePlanBuilder,
    input: TypeBridgeByteView,
    out_value: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer contract.
    unsafe {
        text_value(builder, input, out_value, |text| {
            CanonicalDuration::from_str(text)
                .ok()
                .map(CanonicalValue::Duration)
        })
    }
}

/// Validate the program against `package`'s compiled schema authority and
/// return a plan handle. The builder stays usable; building again gives the
/// same bytes. The first invalid C argument, else the core builder's first
/// invalid call, else any plan validation failure is returned as
/// diagnostics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_build(
    builder: *const TypeBridgePlanBuilder,
    package: *const TypeBridgeSchemaPackage,
    out_plan: *mut *mut TypeBridgePlan,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        if !builder.is_null() {
            outputs.check_bytes(builder.cast(), size_of::<TypeBridgePlanBuilder>())?;
        }
        outputs.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())?;
        if !package.is_null() {
            // SAFETY: the complete package object was fenced above.
            outputs.check_package_borrowed_ranges(unsafe { &*package }.state())?;
        }
        Ok(())
    };
    // SAFETY: the outputs are fenced against the builder and package first.
    if let Err(status) = unsafe { prepare_handle_outputs(out_plan, out_diagnostics, inputs) } {
        return status;
    }
    if builder.is_null() || package.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: live handles by contract.
        let (builder, package) = unsafe { (&*builder, &*package) };
        if let Some(error) = &builder.error {
            return return_execution_error(error.clone(), out_diagnostics);
        }
        let authority = &package.state()._authority;
        let built = builder
            .core
            .build(
                authority.declared_schema(),
                authority.managed_state().managed_semantic_schema().clone(),
            )
            .and_then(|plan| plan.canonical_bytes().map(|canonical| (plan, canonical)));
        let (plan, canonical) = match built {
            Ok(value) => value,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "the plan builder program is invalid"),
                    out_diagnostics,
                );
            }
        };
        let handle = Box::new(TypeBridgePlan::new(plan, canonical));
        // SAFETY: the output slot was fenced and initialized above.
        unsafe { out_plan.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}
