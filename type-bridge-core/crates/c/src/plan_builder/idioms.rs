//! Typed C values over the existing core recorder.
use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use type_bridge_contract::query_plan::v3::Site;

const NULL: u32 = 0;
const LITERAL: u32 = 1;
const BINDING: u32 = 2;
const EXPRESSION: u32 = 3;
const POISON: u32 = 4;
const FIELD: u32 = 5;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct ValueBase {
    pub(super) builder: *mut c_void,
    pub(super) kind: u32,
    pub(super) id: u32,
    pub(super) owner: u32,
    pub(super) field: u32,
}
impl ValueBase {
    const LITERAL: Self = Self {
        builder: ptr::null_mut(),
        kind: LITERAL,
        id: 0,
        owner: 0,
        field: 0,
    };
    fn poison(constructor: u32) -> Self {
        Self {
            kind: POISON,
            id: constructor,
            ..Self::LITERAL
        }
    }
}

macro_rules! constructor_messages {
    ($($name:literal),* $(,)?) => { &[$(concat!($name, ": no operand carried a builder")),*] };
}
const CONSTRUCTOR_MESSAGES: &[&str] = constructor_messages![
    "typed value",
    "tb_long_add",
    "tb_long_sub",
    "tb_long_mul",
    "tb_long_div",
    "tb_double_add",
    "tb_double_sub",
    "tb_double_mul",
    "tb_double_div",
    "typed field",
    "tb_long_eq",
    "tb_long_ne",
    "tb_long_lt",
    "tb_long_le",
    "tb_long_gt",
    "tb_long_ge",
    "tb_double_eq",
    "tb_double_ne",
    "tb_double_lt",
    "tb_double_le",
    "tb_double_gt",
    "tb_double_ge",
    "tb_bool_eq",
    "tb_bool_ne",
    "tb_bool_lt",
    "tb_bool_le",
    "tb_bool_gt",
    "tb_bool_ge",
    "tb_string_eq",
    "tb_string_ne",
    "tb_string_lt",
    "tb_string_le",
    "tb_string_gt",
    "tb_string_ge",
    "tb_date_eq",
    "tb_date_ne",
    "tb_date_lt",
    "tb_date_le",
    "tb_date_gt",
    "tb_date_ge",
    "tb_datetime_eq",
    "tb_datetime_ne",
    "tb_datetime_lt",
    "tb_datetime_le",
    "tb_datetime_gt",
    "tb_datetime_ge",
    "tb_datetime_tz_eq",
    "tb_datetime_tz_ne",
    "tb_datetime_tz_lt",
    "tb_datetime_tz_le",
    "tb_datetime_tz_gt",
    "tb_datetime_tz_ge",
    "tb_decimal_eq",
    "tb_decimal_ne",
    "tb_decimal_lt",
    "tb_decimal_le",
    "tb_decimal_gt",
    "tb_decimal_ge",
    "tb_duration_eq",
    "tb_duration_ne",
    "tb_duration_lt",
    "tb_duration_le",
    "tb_duration_gt",
    "tb_duration_ge",
    "tb_string_contains",
    "tb_string_starts_with",
    "tb_string_ends_with",
    "tb_string_like",
];
fn constructor_error(id: u32) -> SdkExecutionDiagnostic {
    invalid(
        "c_plan_constructor_missing_builder",
        CONSTRUCTOR_MESSAGES
            .get(id as usize)
            .copied()
            .unwrap_or("typed constructor: no operand carried a builder"),
    )
}

#[derive(Clone)]
enum LiteralInput {
    Long(i64),
    Double(f64),
    Boolean(u8),
    Text {
        view: TypeBridgeByteView,
        parse: fn(&str, type_bridge_contract::limits::CodecLimits) -> Option<CanonicalValue>,
    },
}
impl LiteralInput {
    fn parse(
        self,
        limits: type_bridge_contract::limits::CodecLimits,
    ) -> Result<CanonicalValue, SdkExecutionDiagnostic> {
        match self {
            Self::Long(value) => Ok(CanonicalValue::Long(value)),
            Self::Double(value) => CanonicalDouble::new(value)
                .map(CanonicalValue::Double)
                .map_err(|_| value_invalid()),
            Self::Boolean(0) => Ok(CanonicalValue::Boolean(false)),
            Self::Boolean(1) => Ok(CanonicalValue::Boolean(true)),
            Self::Boolean(_) => Err(value_invalid()),
            Self::Text { view, parse } => {
                if view.length > limits.max_string_bytes.min(limits.max_bytes)
                    || (view.data.is_null() && view.length != 0)
                {
                    return Err(value_invalid());
                }
                let bytes = if view.length == 0 {
                    &[][..]
                } else {
                    // SAFETY: literal views remain readable until their consuming statement or constructor finishes.
                    unsafe { std::slice::from_raw_parts(view.data, view.length) }
                };
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|text| parse(text, limits))
                    .ok_or_else(value_invalid)
            }
        }
    }
}

#[derive(Clone)]
struct OperandValue {
    base: ValueBase,
    literal: LiteralInput,
}
#[derive(Clone)]
pub(super) enum NativeExpression {
    Raw(Expr),
    Typed(TypedArithmetic),
}
#[derive(Clone)]
pub(super) struct TypedArithmetic {
    source: ValueBase,
    left: OperandValue,
    right: OperandValue,
    op: ArithOp,
    binding: Option<BindingId>,
}

impl TypeBridgePlanBuilder {
    pub(super) fn recorded_expression(&mut self, id: u32) -> Option<Expr> {
        match self.get(|b| &b.exprs, id)? {
            NativeExpression::Raw(expr) => Some(expr),
            NativeExpression::Typed(node) => {
                let site = self.typed_site(node.source)?;
                self.materialize_expression(id, &site)
            }
        }
    }

