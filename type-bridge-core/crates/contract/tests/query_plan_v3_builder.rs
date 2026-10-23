//! The plan builder gives the same canonical bytes as the plan built by
//! hand under its identity and naming rules.

#[path = "support/plan_v3.rs"]
#[allow(dead_code)]
mod support;

use support::*;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, QueryVariable, ValueComparator};
use type_bridge_contract::query_plan::v3::{
    ArithOp, DeleteItem, Expr, InsertThing, Operation, Pattern, PlanBuilder, PlanSpec, QueryPlanV3,
    Set, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

fn purchase_type() -> TypeId {
    TypeId::new(TypeKind::Relation, "purchase").unwrap()
}
fn owns(owner: TypeId, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(owner, AttributeId::new(attribute).unwrap()).unwrap()
}
fn attribute(name: &str) -> AttributeId {
    AttributeId::new(name).unwrap()
}
fn binding_operand(n: u16) -> QueryOperand {
    QueryOperand::Binding { binding: id(n) }
}
fn input_operand(n: u16) -> QueryOperand {
    QueryOperand::Input {
        column: InputColumnId::new(n),
    }
}
fn text(value: &str) -> Option<CanonicalValue> {
    match string(value) {
        QueryOperand::Literal { value } => Some(value),
        _ => unreachable!(),
    }
}
fn has(owner: u16, value: u16, name: &str) -> Pattern {
    QueryPattern::Has {
        owner: id(owner),
        attribute: id(value),
        attribute_id: attribute(name),
    }
    .into()
}
fn compare(comparator: ValueComparator, left: QueryOperand, right: QueryOperand) -> Pattern {
    QueryPattern::Value {
        comparator,
        left,
        right,
    }
    .into()
}
fn sub(left: QueryOperand, right: QueryOperand) -> Expr {
    Expr::Arith {
        op: ArithOp::Sub,
        lhs: Box::new(Expr::Operand(left)),
        rhs: Box::new(Expr::Operand(right)),
    }
}
fn bindings(names: &[&str]) -> Vec<AssertionBinding> {
    names
        .iter()
        .enumerate()
        .map(|(n, name)| binding(u16::try_from(n).unwrap(), name))
        .collect()
}
fn column(n: u16, name: &str) -> InputColumn {
    InputColumn::new(
        InputColumnId::new(n),
        QueryVariable::new(name).unwrap(),
        ValueTypeTag::String,
        false,
    )
}

/// The one-query purchase, written with the builder.
fn built_purchase() -> QueryPlanV3 {
    let mut plan = PlanBuilder::new();
    let item = plan.exact(entity("item"));
    let buyer = plan.exact(entity("account"));
    let purchase_id = plan.input("purchase_id", ValueTypeTag::String, false);
    let item_id = plan.input("item_id", ValueTypeTag::String, false);
    let buyer_id = plan.input("buyer_id", ValueTypeTag::String, false);
    let item_key = plan.field(item, attribute("item-id"));
    let buyer_key = plan.field(buyer, attribute("id"));
    let stock = plan.field(item, attribute("stock"));
    let price = plan.field(item, attribute("price"));
    let balance = plan.field(buyer, attribute("balance"));
    let binding = |binding| QueryOperand::Binding { binding };
    let input = |column| QueryOperand::Input { column };
    plan.compare(binding(item_key), ValueComparator::Equal, input(item_id))
        .compare(binding(buyer_key), ValueComparator::Equal, input(buyer_id))
        .compare(binding(stock), ValueComparator::Greater, literal(0))
        .compare(
            binding(balance),
            ValueComparator::GreaterOrEqual,
            binding(price),
        );
    let next_stock = plan.let_(
        "next_stock",
        Expr::Arith {
            op: ArithOp::Sub,
            lhs: Box::new(Expr::Operand(binding(stock))),
            rhs: Box::new(Expr::Operand(literal(1))),
        },
    );
    let next_balance = plan.let_(
        "next_balance",
        Expr::Arith {
            op: ArithOp::Sub,
            lhs: Box::new(Expr::Operand(binding(balance))),
            rhs: Box::new(Expr::Operand(binding(price))),
        },
    );
    plan.update(item, owns(entity("item"), "stock"), binding(next_stock))
        .update(
            buyer,
            owns(entity("account"), "balance"),
            binding(next_balance),
        );
    let purchase = plan.insert(purchase_type());
    plan.thing_field(
        purchase,
        owns(purchase_type(), "purchase-id"),
        input(purchase_id),
    )
    .thing_player(purchase, RoleId::new("purchase", "buyer").unwrap(), buyer)
    .thing_player(purchase, RoleId::new("purchase", "item").unwrap(), item)
    .row(vec![text("purchase-1"), text("item-1"), text("buyer-1")]);
    plan.build(&schema(), semantics()).unwrap()
}

/// The same purchase built by hand: bindings in creation order, the match
/// in call order, the two updates sharing one stage.
fn hand_built_purchase() -> QueryPlanV3 {
    let spec = PlanSpec {
        bindings: bindings(&[
            "item",
            "account",
            "item_item_id",
            "account_id",
            "item_stock",
            "item_price",
            "account_balance",
            "next_stock",
            "next_balance",
            "purchase",
        ]),
        inputs: vec![
            column(0, "purchase_id"),
            column(1, "item_id"),
            column(2, "buyer_id"),
        ],
        given_rows: Some(vec![InputRow::new(vec![
            text("purchase-1"),
            text("item-1"),
            text("buyer-1"),
        ])]),
        operation: Operation::Write {
            returns: WriteReturns::Matched,
        },
        pipeline: vec![
            Stage::Match {
                patterns: vec![
                    pat(0, entity("item")),
                    pat(1, entity("account")),
                    has(0, 2, "item-id"),
                    has(1, 3, "id"),
                    has(0, 4, "stock"),
                    has(0, 5, "price"),
                    has(1, 6, "balance"),
                    compare(ValueComparator::Equal, binding_operand(2), input_operand(1)),
                    compare(ValueComparator::Equal, binding_operand(3), input_operand(2)),
                    compare(ValueComparator::Greater, binding_operand(4), literal(0)),
                    compare(
                        ValueComparator::GreaterOrEqual,
                        binding_operand(6),
                        binding_operand(5),
                    ),
                    Pattern::Let {
                        binding: id(7),
                        expr: sub(binding_operand(4), literal(1)),
                    },
                    Pattern::Let {
                        binding: id(8),
                        expr: sub(binding_operand(6), binding_operand(5)),
                    },
                ],
            },
            Stage::Update {
                sets: vec![
                    Set::Field {
                        owner: id(0),
                        field: owns(entity("item"), "stock"),
                        operand: binding_operand(7),
                    },
                    Set::Field {
                        owner: id(1),
                        field: owns(entity("account"), "balance"),
                        operand: binding_operand(8),
                    },
                ],
            },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: id(9),
                    type_id: purchase_type(),
                    fields: vec![(owns(purchase_type(), "purchase-id"), input_operand(0))],
                    players: vec![
                        (RoleId::new("purchase", "buyer").unwrap(), id(1)),
                        (RoleId::new("purchase", "item").unwrap(), id(0)),
                    ],
                }],
            },
        ],
        output: QueryOutput::Rows { columns: vec![] },
        managed_semantics: semantics(),
        compatibility: None,
        functions: vec![],
    };
    QueryPlanV3::new(spec, &schema()).unwrap()
}

