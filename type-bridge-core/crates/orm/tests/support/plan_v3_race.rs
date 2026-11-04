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
    ArithOp, AttachItem, DeleteItem, DocumentRole, Expr, InsertThing, Operation, Pattern,
    PlanBuilder, PlanSpec, QueryPlanV3, Set, Site, Stage, ThingDocument, WriteReturns,
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

/// The same purchase with caller values bound by the shared core builder.
pub fn implicit_purchase(order_id: &str, item_id: &str, account_id: &str) -> QueryPlanV3 {
    let mut p = PlanBuilder::new();
    let item = p.exact(entity("item"));
    let account = p.exact(entity("account"));
    let item_key = p.field(item, AttributeId::new("item-id").unwrap());
    let account_key = p.field(account, AttributeId::new("account-id").unwrap());
    let stock = p.field(item, AttributeId::new("stock").unwrap());
    let price = p.field(item, AttributeId::new("price").unwrap());
    let balance = p.field(account, AttributeId::new("balance").unwrap());
    let item_value = p.bind_value(Site::new(item, "item-id"), text(item_id));
    let account_value = p.bind_value(Site::new(account, "account-id"), text(account_id));
    let value = |binding| QueryOperand::Binding { binding };
    p.compare(value(item_key), ValueComparator::Equal, item_value)
        .compare(value(account_key), ValueComparator::Equal, account_value)
        .compare(
            value(stock),
            ValueComparator::Greater,
            QueryOperand::Literal {
                value: CanonicalValue::Long(0),
            },
        )
        .compare(
            value(balance),
            ValueComparator::GreaterOrEqual,
            value(price),
        );
    let next_stock = p.let_(
        "next_stock",
        sub(
            value(stock),
            QueryOperand::Literal {
                value: CanonicalValue::Long(1),
            },
        ),
    );
    let next_balance = p.let_("next_balance", sub(value(balance), value(price)));
    p.update(item, owns(entity("item"), "stock"), value(next_stock))
        .update(
            account,
            owns(entity("account"), "balance"),
            value(next_balance),
        );
    let purchase = p.insert(purchase_type());
    let order = p.bind_value(Site::new(purchase, "order-id"), text(order_id));
    p.thing_field(purchase, owns(purchase_type(), "order-id"), order)
        .thing_player(purchase, role("buyer"), account)
        .thing_player(purchase, role("item"), item);
    p.build(&schema(), purchase_template().managed_semantics().clone())
        .unwrap()
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

/// Add an on-call value to every doctor without replacing one.
pub fn attach_on_call() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["doctor"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, entity("doctor"))],
                },
                Stage::Attach {
                    items: vec![AttachItem::Has {
                        optional: false,
                        owner: id(0),
                        field: owns(entity("doctor"), "on-call"),
                        operand: QueryOperand::Literal {
                            value: CanonicalValue::Boolean(true),
                        },
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

/// Remove every buyer of every purchase.
pub fn drop_buyers() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["purchase"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, purchase_type())],
                },
                Stage::Delete {
                    items: vec![DeleteItem::Players {
                        relation: id(0),
                        role: role("buyer"),
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

/// The touch, answering each touched doctor's hydration document.
pub fn touch_fetch() -> QueryPlanV3 {
    let mut plan = touch_spec(
        vec![],
        Operation::Write {
            returns: WriteReturns::Rows,
        },
    );
    plan.pipeline
        .push(Stage::Fetch(ThingDocument::new(id(0), vec![])));
    QueryPlanV3::new(plan, &schema()).unwrap()
}

/// Every purchase with its buyer (required) and item (optional) documents.
pub fn purchase_fetch() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["purchase"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, purchase_type())],
                },
                Stage::Fetch(ThingDocument::new(
                    id(0),
                    vec![
                        DocumentRole::new(role("buyer"), true),
                        DocumentRole::new(role("item"), false),
                    ],
                )),
            ],
            Operation::Rows,
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// What `purchase_fetch` lowers to: the released read-back shape.
pub const PURCHASE_FETCH_TYPEQL: &str = r#"match
  $purchase isa! purchase;
match
  $purchase isa! $PlanFetchType;
  $purchase links (buyer: $PlanFetchRole0); $PlanFetchRole0 isa! $PlanFetchRole0Type;
  try { $purchase links (item: $PlanFetchRole1); $PlanFetchRole1 isa! $PlanFetchRole1Type; };
fetch {
  "_iid": iid($purchase), "_type": label($PlanFetchType), "attributes": { $purchase.* },
  "_role_0_iid": iid($PlanFetchRole0), "_role_0_type": label($PlanFetchRole0Type),
  "_role_0_attributes": { $PlanFetchRole0.* },
  "_role_1_iid": iid($PlanFetchRole1), "_role_1_type": label($PlanFetchRole1Type),
  "_role_1_attributes": { $PlanFetchRole1.* }
};"#;

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

/// Insert a doctor and fetch its document in the same provider statement.
pub fn insert_doctor_fetch(name: &str) -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["doctor"],
            vec![
                Stage::Insert {
                    things: vec![InsertThing {
                        binding: id(0),
                        type_id: entity("doctor"),
                        players: vec![],
                        fields: vec![
                            (
                                owns(entity("doctor"), "name"),
                                QueryOperand::Literal {
                                    value: CanonicalValue::String(
                                        CanonicalString::new(name).unwrap(),
                                    ),
                                },
                            ),
                            (
                                owns(entity("doctor"), "on-call"),
                                QueryOperand::Literal {
                                    value: CanonicalValue::Boolean(true),
                                },
                            ),
                        ],
                    }],
                },
                Stage::Fetch(ThingDocument::new(id(0), vec![])),
            ],
            Operation::Write {
                returns: WriteReturns::Rows,
            },
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Capture all doctors before deleting them in the same write transaction.
pub fn delete_doctors_returning() -> QueryPlanV3 {
    delete_doctors_plan(true)
}

/// The unchanged plain-delete counterpart used by the cost comparison.
pub fn delete_doctors() -> QueryPlanV3 {
    delete_doctors_plan(false)
}

fn delete_doctors_plan(returning: bool) -> QueryPlanV3 {
    let mut pipeline = vec![Stage::Match {
        patterns: vec![isa(0, entity("doctor"))],
    }];
    if returning {
        pipeline.push(Stage::Fetch(ThingDocument::new(id(0), vec![])));
    }
    pipeline.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: id(0) }],
    });
    QueryPlanV3::new(
        spec(
            &["doctor"],
            pipeline,
            Operation::Write {
                returns: if returning {
                    WriteReturns::Rows
                } else {
                    WriteReturns::Matched
                },
            },
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Delete purchases, retaining both role slots and optionally requiring a buyer.
pub fn delete_purchases_returning(require_buyer: bool) -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["purchase"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, purchase_type())],
                },
                Stage::Fetch(ThingDocument::new(
                    id(0),
                    vec![
                        DocumentRole::new(role("buyer"), require_buyer),
                        DocumentRole::new(role("item"), false),
                    ],
                )),
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
            ],
            Operation::Write {
                returns: WriteReturns::Rows,
            },
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Capture and delete only doctors selected by supplied given-row names.
pub fn delete_named_doctors_returning(names: &[&str]) -> QueryPlanV3 {
    let mut plan = spec(
        &["doctor", "name"],
        vec![
            Stage::Match {
                patterns: vec![
                    isa(0, entity("doctor")),
                    has(0, 1, "name"),
                    compare(ValueComparator::Equal, binding(1), input(0)),
                ],
            },
            Stage::Fetch(ThingDocument::new(id(0), vec![])),
            Stage::Delete {
                items: vec![DeleteItem::Thing { binding: id(0) }],
            },
        ],
        Operation::Write {
            returns: WriteReturns::Rows,
        },
        vec![],
    );
    plan.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("selected_name").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    plan.given_rows = Some(
        names
            .iter()
            .map(|name| InputRow::new(vec![Some(text(name))]))
            .collect(),
    );
    QueryPlanV3::new(plan, &schema()).unwrap()
}

