//! The one-query purchase: its Split-YAML schema and the plan the SDK
//! builders must produce, built by hand from contract types. Shared by the
//! SDK, server and generated-crate tests.
#![allow(dead_code)]

/// The race schema without keys or cardinalities, matching the live
/// schema the server tests define.
pub const SOURCE: &str = "format: typebridge.schema/v2
attributes:
  item-id: { value: string }
  stock: { value: integer }
  price: { value: integer }
  account-id: { value: string }
  balance: { value: integer }
  order-id: { value: string }
  name: { value: string }
  on-call: { value: boolean }
entities:
  item:
    owns: { item-id: {}, stock: {}, price: {} }
  account:
    owns: { account-id: {}, balance: {} }
  doctor:
    owns: { name: {}, on-call: {} }
relations:
  purchase:
    relates: { buyer: {}, item: {} }
    owns: { order-id: {} }
plays:
  item:
    purchase:
      item: {}
  account:
    purchase:
      buyer: {}
";
pub const SCOPE: &str = "rust-plan-purchase";
pub const PROFILE: &str = "typedb-3.13.6/v1";

/// The same purchase built by hand from contract types: bindings in
/// creation order, the match in call order, both updates in one stage.
pub fn hand_built_purchase(
    declared: &type_bridge_contract::schema::DeclaredSchema,
    semantics: type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint,
    order: &str,
    item_id: &str,
    account_id: &str,
) -> type_bridge_contract::query_plan::QueryPlan {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, BindingId, QueryVariable, ValueComparator,
    };
    use type_bridge_contract::query_plan::{
        ArithOp, Expr, InsertThing, Operation, Pattern, PlanSpec, QueryPlan, Set, Stage,
        WriteReturns,
    };
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
    };
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};

    let id = |n: u16| BindingId::new(n).unwrap();
    let entity = |label: &str| TypeId::new(TypeKind::Entity, label).unwrap();
    let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
    let owns = |owner: TypeId, name: &str| {
        OwnsFactId::new(owner, AttributeId::new(name).unwrap()).unwrap()
    };
    let binding = |n: u16| QueryOperand::Binding { binding: id(n) };
    let input = |n: u16| QueryOperand::Input {
        column: InputColumnId::new(n),
    };
    let long = |n: i64| QueryOperand::Literal {
        value: CanonicalValue::Long(n),
    };
    let text = |s: &str| Some(CanonicalValue::String(CanonicalString::new(s).unwrap()));
    let isa = |n: u16, type_id: TypeId| -> Pattern {
        QueryPattern::Isa {
            binding: id(n),
            include_subtypes: false,
            type_id,
        }
        .into()
    };
    let has = |owner: u16, value: u16, name: &str| -> Pattern {
        QueryPattern::Has {
            owner: id(owner),
            attribute: id(value),
            attribute_id: AttributeId::new(name).unwrap(),
        }
        .into()
    };
    let compare = |comparator, left, right| -> Pattern {
        QueryPattern::Value {
            comparator,
            left,
            right,
        }
        .into()
    };
    let sub = |left, right| Expr::Arith {
        op: ArithOp::Sub,
        lhs: Box::new(Expr::Operand(left)),
        rhs: Box::new(Expr::Operand(right)),
    };
    let names = [
        "item",
        "account",
        "item_item_id",
        "account_account_id",
        "item_stock",
        "item_price",
        "account_balance",
        "next_stock",
        "next_balance",
        "purchase",
    ];
    let spec = PlanSpec {
        bindings: names
            .iter()
            .enumerate()
            .map(|(n, name)| {
                AssertionBinding::new(id(n as u16), QueryVariable::new(*name).unwrap())
            })
            .collect(),
        inputs: ["order_id", "item_id", "account_id"]
            .iter()
            .enumerate()
            .map(|(n, name)| {
                InputColumn::new(
                    InputColumnId::new(n as u16),
                    QueryVariable::new(*name).unwrap(),
                    ValueTypeTag::String,
                    false,
                )
            })
            .collect(),
        given_rows: Some(vec![InputRow::new(vec![
            text(order),
            text(item_id),
            text(account_id),
        ])]),
        operation: Operation::Write {
            returns: WriteReturns::Matched,
        },
        pipeline: vec![
            Stage::Match {
                patterns: vec![
                    isa(0, entity("item")),
                    isa(1, entity("account")),
                    has(0, 2, "item-id"),
                    has(1, 3, "account-id"),
                    has(0, 4, "stock"),
                    has(0, 5, "price"),
                    has(1, 6, "balance"),
                    compare(ValueComparator::Equal, binding(2), input(1)),
                    compare(ValueComparator::Equal, binding(3), input(2)),
                    compare(ValueComparator::Greater, binding(4), long(0)),
                    compare(ValueComparator::GreaterOrEqual, binding(6), binding(5)),
                    Pattern::Let {
                        binding: id(7),
                        expr: sub(binding(4), long(1)),
                    },
                    Pattern::Let {
                        binding: id(8),
                        expr: sub(binding(6), binding(5)),
                    },
                ],
            },
            Stage::Update {
                sets: vec![
                    Set::Field {
                        owner: id(0),
                        field: owns(entity("item"), "stock"),
                        operand: binding(7),
                    },
                    Set::Field {
                        owner: id(1),
                        field: owns(entity("account"), "balance"),
                        operand: binding(8),
                    },
                ],
            },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: id(9),
                    type_id: purchase.clone(),
                    fields: vec![(owns(purchase, "order-id"), input(0))],
                    players: vec![
                        (RoleId::new("purchase", "buyer").unwrap(), id(1)),
                        (RoleId::new("purchase", "item").unwrap(), id(0)),
                    ],
                }],
            },
        ],
        output: QueryOutput::Rows { columns: vec![] },
        managed_semantics: semantics,
        functions: vec![],
    };
    QueryPlan::new(spec, declared).unwrap()
}

