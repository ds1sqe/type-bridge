//! The one-query race scenarios as `query-plan/v3` programs, with the
//! hand-written TypeQL each must lower to. Shared by the in-crate lowering
//! tests and the live plan execution test.
#![allow(dead_code)]

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::v3::{
    ArithOp, DeleteItem, Expr, InsertThing, Operation, Pattern, PlanSpec, QueryPlanV3, Set, Stage,
    WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact,
    RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};

/// The race schema, as TypeDB receives it.
pub const SCHEMA_TYPEQL: &str = "define
attribute item-id, value string; attribute stock, value integer; attribute price, value integer;
attribute account-id, value string; attribute balance, value integer;
attribute order-id, value string; attribute name, value string; attribute on-call, value boolean;
entity item, owns item-id @key, owns stock @card(0..1), owns price @card(0..1), plays purchase:item;
entity account, owns account-id @key, owns balance @card(0..1), plays purchase:buyer;
entity doctor, owns name @key, owns on-call @card(0..1);
relation purchase, relates buyer, relates item, owns order-id @key;";

/// The one-query purchase: check stock and balance, take one item and the
/// price, and record the purchase, all in one pipeline.
pub const PURCHASE_TYPEQL: &str = "given $order_id: string, $item_id: string, $account_id: string;
match
  $item isa! item;
  $buyer isa! account;
  $item has item-id $item_key; $item_key isa! item-id;
  $item has stock $stock; $stock isa! stock;
  $item has price $price; $price isa! price;
  $buyer has account-id $buyer_key; $buyer_key isa! account-id;
  $buyer has balance $balance; $balance isa! balance;
  $item_key == $item_id;
  $buyer_key == $account_id;
  $stock > 0;
  $balance >= $price;
  let $next_stock = ($stock - 1);
  let $next_balance = ($balance - $price);
update
  $item has stock == $next_stock;
  $buyer has balance == $next_balance;
insert
  $purchase isa purchase, links (buyer: $buyer, item: $item), has order-id == $order_id;";

/// The touch: rewrite the value that was read, which takes its lock.
pub const TOUCH_TYPEQL: &str = "match
  $doctor isa! doctor;
  $doctor has on-call $on_call; $on_call isa! on-call;
update
  $doctor has on-call == $on_call;";

/// Remove one ownership without deleting the owner.
pub const DROP_PRICE_TYPEQL: &str = "match
  $item isa! item;
  $item has item-id $item_key; $item_key isa! item-id;
  $item_key == \"I1\";
match
  try { $item has price $PlanHas1x0; $PlanHas1x0 isa! price; };
delete
  try { has $PlanHas1x0 of $item; };";

/// The stock read used for count and exists.
pub const STOCK_READ_TYPEQL: &str = "match
  $item isa! item;
  $item has stock $stock; $stock isa! stock;";