    fn materialize_expression(&mut self, id: u32, site: &Site) -> Option<Expr> {
        struct Frame {
            id: u32,
            phase: u8,
            left: Option<Expr>,
            site: Option<Site>,
        }
        let mut stack = vec![Frame {
            id,
            phase: 0,
            left: None,
            site: None,
        }];
        let mut result = None;
        while let Some(frame) = stack.last_mut() {
            let node = match self.get(|b| &b.exprs, frame.id)? {
                NativeExpression::Raw(expr) => {
                    result = Some(expr);
                    stack.pop();
                    continue;
                }
                NativeExpression::Typed(node) => node,
            };
            if let Some(binding) = node.binding {
                result = Some(Expr::Operand(QueryOperand::Binding { binding }));
                stack.pop();
                continue;
            }
            let operand = match frame.phase {
                0 => {
                    frame.phase = 1;
                    frame.site = self.typed_site(node.source);
                    frame.site.as_ref()?;
                    node.left
                }
                1 => {
                    frame.left = result.take();
                    frame.phase = 2;
                    node.right
                }
                _ => {
                    let expression = Expr::Arith {
                        op: node.op,
                        lhs: Box::new(frame.left.take()?),
                        rhs: Box::new(result.take()?),
                    };
                    let binding = self.core.let_value(
                        if frame.id == id {
                            site.clone()
                        } else {
                            frame.site.as_ref()?.clone()
                        },
                        expression,
                    );
                    if let Some(Some(NativeExpression::Typed(node))) =
                        self.exprs.get_mut(frame.id as usize)
                    {
                        node.binding = Some(binding);
                    }
                    result = Some(Expr::Operand(QueryOperand::Binding { binding }));
                    stack.pop();
                    continue;
                }
            };
            if operand.base.kind == EXPRESSION {
                // Constructor admission guarantees child ids precede their parent.
                if operand.base.id >= frame.id {
                    self.fail(id_unknown());
                    return None;
                }
                stack.push(Frame {
                    id: operand.base.id,
                    phase: 0,
                    left: None,
                    site: None,
                });
            } else {
                result = self.typed_operand(operand, frame.site.as_ref()?);
                result.as_ref()?;
            }
        }
        result
    }

    pub(super) fn column_binding(&mut self, base: ValueBase) -> Option<BindingId> {
        if !matches!(base.kind, FIELD | BINDING | EXPRESSION) {
            self.fail(invalid(
                "c_plan_column_requires_binding",
                "A scalar result column requires a builder-bound value",
            ));
            return None;
        }
        let site = self.typed_site(base)?;
        let expression = self.typed_operand(
            OperandValue {
                base,
                literal: LiteralInput::Long(0),
            },
            &site,
        )?;
        Some(match expression {
            Expr::Operand(QueryOperand::Binding { binding }) => binding,
            expression => self.core.let_value(site, expression),
        })
    }

    fn typed_site(&mut self, base: ValueBase) -> Option<Site> {
        let owner = self.binding(base.owner)?;
        let Some(field) = self
            .fields
            .get(base.field as usize)
            .and_then(Option::as_ref)
        else {
            self.fail(id_unknown());
            return None;
        };
        Some(Site::new(owner, field.attribute().label().as_str()))
    }
    fn typed_operand(&mut self, input: OperandValue, site: &Site) -> Option<Expr> {
        match input.base.kind {
            LITERAL => match input.literal.parse(self.core.limits().codec) {
                Ok(value) => Some(Expr::Operand(self.core.bind_value(site.clone(), value))),
                Err(error) => {
                    self.fail(error);
                    None
                }
            },
            FIELD => {
                let owner = self.binding(input.base.owner)?;
                let field = self.field_id(input.base.field)?;
                let binding = self.core.field_value(owner, field.attribute().clone());
                Some(Expr::Operand(QueryOperand::Binding { binding }))
            }
            BINDING => self
                .binding(input.base.id)
                .map(|binding| Expr::Operand(QueryOperand::Binding { binding })),
            EXPRESSION => self.materialize_expression(input.base.id, site),
            POISON => {
                self.fail(constructor_error(input.base.id));
                None
            }
            NULL => {
                self.fail(id_unknown());
                None
            }
            _ => {
                self.fail(constant_invalid());
                None
            }
        }
    }
}

fn arithmetic(left: OperandValue, right: OperandValue, op: ArithOp, constructor: u32) -> ValueBase {
    let source = if !left.base.builder.is_null() {
        left.base
    } else {
        right.base
    };
    if source.builder.is_null() {
        return ValueBase::poison(constructor);
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: non-null builder-bearing values borrow their live, exclusively used builder.
        let builder = unsafe { &mut *source.builder.cast::<TypeBridgePlanBuilder>() };
        if builder.require_open().is_err() {
            return ValueBase::poison(constructor);
        }
        if (!left.base.builder.is_null() && left.base.builder != source.builder)
            || (!right.base.builder.is_null() && right.base.builder != source.builder)
        {
            builder.fail(invalid(
                "plan_context_missing",
                "the operands belong to different builders",
            ));
            return ValueBase::poison(constructor);
        }
        for operand in [&left, &right] {
            if operand.base.kind == EXPRESSION
                && !builder
                    .exprs
                    .get(operand.base.id as usize)
                    .is_some_and(Option::is_some)
            {
                builder.fail(id_unknown());
                return ValueBase::poison(constructor);
            }
        }
        let id = push(
            &mut builder.exprs,
            Some(NativeExpression::Typed(TypedArithmetic {
                source,
                left,
                right,
                op,
                binding: None,
            })),
        );
        ValueBase {
            kind: EXPRESSION,
            id,
            ..source
        }
    }))
    .unwrap_or_else(|_| ValueBase::poison(constructor))
}