#[test]
fn builder_purchase_bytes_equal_the_hand_built_plan() {
    let built = built_purchase();
    let hand = hand_built_purchase();
    assert_eq!(
        built.canonical_bytes().unwrap(),
        hand.canonical_bytes().unwrap()
    );
    assert_eq!(built.fingerprint().unwrap(), hand.fingerprint().unwrap());
}

#[test]
fn builder_reads_choose_rows_count_or_exists() {
    let read = |terminal: fn(&mut PlanBuilder)| {
        let mut plan = PlanBuilder::new();
        let account = plan.exact(entity("account"));
        plan.returning(vec![account]);
        terminal(&mut plan);
        plan.build(&schema(), semantics()).unwrap()
    };
    assert_eq!(read(|_| {}).operation(), Operation::Rows);
    assert_eq!(
        read(|plan| {
            plan.count();
        })
        .operation(),
        Operation::Count
    );
    let exists = read(|plan| {
        plan.exists();
    });
    assert_eq!(exists.operation(), Operation::Exists);
    assert_eq!(exists.output(), &QueryOutput::Rows { columns: vec![] });
}

#[test]
fn builder_write_returning_rows_and_coalesced_stages() {
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    let peer = plan.exact(entity("account"));
    plan.delete_field(account, field("score"))
        .delete_field(peer, field("score"))
        .returning(vec![account, peer]);
    let built = plan.build(&schema(), semantics()).unwrap();
    assert_eq!(
        built.operation(),
        Operation::Write {
            returns: WriteReturns::Rows
        }
    );
    assert_eq!(built.bindings()[1].variable().as_str(), "account_2");
    assert_eq!(
        built.pipeline()[1],
        Stage::Delete {
            items: vec![
                DeleteItem::Has {
                    owner: id(0),
                    field: field("score"),
                },
                DeleteItem::Has {
                    owner: id(1),
                    field: field("score"),
                },
            ],
        }
    );
}