fn id(n: u16) -> BindingId {
    BindingId::new(n).unwrap()
}
fn entity(name: &str) -> TypeId {
    TypeId::new(TypeKind::Entity, name).unwrap()
}
fn purchase_type() -> TypeId {
    TypeId::new(TypeKind::Relation, "purchase").unwrap()
}
fn attribute(name: &str) -> TypeId {
    TypeId::new(TypeKind::Attribute, name).unwrap()
}
fn owns(owner: TypeId, name: &str) -> OwnsFactId {
    OwnsFactId::new(owner, AttributeId::new(name).unwrap()).unwrap()
}
fn role(name: &str) -> RoleId {
    RoleId::new("purchase", name).unwrap()
}
fn binding(n: u16) -> QueryOperand {
    QueryOperand::Binding { binding: id(n) }
}
fn input(n: u16) -> QueryOperand {
    QueryOperand::Input {
        column: InputColumnId::new(n),
    }
}
fn text(value: &str) -> CanonicalValue {
    CanonicalValue::String(CanonicalString::new(value).unwrap())
}
fn isa(n: u16, type_id: TypeId) -> Pattern {
    QueryPattern::Isa {
        binding: id(n),
        include_subtypes: false,
        type_id,
    }
    .into()
}
fn has(owner: u16, value: u16, name: &str) -> Pattern {
    QueryPattern::Has {
        owner: id(owner),
        attribute: id(value),
        attribute_id: AttributeId::new(name).unwrap(),
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

/// Declared authority for the race schema.
pub fn schema() -> DeclaredSchema {
    let mut facts = Vec::new();
    for type_id in [
        entity("item"),
        entity("account"),
        entity("doctor"),
        purchase_type(),
    ] {
        facts.push(SchemaFact::Type(TypeFact::new(type_id).unwrap()));
    }
    for (name, value_type) in [
        ("item-id", ValueTypeTag::String),
        ("stock", ValueTypeTag::Long),
        ("price", ValueTypeTag::Long),
        ("account-id", ValueTypeTag::String),
        ("balance", ValueTypeTag::Long),
        ("order-id", ValueTypeTag::String),
        ("name", ValueTypeTag::String),
        ("on-call", ValueTypeTag::Boolean),
    ] {
        facts.push(SchemaFact::Type(TypeFact::new(attribute(name)).unwrap()));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(AttributeId::new(name).unwrap()),
            value_type,
        )));
    }
    for (owner, name) in [
        (entity("item"), "item-id"),
        (entity("item"), "stock"),
        (entity("item"), "price"),
        (entity("account"), "account-id"),
        (entity("account"), "balance"),
        (entity("doctor"), "name"),
        (entity("doctor"), "on-call"),
        (purchase_type(), "order-id"),
    ] {
        facts.push(SchemaFact::Owns(OwnsFact::new(owns(owner, name))));
    }
    for name in ["buyer", "item"] {
        facts.push(SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(purchase_type(), role(name)).unwrap(),
                None,
            )
            .unwrap(),
        ));
    }
    facts.push(SchemaFact::Plays(PlaysFact::new(
        PlaysFactId::new(entity("item"), role("item")).unwrap(),
    )));
    facts.push(SchemaFact::Plays(PlaysFact::new(
        PlaysFactId::new(entity("account"), role("buyer")).unwrap(),
    )));
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("plan-v3-race").unwrap(),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .unwrap(),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap()
}

fn spec(names: &[&str], pipeline: Vec<Stage>, operation: Operation, output: Vec<u16>) -> PlanSpec {
    PlanSpec {
        bindings: names
            .iter()
            .enumerate()
            .map(|(n, name)| {
                AssertionBinding::new(
                    id(u16::try_from(n).unwrap()),
                    QueryVariable::new(*name).unwrap(),
                )
            })
            .collect(),
        inputs: vec![],
        given_rows: None,
        operation,
        pipeline,
        output: QueryOutput::Rows {
            columns: output.into_iter().map(id).collect(),
        },
        managed_semantics: ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
            b"plan-v3-race-authority",
        )
        .unwrap(),
        compatibility: None,
        functions: vec![],
    }
}

const MATCHED: Operation = Operation::Write {
    returns: WriteReturns::Matched,
};