/// Read-back golden shared by the four SDK builder tests.
pub fn read_back(
    declared: &type_bridge_contract::schema::DeclaredSchema,
    semantics: type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint,
    item_required: bool,
) -> type_bridge_contract::query_plan::QueryPlan {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::{DocumentRole, PlanBuilder};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    let mut plan = PlanBuilder::new();
    let purchase = plan.exact(TypeId::new(TypeKind::Relation, "purchase").unwrap());
    let key = plan.field(purchase, AttributeId::new("order-id").unwrap());
    let order = plan.input("order_id", ValueTypeTag::String, false);
    plan.compare(
        QueryOperand::Binding { binding: key },
        ValueComparator::Equal,
        QueryOperand::Input { column: order },
    );
    plan.fetch(
        purchase,
        vec![
            DocumentRole::new(RoleId::new("purchase", "buyer").unwrap(), true),
            DocumentRole::new(RoleId::new("purchase", "item").unwrap(), item_required),
        ],
    );
    plan.row(vec![Some(CanonicalValue::String(
        CanonicalString::new("O1").unwrap(),
    ))]);
    plan.build(declared, semantics).unwrap()
}

/// Explicit Core oracle for the generated authoring program's binding schedule.
pub fn authored_purchase(
    declared: &type_bridge_contract::schema::DeclaredSchema,
    semantics: type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint,
    order: &str,
    item_id: &str,
    account_id: &str,
) -> type_bridge_contract::query_plan::QueryPlan {
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::{ArithOp, Expr, PlanBuilder};
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    let entity = |label| TypeId::new(TypeKind::Entity, label).unwrap();
    let attribute = |label| AttributeId::new(label).unwrap();
    let field = |owner, label| OwnsFactId::new(owner, attribute(label)).unwrap();
    let binding = |binding| QueryOperand::Binding { binding };
    let input = |column| QueryOperand::Input { column };
    let sub = |left, right| Expr::Arith {
        op: ArithOp::Sub,
        lhs: Box::new(Expr::Operand(left)),
        rhs: Box::new(Expr::Operand(right)),
    };
    let text = |value| Some(CanonicalValue::String(CanonicalString::new(value).unwrap()));
    let mut p = PlanBuilder::new();
    let item = p.exact(entity("item"));
    let key = p.field(item, attribute("item-id"));
    let item_input = p.input("item_item_id_2", ValueTypeTag::String, false);
    p.compare(binding(key), ValueComparator::Equal, input(item_input));
    let stock = p.field(item, attribute("stock"));
    let minimum = p.input("item_stock_2", ValueTypeTag::Long, false);
    p.compare(binding(stock), ValueComparator::Greater, input(minimum));
    let buyer = p.exact(entity("account"));
    let account = p.field(buyer, attribute("account-id"));
    let account_input = p.input("account_account_id_2", ValueTypeTag::String, false);
    p.compare(
        binding(account),
        ValueComparator::Equal,
        input(account_input),
    );
    let price = p.field(item, attribute("price"));
    let balance = p.field(buyer, attribute("balance"));
    p.compare(
        binding(price),
        ValueComparator::LessOrEqual,
        binding(balance),
    );
    let decrement = p.input("item_stock_3", ValueTypeTag::Long, false);
    let next_stock = p.let_("item_stock_4", sub(binding(stock), input(decrement)));
    p.update(item, field(entity("item"), "stock"), binding(next_stock));
    let next_balance = p.let_("account_balance_2", sub(binding(balance), binding(price)));
    p.update(
        buyer,
        field(entity("account"), "balance"),
        binding(next_balance),
    );
    let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
    let created = p.insert(purchase.clone());
    let order_input = p.input("purchase_order_id", ValueTypeTag::String, false);
    p.thing_field(created, field(purchase, "order-id"), input(order_input));
    p.thing_player(created, RoleId::new("purchase", "buyer").unwrap(), buyer);
    p.thing_player(created, RoleId::new("purchase", "item").unwrap(), item);
    p.row(vec![
        text(item_id),
        Some(CanonicalValue::Long(0)),
        text(account_id),
        Some(CanonicalValue::Long(1)),
        text(order),
    ]);
    p.build(declared, semantics).unwrap()
}