#[test]
fn builder_batch_insert_is_one_plan_over_rows() {
    let mut plan = PlanBuilder::new();
    let new_id = plan.input("new_id", ValueTypeTag::String, false);
    let new_score = plan.input("new_score", ValueTypeTag::Long, false);
    let account = plan.insert(entity("account"));
    plan.thing_field(account, field("id"), QueryOperand::Input { column: new_id })
        .thing_field(
            account,
            field("score"),
            QueryOperand::Input { column: new_score },
        );
    for n in 1..=3 {
        plan.row(vec![
            text(&format!("account-{n}")),
            Some(CanonicalValue::Long(n)),
        ]);
    }
    let built = plan.build(&schema(), semantics()).unwrap();
    assert_eq!(built.given_rows().unwrap().len(), 3);
    assert_eq!(built.pipeline().len(), 1);
}

#[test]
fn builder_reports_its_first_invalid_call_at_build() {
    let code = |plan: PlanBuilder| {
        plan.build(&schema(), semantics())
            .unwrap_err()
            .code()
            .as_str()
            .to_owned()
    };
    let mut duplicate = PlanBuilder::new();
    duplicate.exact(entity("account"));
    duplicate.input("account", ValueTypeTag::String, false);
    assert_eq!(code(duplicate), "query_plan_builder_duplicate_name");

    let mut not_created = PlanBuilder::new();
    let account = not_created.exact(entity("account"));
    not_created.thing_field(account, field("score"), literal(1));
    assert_eq!(code(not_created), "query_plan_builder_not_created");

    let mut counted_write = PlanBuilder::new();
    let account = counted_write.exact(entity("account"));
    counted_write.delete(account).count();
    assert_eq!(code(counted_write), "query_plan_builder_write_operation");

    let mut untyped = PlanBuilder::new();
    let account = untyped.exact(entity("account"));
    let score = untyped.field(account, attribute("score"));
    untyped.links(score, role(), account);
    assert_eq!(code(untyped), "query_plan_builder_untyped_relation");

    // Plan validation still runs: `item` does not own `score`.
    let mut unowned = PlanBuilder::new();
    let item = unowned.exact(entity("item"));
    unowned.update(item, owns(entity("item"), "score"), literal(1));
    assert!(unowned.build(&schema(), semantics()).is_err());
}

#[test]
fn builder_put_and_links_build() {
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    let membership = plan.exact(relation());
    plan.links(membership, role(), account);
    let other = plan.put(entity("account"));
    plan.thing_field(other, field("id"), string("account-b"));
    let built = plan.build(&schema(), semantics()).unwrap();
    assert_eq!(built.bindings()[2].variable().as_str(), "account_2");
    assert!(matches!(built.pipeline()[1], Stage::Put { .. }));
}

#[test]
fn builder_refuses_typedb_3_14_query_rules_on_every_profile() {
    // Every SDK's plan builder wraps this builder, so the refusal reaches
    // every language from here.
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    let shift = plan.field(account, attribute("shift"));
    plan.compare(
        QueryOperand::Binding { binding: shift },
        ValueComparator::Greater,
        QueryOperand::Binding { binding: shift },
    );
    assert_eq!(
        plan.build(&schema_with_duration(), semantics())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_duration_range"
    );
}