fn purchase_spec() -> PlanSpec {
    let patterns = vec![
        isa(0, entity("item")),
        isa(1, entity("account")),
        has(0, 8, "item-id"),
        has(0, 2, "stock"),
        has(0, 4, "price"),
        has(1, 9, "account-id"),
        has(1, 3, "balance"),
        compare(ValueComparator::Equal, binding(8), input(1)),
        compare(ValueComparator::Equal, binding(9), input(2)),
        compare(
            ValueComparator::Greater,
            binding(2),
            QueryOperand::Literal {
                value: CanonicalValue::Long(0),
            },
        ),
        compare(ValueComparator::GreaterOrEqual, binding(3), binding(4)),
        Pattern::Let {
            binding: id(5),
            expr: sub(
                binding(2),
                QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                },
            ),
        },
        Pattern::Let {
            binding: id(6),
            expr: sub(binding(3), binding(4)),
        },
    ];
    let mut plan = spec(
        &[
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
        ],
        vec![
            Stage::Match { patterns },
            Stage::Update {
                sets: vec![
                    Set::Field {
                        owner: id(0),
                        field: owns(entity("item"), "stock"),
                        operand: binding(5),
                    },
                    Set::Field {
                        owner: id(1),
                        field: owns(entity("account"), "balance"),
                        operand: binding(6),
                    },
                ],
            },
            Stage::Insert {
                things: vec![InsertThing {
                    binding: id(7),
                    type_id: purchase_type(),
                    fields: vec![(owns(purchase_type(), "order-id"), input(0))],
                    players: vec![(role("buyer"), id(1)), (role("item"), id(0))],
                }],
            },
        ],
        MATCHED,
        vec![],
    );
    plan.inputs = ["order_id", "item_id", "account_id"]
        .into_iter()
        .enumerate()
        .map(|(n, name)| {
            InputColumn::new(
                InputColumnId::new(u16::try_from(n).unwrap()),
                QueryVariable::new(name).unwrap(),
                ValueTypeTag::String,
                false,
            )
        })
        .collect();
    plan
}

/// The purchase as a reusable template without bound rows.
pub fn purchase_template() -> QueryPlanV3 {
    QueryPlanV3::new(purchase_spec(), &schema()).unwrap()
}

/// The purchase bound to one order, item and buyer.
pub fn purchase(order: &str, item: &str, account: &str) -> QueryPlanV3 {
    let mut plan = purchase_spec();
    plan.given_rows = Some(vec![InputRow::new(vec![
        Some(text(order)),
        Some(text(item)),
        Some(text(account)),
    ])]);
    QueryPlanV3::new(plan, &schema()).unwrap()
}

fn touch_spec(output: Vec<u16>, operation: Operation) -> PlanSpec {
    spec(
        &["doctor", "on_call"],
        vec![
            Stage::Match {
                patterns: vec![isa(0, entity("doctor")), has(0, 1, "on-call")],
            },
            Stage::Update {
                sets: vec![Set::Field {
                    owner: id(0),
                    field: owns(entity("doctor"), "on-call"),
                    operand: binding(1),
                }],
            },
        ],
        operation,
        output,
    )
}

/// Rewrite every doctor's on-call value with itself.
pub fn touch() -> QueryPlanV3 {
    QueryPlanV3::new(touch_spec(vec![], MATCHED), &schema()).unwrap()
}

/// The touch, returning the touched doctors from the same pipeline.
pub fn touch_rows() -> QueryPlanV3 {
    QueryPlanV3::new(
        touch_spec(
            vec![0],
            Operation::Write {
                returns: WriteReturns::Rows,
            },
        ),
        &schema(),
    )
    .unwrap()
}

/// Delete the price of item `I1`.
pub fn drop_price() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["item", "item_key"],
            vec![
                Stage::Match {
                    patterns: vec![
                        isa(0, entity("item")),
                        has(0, 1, "item-id"),
                        compare(
                            ValueComparator::Equal,
                            binding(1),
                            QueryOperand::Literal { value: text("I1") },
                        ),
                    ],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Has {
                        owner: id(0),
                        field: owns(entity("item"), "price"),
                    }],
                },
            ],
            MATCHED,
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Read item stock with one read operation.
pub fn stock_read(operation: Operation) -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["item", "stock"],
            vec![Stage::Match {
                patterns: vec![isa(0, entity("item")), has(0, 1, "stock")],
            }],
            operation,
            if operation == Operation::Rows {
                vec![1]
            } else {
                vec![]
            },
        ),
        &schema(),
    )
    .unwrap()
}