/// Check the recording and binding carried by a generated nominal handle.
/// A foreign recording is refused before the binding id is inspected.
///
/// # Safety
/// `builder` is null or live and exclusively borrowed. `recording` is an
/// identity pointer only; this function never dereferences it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_check_handle(
    builder: *mut TypeBridgePlanBuilder,
    recording: *const TypeBridgePlanBuilder,
    binding: u32,
) -> TypeBridgeStatus {
    if builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: the live builder is exclusively borrowed by contract.
        let recorder = unsafe { &mut *builder };
        if !ptr::eq(builder.cast_const(), recording) {
            recorder.fail(invalid(
                "c_plan_builder_foreign_handle",
                "the generated handle belongs to another plan builder",
            ));
            return TypeBridgeStatus::InvalidArgument;
        } else if !recorder
            .binding(binding)
            .is_some_and(|binding| recorder.core.contains_binding(binding))
        {
            recorder.fail(id_unknown());
            return TypeBridgeStatus::InvalidArgument;
        }
        statement_status(recorder)
    })
}

fn field(builder: *mut TypeBridgePlanBuilder, owner: u32, field: u32) -> ValueBase {
    if builder.is_null() {
        return ValueBase::poison(9);
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the generated field accessor borrows its live, exclusively used builder.
        let recorder = unsafe { &mut *builder };
        let Some(binding) = recorder.binding(owner) else {
            return ValueBase::poison(9);
        };
        if !recorder.core.contains_binding(binding)
            || !recorder
                .fields
                .get(field as usize)
                .is_some_and(Option::is_some)
        {
            recorder.fail(id_unknown());
            return ValueBase::poison(9);
        }
        ValueBase {
            builder: builder.cast(),
            kind: FIELD,
            id: 0,
            owner,
            field,
        }
    }))
    .unwrap_or_else(|_| ValueBase::poison(9))
}

macro_rules! scalar {
    ($ty:ident, $host:ty, $literal:ident, $field:ident, $variant:ident) => {
        #[doc = concat!("A typed ", stringify!($host), " plan value borrowed from its builder, or one caller literal.")]
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct $ty { base: ValueBase, literal: $host }
        impl $ty {
            fn operand(self) -> OperandValue { OperandValue { base: self.base, literal: LiteralInput::$variant(self.literal) } }
        }
        #[doc = concat!("Construct one ", stringify!($host), " literal; validation occurs when applied to a builder.")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $literal(value: $host) -> $ty { $ty { base: ValueBase::LITERAL, literal: value } }
        /// Return a cached scalar field; its builder must remain live while the value is used.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $field(builder: *mut TypeBridgePlanBuilder, owner: u32, identity: u32) -> $ty {
            $ty { base: field(builder, owner, identity), literal: 0 as $host }
        }
    };
}
scalar!(
    TypeBridgePlanLong,
    i64,
    type_bridge_plan_long_literal,
    type_bridge_plan_long_field,
    Long
);
scalar!(
    TypeBridgePlanDouble,
    f64,
    type_bridge_plan_double_literal,
    type_bridge_plan_double_field,
    Double
);

macro_rules! arithmetic {
    ($ty:ident, $output:ident, $function:ident, $op:ident, $constructor:expr) => {
        /// Record arithmetic using the builder carried by either operand; two literals yield a poison value.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(left: $ty, right: $ty) -> $output {
            $output {
                base: arithmetic(left.operand(), right.operand(), ArithOp::$op, $constructor),
                literal: 0 as _,
            }
        }
    };
}
arithmetic!(
    TypeBridgePlanLong,
    TypeBridgePlanLong,
    type_bridge_plan_long_add,
    Add,
    1
);
arithmetic!(
    TypeBridgePlanLong,
    TypeBridgePlanLong,
    type_bridge_plan_long_sub,
    Sub,
    2
);
arithmetic!(
    TypeBridgePlanLong,
    TypeBridgePlanLong,
    type_bridge_plan_long_mul,
    Mul,
    3
);
arithmetic!(
    TypeBridgePlanLong,
    TypeBridgePlanDouble,
    type_bridge_plan_long_div,
    Div,
    4
);
arithmetic!(
    TypeBridgePlanDouble,
    TypeBridgePlanDouble,
    type_bridge_plan_double_add,
    Add,
    5
);
arithmetic!(
    TypeBridgePlanDouble,
    TypeBridgePlanDouble,
    type_bridge_plan_double_sub,
    Sub,
    6
);
arithmetic!(
    TypeBridgePlanDouble,
    TypeBridgePlanDouble,
    type_bridge_plan_double_mul,
    Mul,
    7
);
arithmetic!(
    TypeBridgePlanDouble,
    TypeBridgePlanDouble,
    type_bridge_plan_double_div,
    Div,
    8
);

