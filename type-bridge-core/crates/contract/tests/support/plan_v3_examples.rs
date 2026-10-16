use crate::support::*;
use std::collections::BTreeMap;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::v3::{
    ArithOp, Builtin, DeleteItem, Expr, InsertThing, Operation, Pattern, QueryPlanV3, Set, Stage,
    WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryPattern,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};

fn operand(binding: u16) -> QueryOperand {
    QueryOperand::Binding {
        binding: id(binding),
    }
}
fn input(column: u16) -> QueryOperand {
    QueryOperand::Input {
        column: InputColumnId::new(column),
    }
}
fn owns(owner: &str, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(
        if owner == "purchase" {
            TypeId::new(TypeKind::Relation, owner).unwrap()
        } else {
            entity(owner)
        },
        AttributeId::new(attribute).unwrap(),
    )
    .unwrap()
}
fn has(owner: u16, attribute: u16, name: &str) -> Pattern {
    QueryPattern::Has {
        owner: id(owner),
        attribute: id(attribute),
        attribute_id: AttributeId::new(name).unwrap(),
    }
    .into()
}
fn column(n: u16, name: &str, domain: ValueTypeTag) -> InputColumn {
    InputColumn::new(
        InputColumnId::new(n),
        type_bridge_contract::migration_assertion::QueryVariable::new(name).unwrap(),
        domain,
        false,
    )
}
fn sub(left: QueryOperand, right: QueryOperand) -> Expr {
    Expr::Arith {
        op: ArithOp::Sub,
        lhs: Box::new(Expr::Operand(left)),
        rhs: Box::new(Expr::Operand(right)),
    }
}
fn text(value: &str) -> CanonicalValue {
    CanonicalValue::String(CanonicalString::new(value).unwrap())
}

pub fn purchase() -> QueryPlanV3 {
    let patterns = vec![
        pat(0, entity("item")),
        pat(1, entity("account")),
        has(0, 2, "stock"),
        has(1, 3, "balance"),
        has(0, 4, "price"),
        has(0, 8, "item-id"),
        has(1, 9, "id"),
        QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left: operand(8),
            right: input(1),
        }
        .into(),
        QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left: operand(9),
            right: input(2),
        }
        .into(),
        QueryPattern::Value {
            comparator: ValueComparator::Greater,
            left: operand(2),
            right: literal(0),
        }
        .into(),
        QueryPattern::Value {
            comparator: ValueComparator::GreaterOrEqual,
            left: operand(3),
            right: operand(4),
        }
        .into(),
        Pattern::Let {
            binding: id(5),
            expr: sub(operand(2), literal(1)),
        },
        Pattern::Let {
            binding: id(6),
            expr: sub(operand(3), operand(4)),
        },
    ];
    let mut plan = spec(
        vec![
            Stage::Match { patterns },
            Stage::Update {
                sets: vec![
                    Set::Field {
                        owner: id(0),
                        field: owns("item", "stock"),
                        operand: operand(5),
                    },
                    Set::Field {
                        owner: id(1),
                        field: owns("account", "balance"),
                        operand: operand(6),
                    },
                ],
            },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: id(7),
                    type_id: TypeId::new(TypeKind::Relation, "purchase").unwrap(),
                    fields: vec![(owns("purchase", "purchase-id"), input(0))],
                    players: vec![
                        (RoleId::new("purchase", "buyer").unwrap(), id(1)),
                        (RoleId::new("purchase", "item").unwrap(), id(0)),
                    ],
                }],
            },
        ],
        vec![7],
    );
    plan.bindings = [
        "item",
        "buyer",
        "stock",
        "balance",
        "price",
        "next_stock",
        "next_balance",
        "purchase",
        "item_key",
        "buyer_key",
    ]
    .into_iter()
    .enumerate()
    .map(|(n, name)| binding(n as u16, name))
    .collect();
    plan.inputs = vec![
        column(0, "new_purchase_id", ValueTypeTag::String),
        column(1, "expected_item_id", ValueTypeTag::String),
        column(2, "expected_buyer_id", ValueTypeTag::String),
    ];
    plan.given_rows = Some(vec![InputRow::new(vec![
        Some(text("purchase-1")),
        Some(text("item-1")),
        Some(text("buyer-1")),
    ])]);
    QueryPlanV3::new(plan, &schema()).unwrap()
}

pub fn touch() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("doctor")), has(0, 1, "on-call")],
                },
                Stage::Update {
                    sets: vec![Set::Field {
                        owner: id(0),
                        field: owns("doctor", "on-call"),
                        operand: operand(1),
                    }],
                },
            ],
            vec![0],
        ),
        &schema(),
    )
    .unwrap()
}

pub fn relation_assign() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![
                        pat(0, entity("account")),
                        pat(1, relation()),
                        pat(2, entity("account")),
                    ],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Links {
                        relation: id(1),
                        role: role(),
                        player: id(0),
                    }],
                },
                Stage::Update {
                    sets: vec![Set::Link {
                        relation: id(1),
                        role: role(),
                        player: id(2),
                    }],
                },
            ],
            vec![1],
        ),
        &schema(),
    )
    .unwrap()
}