/// Extend only the deletion parity fixture, preserving the existing race schema.
pub fn deletion_subtype_schema() -> DeclaredSchema {
    use type_bridge_contract::schema::{SubFact, SubFactId};
    let mut facts = schema().facts().cloned().collect::<Vec<_>>();
    for label in ["doctor-report", "doctor-memo"] {
        let child = entity(label);
        facts.push(SchemaFact::Type(TypeFact::new(child.clone()).unwrap()));
        facts.push(SchemaFact::Sub(SubFact::new(
            SubFactId::new(child, entity("doctor")).unwrap(),
        )));
    }
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("deletion-subtype-parity").unwrap(),
                index as u64,
                index as u64 + 1,
                index as u32 + 1,
                1,
                index as u32 + 1,
                2,
            )
            .unwrap(),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap()
}

/// Capture concrete doctor subtypes selected by a shared attribute filter.
pub fn delete_doctor_subtypes_returning() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["doctor", "on_call"],
            vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Isa {
                            binding: id(0),
                            include_subtypes: true,
                            type_id: entity("doctor"),
                        }
                        .into(),
                        has(0, 1, "on-call"),
                        compare(
                            ValueComparator::Equal,
                            binding(1),
                            QueryOperand::Literal {
                                value: CanonicalValue::Boolean(true),
                            },
                        ),
                    ],
                },
                Stage::Fetch(ThingDocument::new(id(0), vec![])),
                Stage::Delete {
                    items: vec![DeleteItem::Thing { binding: id(0) }],
                },
            ],
            Operation::Write {
                returns: WriteReturns::Rows,
            },
            vec![],
        ),
        &deletion_subtype_schema(),
    )
    .unwrap()
}

