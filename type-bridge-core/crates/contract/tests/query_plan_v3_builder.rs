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
    Set, Site, Stage, WriteReturns,
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
fn implicit_purchase_equals_explicit_inputs_with_derived_names() {
    fn purchase(implicit: bool) -> QueryPlanV3 {
        let mut plan = PlanBuilder::new();
        let item = plan.exact(entity("item"));
        let buyer = plan.exact(entity("account"));
        let item_key = plan.field(item, attribute("item-id"));
        let buyer_key = plan.field(buyer, attribute("id"));
        let stock = plan.field(item, attribute("stock"));
        let price = plan.field(item, attribute("price"));
        let balance = plan.field(buyer, attribute("balance"));
        let item_id = if implicit {
            plan.bind_value(Site::new(item, "item-id"), text("item-1").unwrap())
        } else {
            QueryOperand::Input {
                column: plan.input("item_item_id_2", ValueTypeTag::String, false),
            }
        };
        let buyer_id = if implicit {
            plan.bind_value(Site::new(buyer, "id"), text("buyer-1").unwrap())
        } else {
            QueryOperand::Input {
                column: plan.input("account_id_2", ValueTypeTag::String, false),
            }
        };
        plan.compare(
            QueryOperand::Binding { binding: item_key },
            ValueComparator::Equal,
            item_id,
        )
        .compare(
            QueryOperand::Binding { binding: buyer_key },
            ValueComparator::Equal,
            buyer_id,
        )
        .compare(
            QueryOperand::Binding { binding: stock },
            ValueComparator::Greater,
            literal(0),
        )
        .compare(
            QueryOperand::Binding { binding: balance },
            ValueComparator::GreaterOrEqual,
            QueryOperand::Binding { binding: price },
        );
        let next_stock = plan.let_(
            "next_stock",
            sub(QueryOperand::Binding { binding: stock }, literal(1)),
        );
        let next_balance = plan.let_(
            "next_balance",
            sub(
                QueryOperand::Binding { binding: balance },
                QueryOperand::Binding { binding: price },
            ),
        );
        plan.update(
            item,
            owns(entity("item"), "stock"),
            QueryOperand::Binding {
                binding: next_stock,
            },
        )
        .update(
            buyer,
            owns(entity("account"), "balance"),
            QueryOperand::Binding {
                binding: next_balance,
            },
        );
        let purchase = plan.insert(purchase_type());
        let purchase_id = if implicit {
            plan.bind_value(
                Site::new(purchase, "purchase-id"),
                text("purchase-1").unwrap(),
            )
        } else {
            QueryOperand::Input {
                column: plan.input("purchase_purchase_id", ValueTypeTag::String, false),
            }
        };
        plan.thing_field(purchase, owns(purchase_type(), "purchase-id"), purchase_id)
            .thing_player(purchase, RoleId::new("purchase", "buyer").unwrap(), buyer)
            .thing_player(purchase, RoleId::new("purchase", "item").unwrap(), item);
        if !implicit {
            plan.row(vec![text("item-1"), text("buyer-1"), text("purchase-1")]);
        }
        plan.build(&schema(), semantics()).unwrap()
    }
    let implicit = purchase(true);
    let explicit = purchase(false);
    assert_eq!(
        implicit.canonical_bytes().unwrap(),
        explicit.canonical_bytes().unwrap()
    );
    assert_eq!(
        implicit.fingerprint().unwrap(),
        explicit.fingerprint().unwrap()
    );
}

#[test]
fn implicit_names_share_the_binding_namespace_and_use_collision_suffixes() {
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    plan.field(account, attribute("score"));
    let first = plan.bind_value(Site::new(account, "score"), CanonicalValue::Long(1));
    let second = plan.bind_value(Site::new(account, "score"), CanonicalValue::Long(2));
    plan.compare(first, ValueComparator::Less, second).count();
    let built = plan.build(&schema(), semantics()).unwrap();
    assert_eq!(built.inputs()[0].public_name().as_str(), "account_score_2");
    assert_eq!(built.inputs()[1].public_name().as_str(), "account_score_3");
    assert_eq!(
        built.given_rows().unwrap()[0].values(),
        &[Some(CanonicalValue::Long(1)), Some(CanonicalValue::Long(2))]
    );
}

