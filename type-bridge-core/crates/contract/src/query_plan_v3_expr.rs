//! Closed V3 expression syntax and TypeDB 3.13.6 scalar inference.

use super::*;

/// Numeric binary operations supported by TypeDB expressions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithOp {
    /// Numeric addition.
    Add,
    /// Numeric subtraction.
    Sub,
    /// Numeric multiplication.
    Mul,
    /// Floating division, including integer operands.
    Div,
    /// Integer remainder for two integers, floating remainder otherwise.
    Mod,
    /// Floating exponentiation.
    Pow,
}

/// Closed scalar builtin functions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Builtin {
    /// Numeric absolute value.
    Abs,
    /// Ceiling of decimal/double to integer.
    Ceil,
    /// Floor of decimal/double to integer.
    Floor,
    /// Round decimal/double to integer.
    Round,
    /// Minimum of two operands of the same numeric domain.
    Min,
    /// Maximum of two operands of the same numeric domain.
    Max,
    /// String length as an integer; lowers to TypeQL len.
    Length,
}

/// A value expression. Values remain typed operands, never TypeQL fragments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Expr {
    /// A scalar binding, literal or declared input.
    Operand(QueryOperand),
    /// Binary numeric expression.
    Arith {
        /// Closed numeric operator.
        op: ArithOp,
        /// Left expression.
        lhs: Box<Expr>,
        /// Right expression.
        rhs: Box<Expr>,
    },
    /// Closed builtin call.
    Call {
        /// Builtin identity.
        func: Builtin,
        /// Exactly one argument, or two for min/max.
        args: Vec<Expr>,
    },
}

impl Expr {
    /// Infer the scalar result from present scalar bindings and declared inputs.
    ///
    /// This uses the same bounded inference as trusted plan construction.
    pub fn value_type(
        &self,
        bindings: &BTreeMap<BindingId, ValueTypeTag>,
        inputs: &[InputColumn],
    ) -> Result<ValueTypeTag, Diagnostic> {
        self.infer(bindings, inputs, QueryLimits::CANONICAL, 1, &mut 0)
    }

    pub(super) fn infer(
        &self,
        bindings: &BTreeMap<BindingId, ValueTypeTag>,
        inputs: &[InputColumn],
        limits: QueryLimits,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<ValueTypeTag, Diagnostic> {
        *nodes = nodes.saturating_add(1);
        if depth > limits.structural.predicate_depth || *nodes > limits.structural.predicate_nodes {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_expression_limit",
                "expression exceeds the structural ceiling",
            ));
        }
        match self {
            Self::Operand(operand) => operand_type(operand, bindings, inputs),
            Self::Arith { op, lhs, rhs } => {
                let left = lhs.infer(bindings, inputs, limits, depth + 1, nodes)?;
                let right = rhs.infer(bindings, inputs, limits, depth + 1, nodes)?;
                if !numeric(left) || !numeric(right) {
                    return Err(invalid(
                        "query_plan_expression_type",
                        "arithmetic requires numeric operands",
                    ));
                }
                // TypeDB tag3.13.6 operation_resolution.rs and operators.rs:
                // integer division/power and decimal non-ring operations use double.
                Ok(match op {
                    ArithOp::Div | ArithOp::Pow => ValueTypeTag::Double,
                    ArithOp::Mod if left == ValueTypeTag::Long && right == ValueTypeTag::Long => {
                        ValueTypeTag::Long
                    }
                    ArithOp::Mod => ValueTypeTag::Double,
                    _ if left == ValueTypeTag::Double || right == ValueTypeTag::Double => {
                        ValueTypeTag::Double
                    }
                    _ if left == ValueTypeTag::Decimal || right == ValueTypeTag::Decimal => {
                        ValueTypeTag::Decimal
                    }
                    _ => ValueTypeTag::Long,
                })
            }
            Self::Call { func, args } => {
                let arity = if matches!(func, Builtin::Min | Builtin::Max) {
                    2
                } else {
                    1
                };
                if args.len() != arity {
                    return Err(invalid(
                        "query_plan_expression_arity",
                        "builtin has the wrong argument count",
                    ));
                }
                let first = args[0].infer(bindings, inputs, limits, depth + 1, nodes)?;
                match func {
                    Builtin::Length if first == ValueTypeTag::String => Ok(ValueTypeTag::Long),
                    Builtin::Abs if numeric(first) => Ok(first),
                    Builtin::Ceil | Builtin::Floor | Builtin::Round
                        if matches!(first, ValueTypeTag::Decimal | ValueTypeTag::Double) =>
                    {
                        Ok(ValueTypeTag::Long)
                    }
                    Builtin::Min | Builtin::Max if numeric(first) => {
                        let second = args[1].infer(bindings, inputs, limits, depth + 1, nodes)?;
                        if first != second {
                            return Err(invalid(
                                "query_plan_expression_type",
                                "min and max require matching numeric domains",
                            ));
                        }
                        Ok(first)
                    }
                    _ => Err(invalid(
                        "query_plan_expression_type",
                        "builtin does not accept this scalar domain",
                    )),
                }
            }
        }
    }

    pub(super) fn capabilities(&self, capabilities: &mut CapabilitySet) -> Result<(), Diagnostic> {
        let capability = match self {
            Self::Operand(operand) => {
                if matches!(
                    operand,
                    QueryOperand::Literal { .. } | QueryOperand::Input { .. }
                ) {
                    insert_capability(capabilities, "query.input.given-rows")?;
                }
                return Ok(());
            }
            Self::Arith { op, lhs, rhs } => {
                lhs.capabilities(capabilities)?;
                rhs.capabilities(capabilities)?;
                match op {
                    ArithOp::Add => "query.expr.add",
                    ArithOp::Sub => "query.expr.sub",
                    ArithOp::Mul => "query.expr.mul",
                    ArithOp::Div => "query.expr.div",
                    ArithOp::Mod => "query.expr.mod",
                    ArithOp::Pow => "query.expr.pow",
                }
            }
            Self::Call { func, args } => {
                for argument in args {
                    argument.capabilities(capabilities)?;
                }
                match func {
                    Builtin::Abs => "query.expr.abs",
                    Builtin::Ceil => "query.expr.ceil",
                    Builtin::Floor => "query.expr.floor",
                    Builtin::Round => "query.expr.round",
                    Builtin::Min => "query.expr.min",
                    Builtin::Max => "query.expr.max",
                    Builtin::Length => "query.expr.length",
                }
            }
        };
        insert_capability(capabilities, capability)
    }
}