/// A condition borrowed from its builder; a zero value has no condition.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanCondition {
    builder: *mut c_void,
    kind: u32,
    id: u32,
}
impl TypeBridgePlanCondition {
    fn poison(id: u32) -> Self {
        Self {
            builder: ptr::null_mut(),
            kind: POISON,
            id,
        }
    }
}
fn comparison(
    left: OperandValue,
    right: OperandValue,
    comparator: ValueComparator,
    constructor: u32,
) -> TypeBridgePlanCondition {
    let source = if !left.base.builder.is_null() {
        left.base
    } else {
        right.base
    };
    if source.builder.is_null() {
        return TypeBridgePlanCondition::poison(constructor);
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: operand values borrow the live, exclusively used builder.
        let builder = unsafe { &mut *source.builder.cast::<TypeBridgePlanBuilder>() };
        if builder.require_open().is_err() {
            return TypeBridgePlanCondition::poison(constructor);
        }
        if (!left.base.builder.is_null() && left.base.builder != source.builder)
            || (!right.base.builder.is_null() && right.base.builder != source.builder)
        {
            builder.fail(invalid(
                "plan_context_missing",
                "the operands belong to different builders",
            ));
            return TypeBridgePlanCondition::poison(constructor);
        }
        let Some(site) = builder.typed_site(source) else {
            return TypeBridgePlanCondition::poison(constructor);
        };
        let left_site = if left.base.builder.is_null() {
            site.clone()
        } else {
            let Some(site) = builder.typed_site(left.base) else {
                return TypeBridgePlanCondition::poison(constructor);
            };
            site
        };
        let right_site = if right.base.builder.is_null() {
            site
        } else {
            let Some(site) = builder.typed_site(right.base) else {
                return TypeBridgePlanCondition::poison(constructor);
            };
            site
        };
        let (Some(Expr::Operand(left)), Some(Expr::Operand(right))) = (
            builder.typed_operand(left, &left_site),
            builder.typed_operand(right, &right_site),
        ) else {
            builder.fail(operand_invalid());
            return TypeBridgePlanCondition::poison(constructor);
        };
        let id = push(&mut builder.conditions, Some((left, comparator, right)));
        TypeBridgePlanCondition {
            builder: source.builder,
            kind: BINDING,
            id,
        }
    }))
    .unwrap_or_else(|_| TypeBridgePlanCondition::poison(constructor))
}
macro_rules! comparisons {
    ($ty:ident; $($function:ident => $op:ident, $constructor:expr);* $(;)?) => {$(
        /// Record a comparison using a builder-bearing operand; literal-only comparisons yield poison.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(left: $ty, right: $ty) -> TypeBridgePlanCondition {
            comparison(left.operand(), right.operand(), ValueComparator::$op, $constructor)
        }
    )*};
}
comparisons!(TypeBridgePlanLong;
    type_bridge_plan_long_eq => Equal, 10; type_bridge_plan_long_ne => NotEqual, 11;
    type_bridge_plan_long_lt => Less, 12; type_bridge_plan_long_le => LessOrEqual, 13;
    type_bridge_plan_long_gt => Greater, 14; type_bridge_plan_long_ge => GreaterOrEqual, 15);
comparisons!(TypeBridgePlanDouble;
    type_bridge_plan_double_eq => Equal, 16; type_bridge_plan_double_ne => NotEqual, 17;
    type_bridge_plan_double_lt => Less, 18; type_bridge_plan_double_le => LessOrEqual, 19;
    type_bridge_plan_double_gt => Greater, 20; type_bridge_plan_double_ge => GreaterOrEqual, 21);

/// Apply a typed condition, reporting any earlier sticky constructor fault.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_require(
    builder: *mut TypeBridgePlanBuilder,
    condition: TypeBridgePlanCondition,
) -> TypeBridgeStatus {
    if builder.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: the caller supplies a live, exclusively used builder.
        let recorder = unsafe { &mut *builder };
        if let Err(status) = recorder.require_open() {
            return status;
        }
        if condition.kind == POISON {
            recorder.fail(constructor_error(condition.id));
        } else if condition.kind != BINDING || condition.builder != builder.cast() {
            recorder.fail(id_unknown());
        } else if let Some((left, comparator, right)) =
            recorder.get(|b| &b.conditions, condition.id)
        {
            recorder.core.compare(left, comparator, right);
        }
        statement_status(recorder)
    })
}

#[derive(Clone, Copy)]
enum Assignment {
    Set,
    Create,
    Key,
}

pub(super) fn statement_status(recorder: &TypeBridgePlanBuilder) -> TypeBridgeStatus {
    recorder
        .error
        .as_ref()
        .map(crate::execution_diagnostic::status_for)
        .unwrap_or(TypeBridgeStatus::Ok)
}

fn set(
    recorder: &mut TypeBridgePlanBuilder,
    pointer: *mut TypeBridgePlanBuilder,
    owner: u32,
    field: u32,
    value: OperandValue,
    assignment: Assignment,
) -> TypeBridgeStatus {
    if let Err(status) = recorder.require_open() {
        return status;
    }
    if value.base.kind == POISON {
        recorder.fail(constructor_error(value.base.id));
        return statement_status(recorder);
    }
    if value.base.kind == NULL && !matches!(assignment, Assignment::Key) {
        return statement_status(recorder);
    }
    if !value.base.builder.is_null() && value.base.builder != pointer.cast() {
        recorder.fail(id_unknown());
    } else if let (Some(owner), Some(field_id)) =
        (recorder.binding(owner), recorder.field_id(field))
    {
        if !recorder.core.contains_binding(owner) {
            recorder.fail(id_unknown());
        } else {
            let key = matches!(assignment, Assignment::Key).then(|| {
                recorder
                    .core
                    .field_value(owner, field_id.attribute().clone())
            });
            let site = Site::new(
                owner,
                if key.is_some() {
                    "key"
                } else {
                    field_id.attribute().label().as_str()
                },
            );
            if let Some(value) = recorder.typed_operand(value, &site) {
                let operand = match value {
                    Expr::Operand(value) => value,
                    expression => QueryOperand::Binding {
                        binding: recorder.core.let_value(site, expression),
                    },
                };
                match assignment {
                    Assignment::Set => {
                        recorder.core.update(owner, field_id, operand);
                    }
                    Assignment::Create => {
                        recorder.core.thing_field(owner, field_id, operand);
                    }
                    Assignment::Key => {
                        recorder.core.compare(
                            QueryOperand::Binding {
                                binding: key.expect("key assignment binds its field"),
                            },
                            ValueComparator::Equal,
                            operand,
                        );
                    }
                }
            }
        }
    }
    statement_status(recorder)
}
macro_rules! setter {
    ($ty:ident, $function:ident, $assignment:ident) => {
        /// Apply one generated scalar field or key assignment.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            builder: *mut TypeBridgePlanBuilder,
            owner: u32,
            field: u32,
            value: $ty,
        ) -> TypeBridgeStatus {
            if builder.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            guarded(|| {
                // SAFETY: the caller supplies a live, exclusively used builder.
                set(
                    unsafe { &mut *builder },
                    builder,
                    owner,
                    field,
                    value.operand(),
                    Assignment::$assignment,
                )
            })
        }
    };
}
setter!(TypeBridgePlanLong, type_bridge_plan_long_set, Set);
setter!(TypeBridgePlanDouble, type_bridge_plan_double_set, Set);