pub fn batch() -> QueryPlanV3 {
    let mut plan = spec(
        vec![Stage::Insert {
            things: vec![InsertThing {
                binding: id(0),
                type_id: entity("account"),
                fields: vec![(field("id"), input(0)), (field("score"), input(1))],
                players: vec![],
            }],
        }],
        vec![],
    );
    plan.inputs = vec![
        column(0, "new_id", ValueTypeTag::String),
        column(1, "new_score", ValueTypeTag::Long),
    ];
    plan.given_rows = Some(vec![
        InputRow::new(vec![Some(text("account-1")), Some(CanonicalValue::Long(1))]),
        InputRow::new(vec![Some(text("account-2")), Some(CanonicalValue::Long(2))]),
    ]);
    QueryPlanV3::new(plan, &schema()).unwrap()
}

pub fn expression_plan(expr: Expr) -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            vec![Stage::Match {
                patterns: vec![Pattern::Let {
                    binding: id(0),
                    expr,
                }],
            }],
            vec![0],
        ),
        &schema(),
    )
    .unwrap()
}

pub fn decimal() -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::Decimal(DecimalValue::new("1.5").unwrap()),
    }
}
pub fn double() -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::Double(CanonicalDouble::new(1.5).unwrap()),
    }
}

/// Write a thing and fetch its hydration document in the same pipeline.
pub fn fetch_written(relation_players: bool) -> QueryPlanV3 {
    use type_bridge_contract::query_plan::v3::{DocumentRole, ThingDocument};
    let mut pipeline = Vec::new();
    let document = if relation_players {
        pipeline.push(Stage::Match {
            patterns: vec![pat(0, entity("account"))],
        });
        pipeline.push(Stage::Insert {
            things: vec![InsertThing {
                binding: id(1),
                type_id: relation(),
                fields: vec![],
                players: vec![(role(), id(0))],
            }],
        });
        ThingDocument::new(id(1), vec![DocumentRole::new(role(), true)])
    } else {
        pipeline.push(Stage::Insert {
            things: vec![thing(0)],
        });
        ThingDocument::new(id(0), vec![])
    };
    pipeline.push(Stage::Fetch(document));
    let mut plan = spec(pipeline, vec![]);
    plan.operation = Operation::Write {
        returns: WriteReturns::Rows,
    };
    QueryPlanV3::new(plan, &schema()).unwrap()
}

/// Add a score value and a member to bound things, keeping existing ones.
pub fn attach() -> QueryPlanV3 {
    use type_bridge_contract::query_plan::v3::AttachItem;
    QueryPlanV3::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("account")), pat(1, relation())],
                },
                Stage::Attach {
                    items: vec![
                        AttachItem::Has {
                            owner: id(0),
                            field: field("score"),
                            operand: literal(3),
                        },
                        AttachItem::Links {
                            relation: id(1),
                            role: role(),
                            player: id(0),
                        },
                    ],
                },
            ],
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Remove every member of a membership, then attach a new one.
pub fn replace_players() -> QueryPlanV3 {
    use type_bridge_contract::query_plan::v3::AttachItem;
    QueryPlanV3::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![pat(0, entity("account")), pat(1, relation())],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Players {
                        relation: id(1),
                        role: role(),
                    }],
                },
                Stage::Attach {
                    items: vec![AttachItem::Links {
                        relation: id(1),
                        role: role(),
                        player: id(0),
                    }],
                },
            ],
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

pub fn plans() -> BTreeMap<&'static str, QueryPlanV3> {
    let mut result = BTreeMap::from([
        ("purchase", purchase()),
        ("touch", touch()),
        ("relation_assign", relation_assign()),
        ("batch", batch()),
    ]);
    for (name, op) in [
        ("add", ArithOp::Add),
        ("sub", ArithOp::Sub),
        ("mul", ArithOp::Mul),
        ("div", ArithOp::Div),
        ("mod", ArithOp::Mod),
        ("pow", ArithOp::Pow),
    ] {
        result.insert(
            name,
            expression_plan(Expr::Arith {
                op,
                lhs: Box::new(Expr::Operand(literal(5))),
                rhs: Box::new(Expr::Operand(literal(2))),
            }),
        );
    }
    for (name, func, args) in [
        ("abs", Builtin::Abs, vec![literal(-2)]),
        ("ceil", Builtin::Ceil, vec![decimal()]),
        ("floor", Builtin::Floor, vec![double()]),
        ("round", Builtin::Round, vec![double()]),
        ("min", Builtin::Min, vec![literal(2), literal(3)]),
        ("max", Builtin::Max, vec![literal(2), literal(3)]),
        ("length", Builtin::Length, vec![string("abc")]),
    ] {
        result.insert(
            name,
            expression_plan(Expr::Call {
                func,
                args: args.into_iter().map(Expr::Operand).collect(),
            }),
        );
    }
    for (name, operation) in [
        ("rows", Operation::Rows),
        ("count", Operation::Count),
        ("exists", Operation::Exists),
    ] {
        let mut plan = spec(
            vec![matched()],
            if operation == Operation::Rows {
                vec![0]
            } else {
                vec![]
            },
        );
        plan.operation = operation;
        result.insert(name, QueryPlanV3::new(plan, &schema()).unwrap());
    }
    let matched = touch()
        .with_operation(
            Operation::Write {
                returns: WriteReturns::Rows,
            },
            &schema(),
        )
        .unwrap();
    result.insert("write_rows", matched);
    result
}