pub(super) const fn numeric(value_type: ValueTypeTag) -> bool {
    matches!(
        value_type,
        ValueTypeTag::Long | ValueTypeTag::Decimal | ValueTypeTag::Double
    )
}

pub(super) fn operand_type(
    operand: &QueryOperand,
    bindings: &BTreeMap<BindingId, ValueTypeTag>,
    inputs: &[InputColumn],
) -> Result<ValueTypeTag, Diagnostic> {
    match operand {
        QueryOperand::Literal { value } => Ok(value.value_type()),
        QueryOperand::Binding { binding } => bindings.get(binding).copied().ok_or_else(|| {
            invalid(
                "query_plan_expression_binding",
                "expression requires a present, already typed scalar binding",
            )
        }),
        QueryOperand::Input { column } => {
            let input = inputs
                .get(usize::from(column.get()))
                .filter(|input| input.id() == *column)
                .ok_or_else(|| {
                    invalid(
                        "query_plan_unknown_input",
                        "expression references an undeclared input",
                    )
                })?;
            if input.optional() {
                return Err(invalid(
                    "query_plan_expression_optional_input",
                    "expression requires a present input",
                ));
            }
            Ok(input.value_type())
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum ExprWire {
    Operand(QueryOperandWire),
    Arith {
        op: ArithOp,
        lhs: Box<ExprWire>,
        rhs: Box<ExprWire>,
    },
    Call {
        func: Builtin,
        args: Vec<ExprWire>,
    },
}
impl ExprWire {
    pub(super) fn rebuild(self) -> Result<Expr, Diagnostic> {
        Ok(match self {
            Self::Operand(value) => Expr::Operand(value.rebuild()?),
            Self::Arith { op, lhs, rhs } => Expr::Arith {
                op,
                lhs: Box::new(lhs.rebuild()?),
                rhs: Box::new(rhs.rebuild()?),
            },
            Self::Call { func, args } => Expr::Call {
                func,
                args: args
                    .into_iter()
                    .map(Self::rebuild)
                    .collect::<Result<_, _>>()?,
            },
        })
    }
}