/// Read doctor documents using a correlated given string comparator.
pub fn string_read(comparator: ValueComparator, needles: &[&str]) -> QueryPlanV3 {
    string_read_with_limits(
        comparator,
        needles,
        type_bridge_contract::limits::QueryLimits::CANONICAL,
    )
}

pub fn string_read_with_limits(
    comparator: ValueComparator,
    needles: &[&str],
    limits: type_bridge_contract::limits::QueryLimits,
) -> QueryPlanV3 {
    let mut plan = spec(
        &["doctor", "name"],
        vec![
            Stage::Match {
                patterns: vec![
                    isa(0, entity("doctor")),
                    has(0, 1, "name"),
                    QueryPattern::Value {
                        comparator,
                        left: binding(1),
                        right: input(0),
                    }
                    .into(),
                ],
            },
            Stage::Fetch(ThingDocument::new(id(0), vec![])),
        ],
        Operation::Rows,
        vec![],
    );
    plan.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("needle").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    plan.given_rows = Some(
        needles
            .iter()
            .map(|value| {
                InputRow::new(vec![Some(CanonicalValue::String(
                    CanonicalString::new_with_limits(*value, limits.codec).unwrap(),
                ))])
            })
            .collect(),
    );
    QueryPlanV3::new_with_limits(plan, &schema(), limits).unwrap()
}

/// Count stocked items through a provider reduction rather than loading roots.
pub fn stock_reduce_count() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["item", "stock", "total"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, entity("item")), has(0, 1, "stock")],
                },
                Stage::Reduce {
                    assignments: vec![type_bridge_contract::query_plan::ReduceAssignment::new(
                        id(2),
                        type_bridge_contract::query_plan::Reducer::Count,
                        None,
                    )],
                    groups: vec![],
                },
            ],
            Operation::Rows,
            vec![2],
        ),
        &schema(),
    )
    .unwrap()
}