#[cfg(test)]
mod tests {
    use super::*;
    fn item(builder: &mut TypeBridgePlanBuilder) -> (u32, u32) {
        let model = builder.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("item".to_owned()));
        let field = builder.register_field(model, Some("stock".to_owned()));
        (builder.thing(model, PlanBuilder::exact), field)
    }
    #[test]
    fn generated_handles_check_recording_before_dense_binding_ids() {
        let mut first = TypeBridgePlanBuilder::new();
        let mut second = TypeBridgePlanBuilder::new();
        let (binding, _) = item(&mut first);
        let (same_id, _) = item(&mut second);
        assert_eq!(binding, same_id);
        // SAFETY: both builders are live, while recording is compared without dereference.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_check_handle(&mut first, &first, binding),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_plan_builder_check_handle(&mut second, &first, binding),
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert_eq!(
            second.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_foreign_handle"
        );
        assert!(first.error.is_none());
        // SAFETY: first is live; an unknown id is refused without recording a new binding.
        assert_eq!(
            unsafe { type_bridge_plan_builder_check_handle(&mut first, &first, u32::MAX) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            first.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_id_unknown"
        );
    }

    #[test]
    fn literal_only_subtraction_is_a_static_poison_and_preserves_the_constructor_diagnostic() {
        let mut builder = TypeBridgePlanBuilder::new();
        // SAFETY: two literal values borrow no builder.
        let poison = unsafe {
            type_bridge_plan_long_sub(
                type_bridge_plan_long_literal(2),
                type_bridge_plan_long_literal(1),
            )
        };
        assert!(poison.base.builder.is_null());
        assert_eq!(poison.base.kind, POISON);
        assert!(builder.exprs.is_empty());
        // SAFETY: builder is live and exclusively borrowed for the statement.
        assert_eq!(
            unsafe { type_bridge_plan_long_set(&mut builder, 0, 0, poison) },
            TypeBridgeStatus::InvalidArgument
        );
        let error = builder.error.as_ref().unwrap();
        assert_eq!(error.code().as_str(), "c_plan_constructor_missing_builder");
        assert_eq!(
            error.message().as_str(),
            "tb_long_sub: no operand carried a builder"
        );
        // A later statement keeps the first error, including its constructor.
        let (owner, field) = item(&mut builder);
        assert_eq!(
            unsafe {
                type_bridge_plan_long_set(
                    &mut builder,
                    owner,
                    field,
                    type_bridge_plan_long_literal(2),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            builder.error.as_ref().unwrap().message().as_str(),
            "tb_long_sub: no operand carried a builder"
        );
    }
    #[test]
    fn cached_fields_and_typed_arithmetic_forward_to_the_core() {
        let mut builder = TypeBridgePlanBuilder::new();
        let (owner, field) = item(&mut builder);
        // SAFETY: every value borrows this live builder until the statement finishes.
        unsafe {
            let first = type_bridge_plan_long_field(&mut builder, owner, field);
            let second = type_bridge_plan_long_field(&mut builder, owner, field);
            assert_eq!(first.base.kind, FIELD);
            assert_eq!(second.base.kind, FIELD);
            assert_eq!(first.base.owner, second.base.owner);
            assert_eq!(first.base.field, second.base.field);
            assert!(!builder.core.contains_binding(BindingId::new(1).unwrap()));
            let condition = type_bridge_plan_long_gt(first, type_bridge_plan_long_literal(0));
            assert_eq!(
                type_bridge_plan_require(&mut builder, condition),
                TypeBridgeStatus::Ok
            );
            assert!(builder.core.contains_binding(BindingId::new(1).unwrap()));
            assert!(!builder.core.contains_binding(BindingId::new(2).unwrap()));
            let expression = type_bridge_plan_long_sub(second, type_bridge_plan_long_literal(1));
            assert_eq!(
                type_bridge_plan_long_set(&mut builder, owner, field, expression),
                TypeBridgeStatus::Ok
            );
        }
        assert!(builder.error.is_none());
        assert_eq!(builder.exprs.len(), 1);
        assert_eq!(builder.conditions.len(), 1);
    }
    #[test]
    fn foreign_operands_and_unknown_fields_fail_before_the_next_statement() {
        let mut first = TypeBridgePlanBuilder::new();
        let mut second = TypeBridgePlanBuilder::new();
        let (owner, field) = item(&mut first);
        let (other_owner, other_field) = item(&mut second);
        // SAFETY: both independently owned builders remain live for these calls.
        unsafe {
            let left = type_bridge_plan_long_field(&mut first, owner, field);
            let right = type_bridge_plan_long_field(&mut second, other_owner, other_field);
            let _ = type_bridge_plan_long_sub(left, right);
            assert_eq!(
                type_bridge_plan_long_set(
                    &mut first,
                    owner,
                    field,
                    type_bridge_plan_long_literal(1)
                ),
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert_eq!(
            first.error.as_ref().unwrap().code().as_str(),
            "plan_context_missing"
        );
        assert!(second.error.is_none());
        assert!(first.exprs.is_empty());
        let mut invalid = TypeBridgePlanBuilder::new();
        let (_, field) = item(&mut invalid);
        // SAFETY: the builder is live; the unknown binding is an invalid content value.
        unsafe {
            type_bridge_plan_long_field(&mut invalid, 123, field);
        }
        assert_eq!(
            invalid.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_id_unknown"
        );
    }
    #[test]
    fn selected_string_limits_apply_to_raw_and_typed_literals() {
        use type_bridge_contract::limits::QueryLimits;
        let text = "x".repeat(MAX_CANONICAL_STRING_BYTES + 1);
        let view = TypeBridgeByteView {
            data: text.as_ptr(),
            length: text.len(),
        };
        let mut selected = QueryLimits::CANONICAL;
        selected.codec.max_string_bytes = text.len();
        selected.codec.max_bytes = selected.codec.max_bytes.max(text.len());
        for (limits, expected) in [
            (QueryLimits::CANONICAL, TypeBridgeStatus::InvalidArgument),
            (selected, TypeBridgeStatus::Ok),
        ] {
            let mut raw = TypeBridgePlanBuilder::new();
            raw.core = PlanBuilder::with_limits(limits);
            let mut value = u32::MAX;
            // SAFETY: the view and both outputs stay live for their calls.
            unsafe {
                assert_eq!(
                    type_bridge_plan_builder_value_string(&mut raw, view, &mut value),
                    TypeBridgeStatus::Ok
                );
            }
            assert_eq!(raw.error.is_none(), expected == TypeBridgeStatus::Ok);
            let mut typed = TypeBridgePlanBuilder::new();
            typed.core = PlanBuilder::with_limits(limits);
            let (owner, field) = item(&mut typed);
            // SAFETY: this statement consumes the live borrowed UTF-8 literal.
            unsafe {
                assert_eq!(
                    type_bridge_plan_string_set(
                        &mut typed,
                        owner,
                        field,
                        type_bridge_plan_string_literal(view)
                    ),
                    expected
                );
            }
        }
    }

    #[test]
    fn text_and_boolean_literals_validate_only_when_consumed() {
        let mut builder = TypeBridgePlanBuilder::new();
        let (owner, field) = item(&mut builder);
        let bad = [0xff_u8];
        let literal = type_bridge_plan_string_literal(TypeBridgeByteView {
            data: bad.as_ptr(),
            length: bad.len(),
        });
        assert!(builder.error.is_none());
        // SAFETY: both builder and byte view remain live for the consuming statement.
        assert_eq!(
            unsafe { type_bridge_plan_string_set(&mut builder, owner, field, literal) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            builder.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_value_invalid"
        );
        let mut other = TypeBridgePlanBuilder::new();
        let (owner, field) = item(&mut other);
        // SAFETY: the live builder validates the invalid Boolean content without reading any foreign pointer.
        assert_eq!(
            unsafe {
                type_bridge_plan_bool_set(
                    &mut other,
                    owner,
                    field,
                    type_bridge_plan_bool_literal(2),
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            other.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_value_invalid"
        );
    }
    #[test]
    fn literal_only_string_comparison_keeps_its_public_constructor_name() {
        let mut builder = TypeBridgePlanBuilder::new();
        let bytes = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        // SAFETY: empty literals borrow no bytes and no builder.
        let poison = unsafe {
            type_bridge_plan_string_gt(
                type_bridge_plan_string_literal(bytes),
                type_bridge_plan_string_literal(bytes),
            )
        };
        // SAFETY: the target builder is live for this statement.
        assert_eq!(
            unsafe { type_bridge_plan_require(&mut builder, poison) },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            builder.error.as_ref().unwrap().message().as_str(),
            "tb_string_gt: no operand carried a builder"
        );
    }
    #[test]
    fn zero_values_omit_sets_and_invalid_double_literals_are_sticky() {
        let mut builder = TypeBridgePlanBuilder::new();
        let (owner, field) = item(&mut builder);
        let omitted = TypeBridgePlanLong {
            base: ValueBase {
                kind: NULL,
                ..ValueBase::LITERAL
            },
            literal: 0,
        };
        // SAFETY: the builder and arguments remain live throughout each statement.
        unsafe {
            assert_eq!(
                type_bridge_plan_long_set(&mut builder, owner, field, omitted),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_plan_double_set(
                    &mut builder,
                    owner,
                    field,
                    type_bridge_plan_double_literal(f64::NAN)
                ),
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert_eq!(
            builder.error.as_ref().unwrap().code().as_str(),
            "c_plan_builder_value_invalid"
        );
    }
    #[test]
    fn string_conditions_equal_the_core_builder_comparisons() {
        let package = crate::entity_crud::tests::package("string_ops");
        let authority = &package.state()._authority;
        let functions = [
            (
                type_bridge_plan_string_contains
                    as unsafe extern "C" fn(
                        TypeBridgePlanString,
                        TypeBridgePlanString,
                    ) -> TypeBridgePlanCondition,
                ValueComparator::Contains,
            ),
            (
                type_bridge_plan_string_starts_with,
                ValueComparator::StartsWith,
            ),
            (type_bridge_plan_string_ends_with, ValueComparator::EndsWith),
            (type_bridge_plan_string_like, ValueComparator::Like),
        ];
        for (constructor, comparator) in functions {
            let mut builder = TypeBridgePlanBuilder::new();
            let model = builder.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("person".into()));
            let field = builder.register_field(model, Some("identifier".into()));
            let owner = builder.thing(model, PlanBuilder::exact);
            // SAFETY: live recording and readable literal remain valid through consumption.
            let condition = unsafe {
                constructor(
                    type_bridge_plan_string_field(&mut builder, owner, field),
                    type_bridge_plan_string_literal(TypeBridgeByteView {
                        data: b"ada".as_ptr(),
                        length: 3,
                    }),
                )
            };
            assert_eq!(
                unsafe { type_bridge_plan_require(&mut builder, condition) },
                TypeBridgeStatus::Ok
            );
            builder.core.count();
            let mut raw = PlanBuilder::new();
            let owner = raw.exact(TypeId::new(TypeKind::Entity, "person").unwrap());
            let field = raw.field_value(owner, AttributeId::new("identifier").unwrap());
            let right = raw.bind_value(
                Site::new(owner, "identifier"),
                CanonicalValue::String(CanonicalString::new("ada").unwrap()),
            );
            raw.compare(QueryOperand::Binding { binding: field }, comparator, right)
                .count();
            let build = |core: &PlanBuilder| {
                core.build(
                    authority.declared_schema(),
                    authority.managed_state().managed_semantic_schema().clone(),
                )
                .unwrap()
                .canonical_bytes()
                .unwrap()
            };
            assert_eq!(build(&builder.core), build(&raw));
        }
    }
}

scalar!(
    TypeBridgePlanBoolean,
    u8,
    type_bridge_plan_bool_literal,
    type_bridge_plan_bool_field,
    Boolean
);

macro_rules! text_scalar {
    ($ty:ident, $literal:ident, $field:ident, $parse:expr) => {
        /// A typed textual scalar, borrowing a builder or literal bytes until consumed.
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct $ty {
            base: ValueBase,
            literal: TypeBridgeByteView,
        }
        impl $ty {
            fn operand(self) -> OperandValue {
                OperandValue {
                    base: self.base,
                    literal: LiteralInput::Text {
                        view: self.literal,
                        parse: $parse,
                    },
                }
            }
        }
        /// Borrow canonical literal bytes, which must remain readable until consumed by a statement or condition.
        #[unsafe(no_mangle)]
        pub extern "C" fn $literal(value: TypeBridgeByteView) -> $ty {
            $ty {
                base: ValueBase::LITERAL,
                literal: value,
            }
        }
        /// Return a cached scalar field borrowed from a live, exclusively used builder.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $field(
            builder: *mut TypeBridgePlanBuilder,
            owner: u32,
            identity: u32,
        ) -> $ty {
            $ty {
                base: field(builder, owner, identity),
                literal: TypeBridgeByteView {
                    data: ptr::null(),
                    length: 0,
                },
            }
        }
    };
}
text_scalar!(
    TypeBridgePlanString,
    type_bridge_plan_string_literal,
    type_bridge_plan_string_field,
    |text, limits| CanonicalString::new_with_limits(text, limits)
        .ok()
        .map(CanonicalValue::String)
);
text_scalar!(
    TypeBridgePlanDate,
    type_bridge_plan_date_literal,
    type_bridge_plan_date_field,
    |text, _limits| CanonicalDate::from_str(text).ok().map(CanonicalValue::Date)
);
text_scalar!(
    TypeBridgePlanDateTime,
    type_bridge_plan_datetime_literal,
    type_bridge_plan_datetime_field,
    |text, _limits| CanonicalDateTime::from_str(text)
        .ok()
        .map(CanonicalValue::DateTime)
);
text_scalar!(
    TypeBridgePlanDateTimeTz,
    type_bridge_plan_datetime_tz_literal,
    type_bridge_plan_datetime_tz_field,
    |text, _limits| parse_provider_datetime_tz(text)
        .ok()
        .map(CanonicalValue::DateTimeTz)
);
text_scalar!(
    TypeBridgePlanDecimal,
    type_bridge_plan_decimal_literal,
    type_bridge_plan_decimal_field,
    |text, _limits| DecimalValue::new(text)
        .ok()
        .filter(|value| value.as_str() == text)
        .map(CanonicalValue::Decimal)
);
text_scalar!(
    TypeBridgePlanDuration,
    type_bridge_plan_duration_literal,
    type_bridge_plan_duration_field,
    |text, _limits| CanonicalDuration::from_str(text)
        .ok()
        .map(CanonicalValue::Duration)
);

comparisons!(TypeBridgePlanBoolean;
    type_bridge_plan_bool_eq => Equal, 22;
    type_bridge_plan_bool_ne => NotEqual, 23;
    type_bridge_plan_bool_lt => Less, 24;
    type_bridge_plan_bool_le => LessOrEqual, 25;
    type_bridge_plan_bool_gt => Greater, 26;
    type_bridge_plan_bool_ge => GreaterOrEqual, 27;
);
setter!(TypeBridgePlanBoolean, type_bridge_plan_bool_set, Set);

comparisons!(TypeBridgePlanString;
    type_bridge_plan_string_eq => Equal, 28;
    type_bridge_plan_string_ne => NotEqual, 29;
    type_bridge_plan_string_lt => Less, 30;
    type_bridge_plan_string_le => LessOrEqual, 31;
    type_bridge_plan_string_gt => Greater, 32;
    type_bridge_plan_string_ge => GreaterOrEqual, 33;
);
comparisons!(TypeBridgePlanString;
    type_bridge_plan_string_contains => Contains, 64;
    type_bridge_plan_string_starts_with => StartsWith, 65;
    type_bridge_plan_string_ends_with => EndsWith, 66;
    type_bridge_plan_string_like => Like, 67;
);
setter!(TypeBridgePlanString, type_bridge_plan_string_set, Set);

comparisons!(TypeBridgePlanDate;
    type_bridge_plan_date_eq => Equal, 34;
    type_bridge_plan_date_ne => NotEqual, 35;
    type_bridge_plan_date_lt => Less, 36;
    type_bridge_plan_date_le => LessOrEqual, 37;
    type_bridge_plan_date_gt => Greater, 38;
    type_bridge_plan_date_ge => GreaterOrEqual, 39;
);
setter!(TypeBridgePlanDate, type_bridge_plan_date_set, Set);

comparisons!(TypeBridgePlanDateTime;
    type_bridge_plan_datetime_eq => Equal, 40;
    type_bridge_plan_datetime_ne => NotEqual, 41;
    type_bridge_plan_datetime_lt => Less, 42;
    type_bridge_plan_datetime_le => LessOrEqual, 43;
    type_bridge_plan_datetime_gt => Greater, 44;
    type_bridge_plan_datetime_ge => GreaterOrEqual, 45;
);
setter!(TypeBridgePlanDateTime, type_bridge_plan_datetime_set, Set);

comparisons!(TypeBridgePlanDateTimeTz;
    type_bridge_plan_datetime_tz_eq => Equal, 46;
    type_bridge_plan_datetime_tz_ne => NotEqual, 47;
    type_bridge_plan_datetime_tz_lt => Less, 48;
    type_bridge_plan_datetime_tz_le => LessOrEqual, 49;
    type_bridge_plan_datetime_tz_gt => Greater, 50;
    type_bridge_plan_datetime_tz_ge => GreaterOrEqual, 51;
);
setter!(
    TypeBridgePlanDateTimeTz,
    type_bridge_plan_datetime_tz_set,
    Set
);

comparisons!(TypeBridgePlanDecimal;
    type_bridge_plan_decimal_eq => Equal, 52;
    type_bridge_plan_decimal_ne => NotEqual, 53;
    type_bridge_plan_decimal_lt => Less, 54;
    type_bridge_plan_decimal_le => LessOrEqual, 55;
    type_bridge_plan_decimal_gt => Greater, 56;
    type_bridge_plan_decimal_ge => GreaterOrEqual, 57;
);
setter!(TypeBridgePlanDecimal, type_bridge_plan_decimal_set, Set);

comparisons!(TypeBridgePlanDuration;
    type_bridge_plan_duration_eq => Equal, 58;
    type_bridge_plan_duration_ne => NotEqual, 59;
    type_bridge_plan_duration_lt => Less, 60;
    type_bridge_plan_duration_le => LessOrEqual, 61;
    type_bridge_plan_duration_gt => Greater, 62;
    type_bridge_plan_duration_ge => GreaterOrEqual, 63;
);
setter!(TypeBridgePlanDuration, type_bridge_plan_duration_set, Set);

setter!(
    TypeBridgePlanLong,
    type_bridge_plan_long_create_field,
    Create
);
setter!(TypeBridgePlanLong, type_bridge_plan_long_key, Key);

setter!(
    TypeBridgePlanDouble,
    type_bridge_plan_double_create_field,
    Create
);
setter!(TypeBridgePlanDouble, type_bridge_plan_double_key, Key);

setter!(
    TypeBridgePlanBoolean,
    type_bridge_plan_bool_create_field,
    Create
);
setter!(TypeBridgePlanBoolean, type_bridge_plan_bool_key, Key);

setter!(
    TypeBridgePlanString,
    type_bridge_plan_string_create_field,
    Create
);
setter!(TypeBridgePlanString, type_bridge_plan_string_key, Key);

setter!(
    TypeBridgePlanDate,
    type_bridge_plan_date_create_field,
    Create
);
setter!(TypeBridgePlanDate, type_bridge_plan_date_key, Key);

setter!(
    TypeBridgePlanDateTime,
    type_bridge_plan_datetime_create_field,
    Create
);
setter!(TypeBridgePlanDateTime, type_bridge_plan_datetime_key, Key);

setter!(
    TypeBridgePlanDateTimeTz,
    type_bridge_plan_datetime_tz_create_field,
    Create
);
setter!(
    TypeBridgePlanDateTimeTz,
    type_bridge_plan_datetime_tz_key,
    Key
);

setter!(
    TypeBridgePlanDecimal,
    type_bridge_plan_decimal_create_field,
    Create
);
setter!(TypeBridgePlanDecimal, type_bridge_plan_decimal_key, Key);

setter!(
    TypeBridgePlanDuration,
    type_bridge_plan_duration_create_field,
    Create
);
setter!(TypeBridgePlanDuration, type_bridge_plan_duration_key, Key);

macro_rules! column {
    ($ty:ident, $function:ident, $domain:expr) => {
        /// Hold a typed scalar result column; the recording consumes it lazily.
        #[unsafe(no_mangle)]
        pub extern "C" fn $function(value: $ty) -> super::columns::TypeBridgePlanColumn {
            super::columns::TypeBridgePlanColumn {
                value: value.base,
                domain: $domain,
                model_ordinal: u32::MAX,
            }
        }
    };
}
column!(TypeBridgePlanString, type_bridge_plan_string_column, 1);
column!(TypeBridgePlanLong, type_bridge_plan_long_column, 2);
column!(TypeBridgePlanDouble, type_bridge_plan_double_column, 3);
column!(TypeBridgePlanBoolean, type_bridge_plan_bool_column, 4);
column!(TypeBridgePlanDate, type_bridge_plan_date_column, 5);
column!(TypeBridgePlanDateTime, type_bridge_plan_datetime_column, 6);
column!(
    TypeBridgePlanDateTimeTz,
    type_bridge_plan_datetime_tz_column,
    7
);
column!(TypeBridgePlanDecimal, type_bridge_plan_decimal_column, 8);
column!(TypeBridgePlanDuration, type_bridge_plan_duration_column, 9);