#[test]
fn explicit_and_implicit_inputs_are_refused_in_both_orders() {
    for explicit_first in [false, true] {
        for explicit_row in [false, true] {
            let mut plan = PlanBuilder::new();
            let account = plan.exact(entity("account"));
            let explicit = |plan: &mut PlanBuilder| {
                if explicit_row {
                    plan.row(vec![]);
                } else {
                    plan.input("id", ValueTypeTag::String, false);
                }
            };
            if explicit_first {
                explicit(&mut plan);
            }
            plan.bind_value(Site::new(account, "id"), text("account-1").unwrap());
            if !explicit_first {
                explicit(&mut plan);
            }
            assert_eq!(
                plan.build(&schema(), semantics())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "plan_builder_mixed_inputs"
            );
        }
    }
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
fn generated_field_access_reuses_bindings_but_explicit_fields_stay_fresh() {
    let mut plan = PlanBuilder::new();
    let item = plan.exact(entity("item"));
    let stock = plan.field_value(item, attribute("stock"));
    assert_eq!(stock, plan.field_value(item, attribute("stock")));
    assert_ne!(stock, plan.field(item, attribute("stock")));
}

#[test]
fn recording_query_policy_reaches_plan_validation_without_entering_wire_bytes() {
    use type_bridge_contract::limits::QueryLimits;
    let make = |limits| {
        let mut builder = PlanBuilder::with_limits(limits);
        builder.exact(entity("account"));
        builder.count();
        builder
    };
    let original = make(QueryLimits::CANONICAL)
        .build(&schema(), semantics())
        .unwrap();
    let mut limits = QueryLimits::CANONICAL;
    limits.codec.max_bytes /= 2;
    let selected = make(limits).build(&schema(), semantics()).unwrap();
    assert_eq!(selected.limits(), limits);
    assert_eq!(
        selected.canonical_bytes().unwrap(),
        original.canonical_bytes().unwrap()
    );
    limits.codec.max_bytes = original.canonical_bytes().unwrap().len() - 1;
    limits.codec.max_string_bytes = limits.codec.max_bytes;
    assert_eq!(
        make(limits)
            .build(&schema(), semantics())
            .unwrap_err()
            .category(),
        type_bridge_contract::diagnostic::DiagnosticCategory::ResourceLimit
    );
}

#[test]
fn ordered_and_unordered_roles_allow_complete_authored_write_model_fetches() {
    use type_bridge_contract::query_plan::v3::DocumentRole;
    for schema in [schema(), schema_with_ordered_role()] {
        let mut read = PlanBuilder::new();
        let membership = read.exact(relation());
        read.fetch_held(membership, vec![DocumentRole::new(role(), false)]);
        read.build_plans(
            &schema,
            semantics(),
            false,
            type_bridge_contract::split::SplitLimits::CANONICAL,
        )
        .unwrap()
        .validate_model_projection()
        .unwrap();
        let mut write = PlanBuilder::new();
        let membership = write.exact(relation());
        write.delete(membership);
        write
            .build_plans(
                &schema,
                semantics(),
                true,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .unwrap();
        write.fetch_held(membership, vec![DocumentRole::new(role(), false)]);
        write
            .build_plans(
                &schema,
                semantics(),
                false,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .unwrap()
            .validate_model_projection()
            .unwrap();
    }
}

#[test]
fn authored_write_model_fetch_checks_ordinary_roles_after_an_ordered_role() {
    use type_bridge_contract::query_plan::v3::DocumentRole;
    let schema = schema_with_ordered_and_unordered_roles();
    for complete in [false, true] {
        let mut write = PlanBuilder::new();
        let membership = write.exact(relation());
        write.delete(membership);
        let mut roles = vec![DocumentRole::new(role(), false)];
        if complete {
            roles.push(DocumentRole::new(
                RoleId::new("membership", "z-tail").unwrap(),
                false,
            ));
        }
        write.fetch_held(membership, roles);
        let built = write
            .build_plans(
                &schema,
                semantics(),
                false,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .unwrap();
        if complete {
            built.validate_model_projection().unwrap();
        } else {
            let error = built.validate_model_projection().unwrap_err();
            assert_eq!(error.code().as_str(), "plan_returning_models_after_write");
            assert!(!error.message().contains("membership"));
        }
    }
}

#[test]
fn model_writes_refuse_missing_subtype_roles_before_staging() {
    use type_bridge_contract::query_plan::v3::DocumentRole;
    let schema = schema_with_subtype_role();
    let mut plan = PlanBuilder::new();
    let membership = plan.subtypes(relation());
    plan.delete(membership);
    plan.fetch_held(membership, vec![DocumentRole::new(role(), false)]);
    let build = |plan: &PlanBuilder| {
        plan.build_plans(
            &schema,
            semantics(),
            false,
            type_bridge_contract::split::SplitLimits::CANONICAL,
        )
        .unwrap()
    };
    assert_eq!(
        build(&plan)
            .validate_model_projection()
            .unwrap_err()
            .code()
            .as_str(),
        "plan_returning_models_after_write"
    );
    let mut complete = PlanBuilder::new();
    let child = complete.exact(TypeId::new(TypeKind::Relation, "child-membership").unwrap());
    complete.delete(child);
    complete.fetch_held(
        child,
        vec![
            DocumentRole::new(role(), false),
            DocumentRole::new(RoleId::new("child-membership", "extra").unwrap(), false),
        ],
    );
    build(&complete).validate_model_projection().unwrap();
}

#[test]
fn generated_link_attaches_instead_of_replacing_existing_players() {
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    let membership = plan.exact(relation());
    plan.link(membership, role(), account);
    let built = plan.build(&schema(), semantics()).unwrap();
    assert!(matches!(
        &built.pipeline()[1],
        Stage::Attach { items } if matches!(&items[..], [type_bridge_contract::query_plan::v3::AttachItem::Links { relation, player, .. }] if *relation == membership && *player == account)
    ));
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

#[test]
fn builder_fetch_matches_validated_document_plans() {
    use type_bridge_contract::query_plan::v3::{DocumentRole, ThingDocument};
    for inserted in [false, true] {
        let mut plan = PlanBuilder::new();
        let account = if inserted {
            let account = plan.insert(entity("account"));
            plan.thing_field(account, field("id"), string("account-a"));
            account
        } else {
            plan.exact(entity("account"))
        };
        plan.fetch(account, vec![]);
        let built = plan.build(&schema(), semantics()).unwrap();
        let mut spec = PlanSpec {
            bindings: bindings(&["account"]),
            inputs: vec![],
            given_rows: None,
            operation: if inserted {
                Operation::Write {
                    returns: WriteReturns::Rows,
                }
            } else {
                Operation::Rows
            },
            pipeline: if inserted {
                vec![Stage::Insert {
                    things: vec![InsertThing {
                        binding: account,
                        type_id: entity("account"),
                        fields: vec![(field("id"), string("account-a"))],
                        players: vec![],
                    }],
                }]
            } else {
                vec![Stage::Match {
                    patterns: vec![pat(0, entity("account"))],
                }]
            },
            output: QueryOutput::Rows { columns: vec![] },
            managed_semantics: semantics(),
            compatibility: None,
            functions: vec![],
        };
        spec.pipeline
            .push(Stage::Fetch(ThingDocument::new(account, vec![])));
        assert_eq!(
            built.canonical_bytes().unwrap(),
            QueryPlanV3::new(spec, &schema())
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
        assert_eq!(
            built.operation(),
            if inserted {
                Operation::Write {
                    returns: WriteReturns::Rows,
                }
            } else {
                Operation::Rows
            }
        );
    }
    let mut plan = PlanBuilder::new();
    let purchase = plan.exact(purchase_type());
    let roles = vec![
        DocumentRole::new(RoleId::new("purchase", "buyer").unwrap(), true),
        DocumentRole::new(RoleId::new("purchase", "item").unwrap(), false),
    ];
    plan.fetch(purchase, roles.clone());
    assert_eq!(
        plan.build(&schema(), semantics())
            .unwrap()
            .pipeline()
            .last(),
        Some(&Stage::Fetch(ThingDocument::new(purchase, roles)))
    );
}

#[test]
fn builder_fetch_conflicts_in_both_orders_including_empty_returning() {
    let terminals: [fn(&mut PlanBuilder); 3] = [
        |p| {
            p.returning(vec![]);
        },
        |p| {
            p.count();
        },
        |p| {
            p.exists();
        },
    ];
    for terminal in terminals {
        for fetch_first in [false, true] {
            let mut plan = PlanBuilder::new();
            let account = plan.exact(entity("account"));
            if fetch_first {
                plan.fetch(account, vec![]);
            }
            terminal(&mut plan);
            if !fetch_first {
                plan.fetch(account, vec![]);
            }
            assert_eq!(
                plan.build(&schema(), semantics())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "query_plan_builder_terminal_conflict"
            );
        }
    }
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    plan.fetch(account, vec![]).fetch(account, vec![]);
    assert_eq!(
        plan.build(&schema(), semantics())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_builder_fetch_once"
    );
}

#[test]
fn builder_fetch_then_delete_snapshots_but_delete_then_fetch_stays_invalid() {
    for fetch_first in [false, true] {
        let mut plan = PlanBuilder::new();
        let account = plan.exact(entity("account"));
        if fetch_first {
            plan.fetch(account, vec![]);
        }
        plan.delete(account);
        if !fetch_first {
            plan.fetch(account, vec![]);
        }
        let built = plan.build(&schema(), semantics());
        if fetch_first {
            let built = built.unwrap();
            assert!(matches!(built.pipeline()[1], Stage::Fetch(_)));
            assert!(matches!(built.pipeline()[2], Stage::Delete { .. }));
        } else {
            assert!(built.is_err());
        }
    }
}

#[test]
fn builder_fetch_remains_terminal_when_configured_before_a_write() {
    for prior_write in [false, true] {
        let mut plan = PlanBuilder::new();
        let account = plan.exact(entity("account"));
        if prior_write {
            let peer = plan.exact(entity("account"));
            plan.update(peer, field("score"), literal(3));
        }
        plan.fetch(account, vec![])
            .update(account, field("score"), literal(7));
        let built = plan.build(&schema(), semantics()).unwrap();
        assert!(
            matches!(&built.pipeline()[1], Stage::Update { sets } if sets.len() == 1 + usize::from(prior_write))
        );
        assert!(matches!(built.pipeline()[2], Stage::Fetch(_)));
        assert_eq!(
            built.operation(),
            Operation::Write {
                returns: WriteReturns::Rows
            }
        );
    }
}

#[test]
fn bulk_inputs_equal_explicit_rows_and_preserve_row_order() {
    fn bulk(implicit: bool, rows: Vec<Vec<CanonicalValue>>) -> QueryPlanV3 {
        let mut plan = PlanBuilder::new();
        let account = plan.insert(entity("account"));
        let operands = if implicit {
            plan.bind_rows(
                vec![
                    (Site::new(account, "id"), ValueTypeTag::String),
                    (Site::new(account, "score"), ValueTypeTag::Long),
                ],
                rows,
            )
        } else {
            let columns = vec![
                QueryOperand::Input {
                    column: plan.input("account_id", ValueTypeTag::String, false),
                },
                QueryOperand::Input {
                    column: plan.input("account_score", ValueTypeTag::Long, false),
                },
            ];
            for row in rows {
                plan.row(row.into_iter().map(Some).collect());
            }
            columns
        };
        plan.thing_field(account, field("id"), operands[0].clone())
            .thing_field(account, field("score"), operands[1].clone());
        plan.build(&schema(), semantics()).unwrap()
    }
    let rows = vec![
        vec![text("account-1").unwrap(), CanonicalValue::Long(1)],
        vec![text("account-2").unwrap(), CanonicalValue::Long(2)],
    ];
    let implicit = bulk(true, rows.clone());
    let explicit = bulk(false, rows);
    assert_eq!(
        implicit.canonical_bytes().unwrap(),
        explicit.canonical_bytes().unwrap()
    );
}

#[test]
fn bulk_inputs_refuse_ragged_rows_mixed_forms_and_bad_value_types() {
    for case in [
        "ragged",
        "second_bulk",
        "scalar_after",
        "scalar_before",
        "explicit_before",
        "explicit_after",
        "wrong_type",
        "unknown_site",
        "empty",
    ] {
        let mut plan = PlanBuilder::new();
        let account = plan.insert(entity("account"));
        if case == "scalar_before" {
            plan.bind_value(Site::new(account, "id"), text("account-1").unwrap());
        }
        if case == "explicit_before" {
            plan.input("id", ValueTypeTag::String, false);
        }
        let columns = if case == "unknown_site" {
            vec![(Site::new(id(99), "id"), ValueTypeTag::String)]
        } else {
            vec![(Site::new(account, "id"), ValueTypeTag::String)]
        };
        let rows = match case {
            "ragged" => vec![vec![]],
            "empty" => vec![],
            "wrong_type" => vec![vec![CanonicalValue::Long(1)]],
            _ => vec![vec![text("account-1").unwrap()]],
        };
        let operands = plan.bind_rows(columns.clone(), rows.clone());
        if case == "second_bulk" {
            plan.bind_rows(columns, rows);
        }
        if case == "scalar_after" {
            plan.bind_value(Site::new(account, "id"), text("account-1").unwrap());
        }
        if case == "explicit_after" {
            plan.row(vec![text("account-1")]);
        }
        if let Some(operand) = operands.first() {
            plan.thing_field(account, field("id"), operand.clone());
        }
        let error = plan.build(&schema(), semantics()).unwrap_err();
        if case == "empty" {
            assert_eq!(error.code().as_str(), "query_invocation_missing_inputs");
        } else if case == "wrong_type" {
            assert_eq!(error.code().as_str(), "query_invocation_value_type");
        } else {
            assert_eq!(
                error.code().as_str(),
                match case {
                    "ragged" => "plan_builder_row_arity",
                    "unknown_site" => "plan_builder_unknown_site",
                    _ => "plan_builder_mixed_inputs",
                }
            );
        }
    }
}

#[test]
fn held_snapshot_terminal_matches_fetch_before_delete() {
    let make = |late| {
        let mut plan = PlanBuilder::new();
        let account = plan.exact(entity("account"));
        if late {
            plan.delete(account).fetch_held(account, vec![]);
        } else {
            plan.fetch(account, vec![]).delete(account);
        }
        plan.build(&schema(), semantics()).unwrap()
    };
    assert_eq!(
        make(true).canonical_bytes().unwrap(),
        make(false).canonical_bytes().unwrap()
    );
}

#[test]
fn generated_assignment_names_and_iid_use_core_validation() {
    let mut plan = PlanBuilder::new();
    let account = plan.exact(entity("account"));
    let score = plan.field(account, attribute("score"));
    let value = plan.let_value(
        Site::new(account, "score"),
        sub(QueryOperand::Binding { binding: score }, literal(1)),
    );
    plan.update(
        account,
        field("score"),
        QueryOperand::Binding { binding: value },
    );
    assert_eq!(
        plan.build(&schema(), semantics()).unwrap().bindings()[2]
            .variable()
            .as_str(),
        "account_score_2"
    );
    plan.by_iid(account, "bad iid");
    assert_eq!(
        plan.build(&schema(), semantics())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_invalid_iid"
    );
}

#[test]
fn model_bulk_groups_shapes_splits_limits_and_preserves_explicit_bytes() {
    use type_bridge_contract::split::SplitLimits;
    let row = |name: &str, score: Option<i64>| {
        let mut row = vec![(field("id"), text(name).unwrap())];
        if let Some(score) = score {
            row.push((field("score"), CanonicalValue::Long(score)));
        }
        row
    };
    let mut p = PlanBuilder::new();
    let account = p.insert(entity("account"));
    p.bind_model_rows(
        account,
        vec![
            row("a", Some(1)),
            row("b", None),
            row("c", Some(2)),
            row("d", Some(3)),
        ],
    )
    .fetch_held(account, vec![]);
    let built = p
        .build_plans(
            &schema(),
            semantics(),
            false,
            SplitLimits::new(2, usize::MAX),
        )
        .unwrap();
    assert_eq!(built.plans().len(), 3);
    assert_eq!(
        built
            .bulk_chunks()
            .unwrap()
            .map(|group| group.positions.to_vec())
            .collect::<Vec<_>>(),
        vec![vec![0, 2], vec![3], vec![1]]
    );
    let mut explicit = PlanBuilder::new();
    let account = explicit.insert(entity("account"));
    let id_value = explicit.input("account_id", ValueTypeTag::String, false);
    let score_value = explicit.input("account_score", ValueTypeTag::Long, false);
    explicit
        .thing_field(
            account,
            field("id"),
            QueryOperand::Input { column: id_value },
        )
        .thing_field(
            account,
            field("score"),
            QueryOperand::Input {
                column: score_value,
            },
        )
        .fetch(account, vec![]);
    explicit
        .row(vec![text("a"), Some(CanonicalValue::Long(1))])
        .row(vec![text("c"), Some(CanonicalValue::Long(2))]);
    assert_eq!(
        built.plans()[0].canonical_bytes().unwrap(),
        explicit
            .build(&schema(), semantics())
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );
}

#[test]
fn empty_bulk_has_no_plans_and_zero_field_rows_are_single_plans() {
    use type_bridge_contract::split::SplitLimits;
    for count in [0, 3] {
        let mut p = PlanBuilder::new();
        let thing = p.insert(entity("other"));
        p.bind_model_rows(thing, vec![vec![]; count])
            .fetch_held(thing, vec![]);
        let built = p
            .build_plans(&schema(), semantics(), false, SplitLimits::CANONICAL)
            .unwrap();
        assert_eq!(built.plans().len(), count);
        assert_eq!(built.bulk_chunks().unwrap().len(), count);
        for (position, plan) in built.plans().iter().enumerate() {
            assert_eq!(plan.given_rows(), None);
            assert_eq!(
                built
                    .bulk_chunks()
                    .unwrap()
                    .nth(position)
                    .unwrap()
                    .positions,
                vec![position]
            );
        }
    }
}

#[test]
fn model_bulk_refuses_mixed_programs_and_wrong_domains() {
    use type_bridge_contract::split::SplitLimits;
    for mixed in [false, true] {
        let mut p = PlanBuilder::new();
        let account = p.insert(entity("account"));
        p.bind_model_rows(account, vec![vec![(field("id"), CanonicalValue::Long(1))]]);
        if mixed {
            p.insert(entity("other"));
        }
        let error = p
            .build_plans(&schema(), semantics(), false, SplitLimits::CANONICAL)
            .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            if mixed {
                "plan_builder_mixed_inputs"
            } else {
                "query_plan_write_value_type"
            }
        );
    }
}

#[test]
fn model_bulk_split_honours_the_recordings_selected_query_limits() {
    use type_bridge_contract::limits::QueryLimits;
    use type_bridge_contract::split::SplitLimits;
    let build = |limits| {
        let mut builder = PlanBuilder::with_limits(limits);
        let account = builder.insert(entity("account"));
        builder
            .bind_model_rows(
                account,
                vec![
                    vec![(field("id"), text("a").unwrap())],
                    vec![(field("id"), text("b").unwrap())],
                ],
            )
            .fetch_held(account, vec![]);
        builder
    };
    let reference = build(QueryLimits::CANONICAL)
        .build_plans(
            &schema(),
            semantics(),
            false,
            SplitLimits::new(1, usize::MAX),
        )
        .unwrap();
    for selected in ["rows", "canonical_bytes", "input_bytes"] {
        let mut limits = QueryLimits::CANONICAL;
        match selected {
            "rows" => limits.structural.input_rows = 1,
            "canonical_bytes" => {
                limits.codec.max_bytes = reference.plans()[0].canonical_bytes().unwrap().len();
                limits.codec.max_string_bytes = limits.codec.max_bytes;
            }
            _ => {
                limits.structural.input_bytes =
                    serde_json::to_vec(reference.plans()[0].given_rows().unwrap())
                        .unwrap()
                        .len()
            }
        }
        let result =
            build(limits).build_plans(&schema(), semantics(), false, SplitLimits::CANONICAL);
        assert!(
            result.is_ok(),
            "selected {selected} must split, not refuse: {:?}",
            result.as_ref().err()
        );
        let result = result.unwrap();
        assert_eq!(result.plans().len(), 2, "selected {selected}");
        for (actual, expected) in result.plans().iter().zip(reference.plans()) {
            assert_eq!(
                actual.canonical_bytes().unwrap(),
                expected.canonical_bytes().unwrap()
            );
        }
    }
}

#[test]
fn configured_binding_policy_owns_wide_authoring_and_wire_revalidation() {
    use type_bridge_contract::limits::QueryLimits;
    use type_bridge_contract::query_plan::v3::{
        decode_query_plan_v3, decode_query_plan_v3_with_limits,
    };
    let mut limits = QueryLimits::V3;
    limits.structural.bindings = 512;
    limits.structural.boolean_terms = 512;
    let mut builder = PlanBuilder::with_limits(limits);
    for _ in 0..300 {
        builder.exact(entity("account"));
    }
    builder.count();
    let wide = builder.build(&schema(), semantics()).unwrap();
    assert_eq!(wide.bindings().len(), 300);
    let bytes = wide.canonical_bytes().unwrap();
    assert_eq!(
        decode_query_plan_v3_with_limits(&bytes, &schema(), limits)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
    assert_eq!(
        decode_query_plan_v3(&bytes, &schema())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_binding_limit"
    );
    let mut default = PlanBuilder::new();
    for _ in 0..257 {
        default.exact(entity("account"));
    }
    default.count();
    assert_eq!(
        default
            .build(&schema(), semantics())
            .unwrap_err()
            .code()
            .as_str(),
        "query_plan_builder_binding_limit"
    );
}