/// Read a purchase with both declared role players.
pub fn purchase_read() -> QueryPlanV3 {
    QueryPlanV3::new(
        spec(
            &["purchase"],
            vec![
                Stage::Match {
                    patterns: vec![isa(0, purchase_type())],
                },
                Stage::Fetch(ThingDocument::new(
                    id(0),
                    vec![
                        DocumentRole::new(role("buyer"), false),
                        DocumentRole::new(role("item"), false),
                    ],
                )),
            ],
            Operation::Rows,
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Reintroduce a discarded doctor under the selected name window.
pub fn rematch_doctor_window() -> QueryPlanV3 {
    use type_bridge_contract::query_plan::{OrderDirection, OrderTerm};
    QueryPlanV3::new(
        spec(
            &["doctor", "name", "peer"],
            vec![
                Stage::Match {
                    patterns: vec![
                        isa(0, entity("doctor")),
                        has(0, 1, "name"),
                        isa(2, entity("doctor")),
                        has(2, 1, "name"),
                    ],
                },
                Stage::Select {
                    bindings: vec![id(0), id(1)],
                },
                Stage::Distinct,
                Stage::Sort {
                    terms: vec![OrderTerm::new(id(1), OrderDirection::Ascending)],
                },
                Stage::Offset { rows: 0 },
                Stage::Limit { rows: 1 },
                Stage::Match {
                    patterns: vec![isa(2, entity("doctor")), has(2, 1, "name")],
                },
                Stage::Fetch(ThingDocument::new(id(2), vec![])),
            ],
            Operation::Rows,
            vec![],
        ),
        &schema(),
    )
    .unwrap()
}

/// Mix a given IID comparison and an ordinary field predicate in one OR.
pub fn doctor_iid_or_name(iid: &str, name: &str) -> QueryPlanV3 {
    let mut plan = spec(
        &["doctor", "name", "identity"],
        vec![
            Stage::Match {
                patterns: vec![
                    isa(0, entity("doctor")),
                    has(0, 1, "name"),
                    Pattern::Let {
                        binding: id(2),
                        expr: Expr::Iid { binding: id(0) },
                    },
                    QueryPattern::Or {
                        branches: vec![
                            vec![QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: binding(2),
                                right: input(0),
                            }],
                            vec![QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: binding(1),
                                right: input(1),
                            }],
                        ],
                    }
                    .into(),
                ],
            },
            Stage::Fetch(ThingDocument::new(id(0), vec![])),
        ],
        Operation::Rows,
        vec![],
    );
    plan.inputs = vec![
        InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_iid").unwrap(),
            ValueTypeTag::String,
            false,
        ),
        InputColumn::new(
            InputColumnId::new(1),
            QueryVariable::new("wanted_name").unwrap(),
            ValueTypeTag::String,
            false,
        ),
    ];
    plan.given_rows = Some(vec![InputRow::new(vec![Some(text(iid)), Some(text(name))])]);
    QueryPlanV3::new(plan, &schema()).unwrap()
}

/// A given IID scoped by a role-player disjunction, with optional negation.
pub fn scoped_given_iid(iid: &str, negative: bool, optional: bool) -> QueryPlanV3 {
    use type_bridge_contract::migration_assertion::AssertionRolePlayer;
    let branch = vec![
        QueryPattern::Isa {
            binding: id(1),
            include_subtypes: false,
            type_id: entity("account"),
        },
        QueryPattern::Links {
            relation: id(0),
            relation_id: purchase_type(),
            players: vec![AssertionRolePlayer::new(role("buyer"), id(1))],
        },
        QueryPattern::Iid {
            binding: id(1),
            iid: input(0),
        },
    ];
    let nested = QueryPattern::Or {
        branches: vec![branch.clone(), branch],
    };
    let scoped = if negative {
        QueryPattern::Not {
            patterns: vec![nested],
        }
    } else if optional {
        QueryPattern::Try {
            patterns: match nested {
                QueryPattern::Or { mut branches } => branches.remove(0),
                _ => unreachable!(),
            },
        }
    } else {
        nested
    };
    let mut plan = spec(
        &["purchase", "buyer"],
        vec![
            Stage::Match {
                patterns: vec![isa(0, purchase_type()), scoped.into()],
            },
            Stage::Select {
                bindings: vec![id(0)],
            },
            Stage::Distinct,
        ],
        Operation::Rows,
        vec![0],
    );
    plan.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("wanted_iid").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    plan.given_rows = Some(vec![InputRow::new(vec![Some(text(iid))])]);
    QueryPlanV3::new(plan, &schema()).unwrap()
}
