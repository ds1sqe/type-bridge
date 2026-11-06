//! Native model-plan adaptation, hydration and result regression tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
};
use crate::_entity::Annotation;
use crate::_registry::DescriptorRegistry;
use crate::AttributeValue;
use crate::ValueType;
use crate::match_request::descriptor_plan::MatchRequestAdapterAuthority;
use crate::match_request::{
    BindingId as V1BindingId, BindingPair, BoundFieldId, ComparisonOp, DescriptorId, FetchShape,
    FetchSlot, FieldId, MatchBinding, MatchExpr, MatchMode, MatchOperation, MatchOrder, MatchPlan,
    MatchRequest, MissingOrder, RoleEdgeId, RoleId as V1RoleId, RowCardinality, SortDirection,
    ThingKind, Window, validate_match_request,
};
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::query_plan::{QueryOutput, QueryPattern};
use type_bridge_contract::value::ValueTypeTag;

pub(crate) fn registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                field_name: "name".to_owned(),
                attr_name: "person-name".to_owned(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Key],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            doc: None,
            meta: Default::default(),
        })
        .expect("register person");
    registry
        .register_relation(RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: Vec::new(),
            roles: vec![RoleDescriptor {
                role_name: "worker".into(),
                player_type_names: vec!["person".into()],
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
        .expect("register employment");
    registry
        .register_relation(RelationDescriptor {
            type_name: "directed-edge".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: Vec::new(),
            roles: vec![
                RoleDescriptor {
                    role_name: "origin".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "destination".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
        .expect("register directed edge");
    let inherited_participant = RoleDescriptor {
        role_name: "participant".into(),
        player_type_names: vec!["person".into()],
        ..Default::default()
    };
    registry
        .register_relation(RelationDescriptor {
            type_name: "interaction".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: Vec::new(),
            roles: vec![inherited_participant.clone()],
            doc: None,
            meta: Default::default(),
        })
        .expect("register parent interaction");
    registry
        .register_relation(RelationDescriptor {
            type_name: "assignment".into(),
            is_abstract: false,
            parent_type: Some("interaction".into()),
            owned_attributes: Vec::new(),
            // Runtime descriptors are effective projections, so inherited
            // roles remain present on the child with identical semantics.
            roles: vec![inherited_participant],
            doc: None,
            meta: Default::default(),
        })
        .expect("register child assignment");
    registry
}

pub(crate) fn name_field() -> BoundFieldId {
    name_field_for(0)
}

pub(crate) fn name_field_for(binding: u16) -> BoundFieldId {
    BoundFieldId::new(
        V1BindingId::new(binding),
        FieldId::new(DescriptorId::new("entity:person"), "name"),
    )
}

pub(crate) fn representative_plan() -> MatchPlan {
    MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Subtypes,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("relation:employment"),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::And {
            expressions: vec![
                MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(0),
                    relation: V1BindingId::new(1),
                    role: V1RoleId::new(DescriptorId::new("relation:employment"), "worker"),
                    player: V1BindingId::new(0),
                },
                MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::GreaterThanOrEqual,
                    value: AttributeValue::String("A".to_owned()),
                },
                MatchExpr::Not {
                    expression: Box::new(MatchExpr::FieldValue {
                        field: name_field(),
                        operator: ComparisonOp::Equal,
                        value: AttributeValue::String("blocked".to_owned()),
                    }),
                },
            ],
        }),
        allowed_cross_joins: BTreeSet::new(),
    }
}

pub(crate) fn fetch_rows_operation() -> MatchOperation {
    MatchOperation::FetchRows {
        output: FetchShape::Positional {
            slots: vec![FetchSlot::One {
                binding: V1BindingId::new(0),
            }],
        },
        order: vec![MatchOrder {
            field: name_field(),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        }],
        window: Window {
            offset: 0,
            limit: 5,
        },
        cardinality: RowCardinality::BoundedMany,
    }
}

#[test]
fn native_read_stacks_cover_rows_pages_counts_and_exists_without_compatibility() {
    let registry = registry();
    let output = FetchShape::Positional {
        slots: vec![FetchSlot::One {
            binding: V1BindingId::new(0),
        }],
    };
    for operation in [
        fetch_rows_operation(),
        MatchOperation::PageBy {
            root: V1BindingId::new(0),
            output,
            order: vec![],
            window: Window {
                offset: 1,
                limit: 2,
            },
            include_total: true,
        },
        MatchOperation::CountBy {
            root: V1BindingId::new(0),
        },
        MatchOperation::ExistsBy {
            root: V1BindingId::new(0),
        },
    ] {
        let request = validate_match_request(
            &registry,
            MatchRequest::v1(representative_plan(), operation),
        )
        .unwrap();
        let stack =
            crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
        assert!(!stack.plans.is_empty());
        for plan in &stack.plans {
            assert!(!plan.writes());
            crate::query_plan_exec::lower(plan).unwrap();
        }
    }
}

fn native_stack_answers(
    stack: &crate::match_request::plan_adapter::AdaptedMatchRequest,
) -> Vec<crate::query_plan_exec::PlanAnswer> {
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::query_plan::{Operation, Stage};
    use type_bridge_contract::query_remote::RemoteValue;
    stack.plans.iter().map(|plan| {
        if let Some(Stage::Fetch(document)) = plan.pipeline().last() {
            return PlanAnswer::Documents(vec![if document.thing.get() == 0 {
                serde_json::json!({"_iid":"0x01","_type":"person","attributes":{"person-name":"Alice"}})
            } else {
                serde_json::json!({"_iid":"0x02","_type":"employment","attributes":{},"_role_0_iid":"0x01","_role_0_type":"person","_role_0_attributes":{"person-name":"Alice"}})
            }]);
        }
        if plan.operation() == Operation::Exists { return PlanAnswer::Exists(true); }
        let reduced = plan.pipeline().iter().any(|stage| matches!(stage, Stage::Reduce { .. }));
        let type_bridge_contract::query_plan::QueryOutput::Rows { columns } = plan.output() else { panic!("rows"); };
        let row = columns.iter().map(|column| {
            let value = if reduced { RemoteValue::Value { value: type_bridge_contract::value::CanonicalValue::Long(1) } }
                else { RemoteValue::Thing { iid: if column.get() == 0 { "0x01" } else { "0x02" }.into(), type_id: TypeId::new(if column.get() == 0 { TypeKind::Entity } else { TypeKind::Relation }, if column.get() == 0 { "person" } else { "employment" }).unwrap() } };
            (format!("b{}",column.get()), serde_json::to_value(value).unwrap())
        }).collect::<serde_json::Map<_,_>>();
        PlanAnswer::Rows(vec![serde_json::Value::Object(row)])
    }).collect()
}

#[test]
fn native_stacks_preserve_hydrated_result_proofs_and_refuse_window_mismatches() {
    let registry = registry();
    let policy = crate::QueryExecutionResourceLimits::default();
    for page in [false, true] {
        let operation = if page {
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: V1BindingId::new(0),
                    }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 5,
                },
                include_total: true,
            }
        } else {
            fetch_rows_operation()
        };
        let request = validate_match_request(
            &registry,
            MatchRequest::v1(representative_plan(), operation),
        )
        .unwrap();
        let stack =
            crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
        let answers = native_stack_answers(&stack);
        let result =
            crate::match_request::result_validation::validated_match_result_from_plan_stack(
                &registry, &request, &stack, &answers, policy,
            )
            .unwrap();
        match result.for_request(&request).unwrap() {
            crate::match_request::MatchResult::Rows { rows } => assert_eq!(rows.len(), 1),
            crate::match_request::MatchResult::Page { entries, total, .. } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(*total, Some(1));
            }
            _ => panic!("hydrated result"),
        }
        let crate::match_request::plan_adapter::Layout::Fetch {
            selection,
            mapping,
            hydration,
            ..
        } = &stack.layout
        else {
            panic!("static stack");
        };
        if page {
            assert_eq!(stack.plans.len(), 3);
        }
        let selection = selection.or(*mapping).expect("selection or mapping rows");
        let mut missing = answers.clone();
        missing[hydration[0].0] = crate::query_plan_exec::PlanAnswer::Documents(vec![]);
        assert_eq!(
            crate::match_request::plan_result::resolve(&stack, &request, &missing, policy)
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_result_window"
        );
        let mut unexpected = answers.clone();
        let crate::query_plan_exec::PlanAnswer::Rows(rows) = &mut unexpected[selection] else {
            panic!("selection");
        };
        rows[0]["b0"]["iid"] = serde_json::json!("0x09");
        assert_eq!(
            crate::match_request::plan_result::resolve(&stack, &request, &unexpected, policy)
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_result_window"
        );
    }
}

#[test]
fn native_selected_relation_roles_recheck_correlated_hidden_player_identity() {
    let registry = registry();
    let mut plan = representative_plan();
    plan.predicate = Some(MatchExpr::RoleEdge {
        id: RoleEdgeId::new(0),
        relation: V1BindingId::new(1),
        role: V1RoleId::new(DescriptorId::new("relation:employment"), "worker"),
        player: V1BindingId::new(0),
    });
    let request = validate_match_request(
        &registry,
        MatchRequest::v1(
            plan,
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: V1BindingId::new(1),
                    }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 5,
                },
                cardinality: RowCardinality::Complete,
            },
        ),
    )
    .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
    let mut answers = native_stack_answers(&stack);
    let policy = crate::QueryExecutionResourceLimits::default();
    crate::match_request::plan_result::resolve(&stack, &request, &answers, policy).unwrap();
    let crate::match_request::plan_adapter::Layout::Fetch {
        mapping: Some(mapping),
        ..
    } = &stack.layout
    else {
        panic!("mapping");
    };
    let crate::query_plan_exec::PlanAnswer::Rows(rows) = &mut answers[*mapping] else {
        panic!("rows");
    };
    rows[0]["b0"]["iid"] = serde_json::json!("0x03");
    assert_eq!(
        crate::match_request::plan_result::resolve(&stack, &request, &answers, policy)
            .unwrap_err()
            .code()
            .as_str(),
        "query_v2_model_predicate_evidence"
    );
}

#[test]
fn native_zero_hop_reachability_uses_the_plan_binding_domains() {
    let registry = registry();
    let plan = MatchPlan {
        bindings: (0..2)
            .map(|id| MatchBinding {
                id: V1BindingId::new(id),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            })
            .collect(),
        predicate: Some(MatchExpr::Reachable {
            relation: DescriptorId::new("relation:directed-edge"),
            role_from: V1RoleId::new(DescriptorId::new("relation:directed-edge"), "origin"),
            role_to: V1RoleId::new(DescriptorId::new("relation:directed-edge"), "destination"),
            source: V1BindingId::new(0),
            target: V1BindingId::new(1),
            min_depth: 0,
            max_depth: 1,
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let request = validate_match_request(
        &registry,
        MatchRequest::v1(
            plan,
            MatchOperation::CountBy {
                root: V1BindingId::new(0),
            },
        ),
    )
    .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
    let lowered = crate::query_plan_exec::lower(&stack.plans[0]).unwrap();
    let typeql = type_bridge_core_lib::__internal::render(&lowered.query).unwrap();
    assert!(typeql.contains("isa! person"));
    assert!(typeql.contains("is $R"));
}

#[test]
fn native_relation_subtypes_with_extra_roles_get_exact_complete_hydration_plans() {
    let registry = registry();
    let mut roles = registry.relation("employment").unwrap().roles.clone();
    roles.push(RoleDescriptor {
        role_name: "mentor".into(),
        player_type_names: vec!["person".into()],
        ..Default::default()
    });
    registry
        .register_relation(RelationDescriptor {
            type_name: "special-employment".into(),
            is_abstract: false,
            parent_type: Some("employment".into()),
            owned_attributes: vec![],
            roles,
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    let request = validate_match_request(
        &registry,
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: V1BindingId::new(0),
                    descriptor: DescriptorId::new("relation:employment"),
                    thing_kind: ThingKind::Relation,
                    match_mode: MatchMode::Subtypes,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: V1BindingId::new(0),
                    }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 5,
                },
                cardinality: RowCardinality::Complete,
            },
        ),
    )
    .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
    let mut sizes = BTreeMap::new();
    for plan in &stack.plans {
        if let Some(type_bridge_contract::query_plan::Stage::Fetch(document)) =
            plan.pipeline().last()
        {
            let types = plan
                .fetch_document_shape()
                .unwrap()
                .type_labels()
                .collect::<Vec<_>>();
            assert_eq!(types.len(), 1);
            sizes.insert(types[0].to_owned(), document.roles.len());
        }
    }
    assert_eq!(
        sizes,
        BTreeMap::from([("employment".into(), 1), ("special-employment".into(), 2)])
    );
}

#[cfg(feature = "typedb")]
#[tokio::test]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn native_typed_read_stacks_match_live_rows_pages_counts_and_subtype_roles() {
    use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
    use crate::session::backend::{AnswerCancellation, TxType};
    use crate::session::database::Database;
    use crate::session::real_driver::ConnectOptions;
    println!("NATIVE_READ_RUNNER pid={}", std::process::id());
    let database_name = crate::live_database::name("native");
    let options = ConnectOptions {
        http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
        ..Default::default()
    };
    let db = Database::connect_with_options(
        &std::env::var("TYPEDB_ADDRESS").unwrap(),
        &database_name,
        "admin",
        "password",
        options,
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    let execution = crate::match_request::selected_result_executor::catch_execution_unwind(async {
        db.execute_raw("define attribute person-name, value string; entity person, owns person-name @key, plays employment:worker, plays special-employment:mentor, plays directed-edge:origin, plays directed-edge:destination, plays interaction:participant; relation employment, relates worker; relation special-employment sub employment, relates mentor; relation directed-edge, relates origin, relates destination; relation interaction, relates participant; relation assignment sub interaction; attribute score, value integer; attribute category, value string; attribute bucket, value string; entity measurement, owns score @card(0..1), owns category, owns bucket;", TxType::Schema).await.unwrap();
        db.execute_raw("insert $a isa person, has person-name \"Alice\"; $b isa person, has person-name \"Bob\"; $e1 isa employment, links(worker:$a); $e2 isa employment, links(worker:$b); $s isa special-employment, links(worker:$b,mentor:$a); $m1 isa measurement, has score 10, has category \"g\", has bucket \"all\"; $m2 isa measurement, has score 20, has category \"g\", has bucket \"all\"; $m3 isa measurement, has category \"x\", has bucket \"all\"; $m4 isa measurement, has score 40, has category \"y\", has bucket \"all\";", TxType::Write).await.unwrap();
        for (columns, given, expected) in [(&[0, 1][..], true, 1), (&[0, 1][..], false, 2), (&[0, 1, 3][..], false, 4)] {
            use type_bridge_contract::migration_assertion::BindingId;
            let (models, selection) = generic_model_selection(columns, given);
            let slots = columns.iter().filter(|id| **id != 1).map(|id| (BindingId::new(*id).unwrap(), DescriptorId::new("entity:person"))).collect::<Vec<_>>();
            let stack = crate::match_request::build_plan_fetch_stack(&models, &selection, &slots).unwrap();
            let policy = stack.execution_limits(QueryExecutionResourceLimits::default());
            let answers = db.execute_plans(stack.plans(), &policy, &AnswerCancellation::default(), QueryExecutionDeadline::for_limits(policy)).await.unwrap();
            let result = stack.resolve(&models, &answers, policy).unwrap();
            assert_eq!(result.model_rows().len(), expected);
            assert_eq!(result.validation_count(), stack.plans().len());
            let type_bridge_contract::query_remote::StagedAnswer::Rows {rows} = result.selection().to_staged_v3(stack.selection_plan()).unwrap() else { panic!("generic selection rows"); };
            assert!(rows.iter().all(|row| row.iter().any(|value| matches!(value,type_bridge_contract::query_remote::RemoteValue::Value {value:type_bridge_contract::value::CanonicalValue::Long(7)}))));
            println!("GENERIC_NATIVE_READ_OK given={given} models={} rows={expected} plans={}",slots.len(),stack.plans().len());
        }
        let registry = registry();
        registry.register_entity(EntityDescriptor { type_name: "measurement".into(), is_abstract: false, parent_type: None, owned_attributes: vec![
            OwnedAttributeDescriptor { field_name: "score".into(), attr_name: "score".into(), value_type: ValueType::Long, annotations: vec![], is_optional: true, is_ordered: false, doc: None, meta: Default::default() },
            OwnedAttributeDescriptor { field_name: "bucket".into(), attr_name: "bucket".into(), value_type: ValueType::String, annotations: vec![], is_optional: false, is_ordered: false, doc: None, meta: Default::default() },
            OwnedAttributeDescriptor { field_name: "category".into(), attr_name: "category".into(), value_type: ValueType::String, annotations: vec![], is_optional: false, is_ordered: false, doc: None, meta: Default::default() },
        ], doc: None, meta: Default::default() }).unwrap();
        let measure_plan = MatchPlan { bindings: vec![MatchBinding { id: V1BindingId::new(0), descriptor: DescriptorId::new("entity:measurement"), thing_kind: ThingKind::Entity, match_mode: MatchMode::Exact }], predicate: None, allowed_cross_joins: BTreeSet::new() };
        let reducers = [crate::match_request::Reduction::Count, crate::match_request::Reduction::Sum, crate::match_request::Reduction::Min, crate::match_request::Reduction::Max, crate::match_request::Reduction::Mean, crate::match_request::Reduction::Median, crate::match_request::Reduction::Std].into_iter().map(|reduction| crate::match_request::ReduceTerm { reduction, input: reduction.requires_input().then(|| BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "score"))) }).collect::<Vec<_>>();
        let category = BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "category"));
        let mut roles = registry.relation("employment").unwrap().roles.clone();
        roles.push(RoleDescriptor { role_name: "mentor".into(), player_type_names: vec!["person".into()], ..Default::default() });
        registry.register_relation(RelationDescriptor { type_name: "special-employment".into(), is_abstract: false, parent_type: Some("employment".into()), owned_attributes: vec![], roles, doc: None, meta: Default::default() }).unwrap();
        let output = FetchShape::Positional { slots: vec![FetchSlot::One { binding: V1BindingId::new(0) }] };
        let subtype_plan = MatchPlan { bindings: vec![MatchBinding { id: V1BindingId::new(0), descriptor: DescriptorId::new("relation:employment"), thing_kind: ThingKind::Relation, match_mode: MatchMode::Subtypes }], predicate: None, allowed_cross_joins: BTreeSet::new() };
        let string_plan = |operator, value: &str| {
            let mut plan = representative_plan();
            plan.predicate = Some(MatchExpr::And { expressions: vec![plan.predicate.take().unwrap(), MatchExpr::FieldValue { field: BoundFieldId::new(V1BindingId::new(0),FieldId::new(DescriptorId::new("entity:person"),"name")), operator, value: AttributeValue::String(value.into()) }] });
            plan
        };
        let hidden_field_plan = MatchPlan {bindings:(0..2).map(|id| MatchBinding {id:V1BindingId::new(id),descriptor:DescriptorId::new("entity:person"),thing_kind:ThingKind::Entity,match_mode:MatchMode::Exact}).collect(),predicate:Some(MatchExpr::FieldValue {field:name_field_for(1),operator:ComparisonOp::Equal,value:AttributeValue::String("Bob".into())}),allowed_cross_joins:BTreeSet::from([BindingPair::new(V1BindingId::new(0),V1BindingId::new(1))])};
        let raw_float_statistics = db.execute_raw("match $m isa measurement, has score $score; let $double_score = $score * 1.0; reduce $deviation = std($double_score);", TxType::Read).await.unwrap();
        println!("NATIVE_PROMOTED_STD_10_20_40 {raw_float_statistics:?}");
        for (label, plan, operation) in [
            ("rows", representative_plan(), fetch_rows_operation()),
            ("hidden-field", hidden_field_plan, fetch_rows_operation()),
            ("contains", string_plan(ComparisonOp::Contains,"lic"), fetch_rows_operation()),
            ("starts", string_plan(ComparisonOp::StartsWith,"Ali"), fetch_rows_operation()),
            ("ends", string_plan(ComparisonOp::EndsWith,"ice"), fetch_rows_operation()),
            ("regex", string_plan(ComparisonOp::Regex,"^Ali.*"), fetch_rows_operation()),
            ("page-total", representative_plan(), MatchOperation::PageBy { root: V1BindingId::new(0), output: output.clone(), order: vec![], window: Window { offset: 1, limit: 1 }, include_total: true }),
            ("count", representative_plan(), MatchOperation::CountBy { root: V1BindingId::new(0) }),
            ("exists", representative_plan(), MatchOperation::ExistsBy { root: V1BindingId::new(0) }),
            ("reduce", measure_plan.clone(), MatchOperation::ReduceBy { root: V1BindingId::new(0), group: None, reducers: reducers.clone() }),
            ("reduce-field", measure_plan.clone(), MatchOperation::ReduceByField { root: V1BindingId::new(0), group: category.clone(), reducers: reducers.clone() }),
            ("reduce-fields", measure_plan, MatchOperation::ReduceByFields { root: V1BindingId::new(0), groups: vec![category, BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "bucket"))], reducers }),
            ("subtype-roles", subtype_plan, MatchOperation::FetchRows { output, order: vec![], window: Window { offset: 0, limit: 10 }, cardinality: RowCardinality::Complete }),
        ] {
            let request = validate_match_request(&registry, MatchRequest::v1(plan, operation)).unwrap();
            let stack = crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
            if label.starts_with("reduce") { assert_eq!(stack.plans.len(), 2); }
            let policy = QueryExecutionResourceLimits::default().for_adapter_statements(stack.plans.len());
            if stack.plans.len() > 3 {
                let strict = QueryExecutionResourceLimits { statements: Some(3), ..Default::default() };
                let error = db.execute_plans(&stack.plans, &strict, &AnswerCancellation::default(), QueryExecutionDeadline::for_limits(strict)).await.unwrap_err();
                assert!(matches!(error, crate::OrmError::Match(error) if error.code().as_str() == "statement_count_limit"));
            }
            let answers = db.execute_plans(&stack.plans, &policy, &AnswerCancellation::default(), QueryExecutionDeadline::for_limits(policy)).await.unwrap_or_else(|error| panic!("{label}: {error:?}"));
            let result = crate::match_request::result_validation::validated_match_result_from_plan_stack(&registry, &request, &stack, &answers, policy).unwrap_or_else(|error| panic!("{label}: {error:?}"));
            let facade = db.execute_match(&registry, &request).await.unwrap_or_else(|error| panic!("{label} facade: {error:?}"));
            assert_eq!(facade.for_request(&request).unwrap(), result.for_request(&request).unwrap());
            match (label, result.for_request(&request).unwrap()) {
                ("rows" | "hidden-field", crate::match_request::MatchResult::Rows { rows }) => assert_eq!(rows.len(), 2),
                ("contains" | "starts" | "ends" | "regex", crate::match_request::MatchResult::Rows { rows }) => assert_eq!(rows.len(),1),
                ("page-total", crate::match_request::MatchResult::Page { entries, total, .. }) => { assert_eq!(entries.len(), 1); assert_eq!(*total, Some(2)); },
                ("count", crate::match_request::MatchResult::Count { value, .. }) => assert_eq!(*value, 2),
                ("exists", crate::match_request::MatchResult::Exists { value, .. }) => assert!(*value),
                ("reduce", crate::match_request::MatchResult::Reduction { rows, .. }) => {
                    assert_eq!(rows.len(), if label == "reduce" { 1 } else { 3 });
                    assert_eq!(rows[0].values()[1], crate::match_request::ReducedValue::Long(Some(70)));
                    use crate::match_request::ReducedValue;
                    assert_eq!(rows[0].values()[0], ReducedValue::Count(4));
                    assert_eq!(rows[0].values()[2], ReducedValue::Long(Some(10)));
                    assert_eq!(rows[0].values()[3], ReducedValue::Long(Some(40)));
                    for (index, expected) in [(4, 70.0_f64 / 3.0), (5, 20.0), (6, f64::from_bits(4624788820634841821))] {
                        let ReducedValue::Double(Some(actual)) = rows[0].values()[index] else { panic!("missing reducer {index}"); };
                        assert_eq!(actual.to_bits(), expected.to_bits(), "reducer {index}");
                    }
                }
                ("reduce-field", crate::match_request::MatchResult::FieldReduction { rows, .. }) | ("reduce-fields", crate::match_request::MatchResult::FieldTupleReduction { rows, .. }) => {
                    assert_eq!(rows.len(), 3);
                    assert_eq!(rows[0].values()[1], crate::match_request::ReducedValue::Long(Some(30)));
                }
                ("subtype-roles", crate::match_request::MatchResult::Rows { rows }) => {
                    assert_eq!(rows.len(), 3);
                    assert!(rows.iter().any(|row| matches!(&row.slots()[0], crate::match_request::SlotValue::One(thing) if thing.concrete_descriptor().as_str() == "relation:special-employment" && thing.roles().len() == 2)));
                }
                _ => panic!("{label}: wrong result kind"),
            }
            println!("NATIVE_READ_OK shape={label} plans={}", stack.plans.len());
        }

        db.execute_raw("insert $m1 isa measurement, has score 9223372036854775807, has category \"overflow\", has bucket \"all\"; $m2 isa measurement, has score 9223372036854775807, has category \"overflow\", has bucket \"all\";", TxType::Write).await.unwrap();
        db.execute_raw("insert $m1 isa measurement, has score -9223372036854775808, has category \"negative-overflow\", has bucket \"all\"; $m2 isa measurement, has score -9223372036854775808, has category \"negative-overflow\", has bucket \"all\"; $a isa measurement, has score -9223372036854775808, has category \"halves\", has bucket \"all\"; $b isa measurement, has score -9223372036854775807, has category \"halves\", has bucket \"all\"; $c isa measurement, has score -1, has category \"halves\", has bucket \"all\"; $d isa measurement, has score 9223372036854775807, has category \"halves\", has bucket \"all\";", TxType::Write).await.unwrap();
        let halves = db.execute_raw("match $m isa measurement, has category \"halves\", has score $score; let $low = $score % 4294967296; let $upper = $score - $low; let $ratio = $upper / 4294967296; let $high = floor($ratio); select $score, $low, $upper, $ratio, $high; sort $score asc;", TxType::Read).await.unwrap();
        let crate::session::backend::QueryResult::Rows(rows) = halves else { panic!("native halves rows"); };
        println!("NATIVE_LONG_HALVES {}", serde_json::to_string(&rows).unwrap());
        assert_eq!(rows.len(), 4);
        let integer = |value: &serde_json::Value| value.as_i64().or_else(|| value.as_str().and_then(|value| value.parse().ok())).unwrap();
        for row in &rows {
            assert_eq!(row["ratio"]["value_type"], "double");
            assert_eq!(row["high"]["value_type"], "integer");
            let score = integer(&row["score"]["value"]);
            let low = integer(&row["low"]["value"]);
            let high = integer(&row["high"]["value"]);
            let upper = integer(&row["upper"]["value"]);
            assert!((0..(1_i64 << 32)).contains(&low));
            assert_eq!(low, score.rem_euclid(1_i64 << 32));
            assert_eq!(i128::from(high) * (1_i128 << 32) + i128::from(low), i128::from(score));
            assert_eq!(i128::from(upper), i128::from(score) - i128::from(low));
            assert_eq!(row["ratio"]["value"].as_f64().unwrap(), high as f64);
        }
        let mut native_statistics_valid = true;
        for (category, expected) in [("overflow", i64::MAX), ("negative-overflow", i64::MIN)] {
            let query = format!("match $m isa measurement, has category \"{category}\", has score $score; reduce $average = mean($score), $deviation = std($score), $middle = median($score), $minimum = min($score), $maximum = max($score);");
            let statistics = db.execute_raw(&query, TxType::Read).await.unwrap();
            let crate::session::backend::QueryResult::Rows(rows) = statistics else { panic!("native extreme statistics rows"); };
            println!("NATIVE_EXTREME_STATISTICS category={category} {}", serde_json::to_string(&rows).unwrap());
            native_statistics_valid &= rows[0]["average"]["value"].as_f64().unwrap() == expected as f64
                && rows[0]["deviation"]["value"].as_f64().unwrap() == 0.0;
            assert_eq!(integer(&rows[0]["minimum"]["value"]), expected);
            assert_eq!(integer(&rows[0]["maximum"]["value"]), expected);
            let query = format!("match $m isa measurement, has category \"{category}\", has score $score; let $double_score = $score * 1.0; reduce $average = mean($double_score), $deviation = std($double_score), $middle = median($double_score);");
            let statistics = db.execute_raw(&query, TxType::Read).await.unwrap();
            let crate::session::backend::QueryResult::Rows(rows) = statistics else { panic!("native promoted statistics rows"); };
            println!("NATIVE_EXTREME_FLOAT_STATISTICS category={category} {}", serde_json::to_string(&rows).unwrap());
            assert_eq!(rows[0]["average"]["value"].as_f64().unwrap(), expected as f64);
            assert_eq!(rows[0]["deviation"]["value"].as_f64().unwrap(), 0.0);
            assert_eq!(rows[0]["middle"]["value"].as_f64().unwrap(), expected as f64);
        }
        println!("NATIVE_LONG_STATISTICS_SAFE={native_statistics_valid}");
        let extreme_request = |category: &str, reductions: &[crate::match_request::Reduction]| {
            validate_match_request(&registry, MatchRequest::v1(MatchPlan {
                bindings: vec![MatchBinding { id: V1BindingId::new(0), descriptor: DescriptorId::new("entity:measurement"), thing_kind: ThingKind::Entity, match_mode: MatchMode::Exact }],
                predicate: Some(MatchExpr::FieldValue {
                    field: BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "category")),
                    operator: ComparisonOp::Equal, value: AttributeValue::String(category.into()),
                }), allowed_cross_joins: BTreeSet::new(),
            }, MatchOperation::ReduceBy { root: V1BindingId::new(0), group: None, reducers: reductions.iter().map(|reduction| crate::match_request::ReduceTerm {
                reduction: *reduction, input: Some(BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "score"))),
            }).collect() })).unwrap()
        };
        db.execute_raw("insert $m1 isa measurement, has score -9223372036854775808, has category \"mixed-extremes\", has bucket \"all\"; $m2 isa measurement, has score 9223372036854775807, has category \"mixed-extremes\", has bucket \"all\";", TxType::Write).await.unwrap();
        for (category, expected) in [("overflow", i64::MAX as f64), ("negative-overflow", i64::MIN as f64), ("mixed-extremes", -0.5)] {
            use crate::match_request::{Reduction, ReducedValue, MatchResult};
            let request = extreme_request(category, &[Reduction::Mean, Reduction::Std, Reduction::Median, Reduction::Min, Reduction::Max]);
            let stack = crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
            assert_eq!(stack.plans.len(), 2);
            let result = db.execute_match(&registry, &request).await.unwrap();
            let MatchResult::Reduction { rows, .. } = result.result() else { panic!("typed extreme statistics"); };
            let ReducedValue::Double(Some(mean)) = rows[0].values()[0] else { panic!("mean"); };
            assert_eq!(mean.to_bits(), expected.to_bits());
            let ReducedValue::Double(Some(std)) = rows[0].values()[1] else { panic!("std"); };
            assert!(std.is_finite() && std >= 0.0);
            if category != "mixed-extremes" { assert_eq!(std.to_bits(), 0); }
            let ReducedValue::Double(Some(median)) = rows[0].values()[2] else { panic!("median"); };
            assert_eq!(median.to_bits(), if category == "mixed-extremes" { 0.0_f64.to_bits() } else { expected.to_bits() });
            let (minimum, maximum) = if category == "mixed-extremes" { (i64::MIN, i64::MAX) } else if category == "overflow" { (i64::MAX, i64::MAX) } else { (i64::MIN, i64::MIN) };
            assert_eq!(rows[0].values()[3], ReducedValue::Long(Some(minimum)));
            assert_eq!(rows[0].values()[4], ReducedValue::Long(Some(maximum)));
            println!("TYPED_EXTREME_STATISTICS category={category} mean_bits={} std_bits={}", mean.to_bits(), std.to_bits());
        }
        db.execute_raw("insert $a isa measurement, has score 9223372036854775805, has category \"odd-extremes\", has bucket \"all\"; $b isa measurement, has score 9223372036854775806, has category \"odd-extremes\", has bucket \"all\"; $c isa measurement, has score 9223372036854775807, has category \"odd-extremes\", has bucket \"all\";", TxType::Write).await.unwrap();
        {
            use crate::match_request::result::{BoundConceptEvidence, ConceptId, HydratedAttribute, HydratedThing, ProviderSolutionEvidence};
            use crate::match_request::reducer::{LoweredReduceTerm, LoweredReduceInput, ReduceDomain, reduce_rows};
            let field = FieldId::new(DescriptorId::new("entity:measurement"), "score");
            let roots = ["0xa1", "0xa2", "0xa3"].map(str::to_owned);
            let solutions = roots.iter().zip([i64::MAX-2, i64::MAX-1, i64::MAX]).map(|(iid, value)| ProviderSolutionEvidence::new(vec![BoundConceptEvidence::new(V1BindingId::new(0), HydratedThing::new(
                ConceptId::new(iid), DescriptorId::new("entity:measurement"), DescriptorId::new("entity:measurement"), ThingKind::Entity,
                vec![HydratedAttribute::new(field.clone(), vec![AttributeValue::Long(value)])], vec![],
            ))])).collect::<Vec<_>>();
            let terms = [LoweredReduceTerm { reduction: crate::match_request::Reduction::Median, input: Some(LoweredReduceInput { binding: V1BindingId::new(0), field, domain: ReduceDomain::Long }) }];
            let old = reduce_rows(V1BindingId::new(0), None, &terms, &roots, &solutions, 10, 10).unwrap();
            let request = extreme_request("odd-extremes", &[crate::match_request::Reduction::Median]);
            let result = db.execute_match(&registry, &request).await.unwrap();
            let crate::match_request::MatchResult::Reduction { rows, .. } = result.result() else { panic!("odd median"); };
            let crate::match_request::ReducedValue::Double(Some(actual)) = rows[0].values()[0] else { panic!("median"); };
            let crate::match_request::ReducedValue::Double(Some(expected)) = old[0].values()[0] else { panic!("released median"); };
            assert_eq!(actual.to_bits(), expected.to_bits());
            assert_ne!((i64::MAX - 1) as i128, ((i64::MAX - 1) as f64) as i128);
            println!("TYPED_ODD_LONG_MEDIAN bits={}", actual.to_bits());
        }
        let overflow = validate_match_request(&registry, MatchRequest::v1(MatchPlan {
            bindings: vec![MatchBinding { id: V1BindingId::new(0), descriptor: DescriptorId::new("entity:measurement"), thing_kind: ThingKind::Entity, match_mode: MatchMode::Exact }],
            predicate: Some(MatchExpr::FieldValue {
                field: BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "category")),
                operator: ComparisonOp::Equal, value: AttributeValue::String("overflow".into()),
            }), allowed_cross_joins: BTreeSet::new(),
        }, MatchOperation::ReduceBy { root: V1BindingId::new(0), group: None, reducers: vec![crate::match_request::ReduceTerm {
            reduction: crate::match_request::Reduction::Sum,
            input: Some(BoundFieldId::new(V1BindingId::new(0), FieldId::new(DescriptorId::new("entity:measurement"), "score"))),
        }] })).unwrap();
        let error = db.execute_match(&registry, &overflow).await.expect_err("integer sum overflow must refuse rather than wrap");
        println!("NATIVE_SUM_OVERFLOW code={:?} failure={:?}", match &error { crate::OrmError::Match(error) => error.code().as_str(), _ => "provider" }, error.failure());
    }).await;
    db.delete_database().await.unwrap();
    if let Err(payload) = execution {
        std::panic::resume_unwind(payload);
    }
}

pub(crate) fn generic_model_selection(
    columns: &[u16],
    given: bool,
) -> (
    Arc<DescriptorRegistry>,
    type_bridge_contract::query_plan::QueryPlan,
) {
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::{Expr, Operation, PlanSpec, QueryPlan, Stage};
    use type_bridge_contract::query_plan::{InputColumn, InputColumnId, InputRow, QueryOperand};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    let registry = registry();
    let authority = MatchRequestAdapterAuthority::from_registry(&registry).unwrap();
    let binding = |n| BindingId::new(n).unwrap();
    let mut patterns = vec![
        QueryPattern::Isa {
            binding: binding(0),
            include_subtypes: false,
            type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
        }
        .into(),
        type_bridge_contract::query_plan::Pattern::Let {
            binding: binding(1),
            expr: Expr::Operand(QueryOperand::Literal {
                value: CanonicalValue::Long(7),
            }),
        },
    ];
    if given {
        patterns.push(
            QueryPattern::Has {
                owner: binding(0),
                attribute: binding(2),
                attribute_id: AttributeId::new("person-name").unwrap(),
            }
            .into(),
        );
        patterns.push(
            QueryPattern::Value {
                comparator: type_bridge_contract::migration_assertion::ValueComparator::Equal,
                left: QueryOperand::Binding {
                    binding: binding(2),
                },
                right: QueryOperand::Input {
                    column: InputColumnId::new(0),
                },
            }
            .into(),
        );
    }
    let tuple = columns.contains(&3);
    if tuple {
        patterns.push(
            QueryPattern::Isa {
                binding: binding(3),
                include_subtypes: false,
                type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
            }
            .into(),
        );
    }
    let columns = columns.iter().map(|n| binding(*n)).collect::<Vec<_>>();
    let plan = QueryPlan::new_with_limits(
        PlanSpec {
            bindings: (0..if tuple { 4 } else { 3 })
                .map(|n| {
                    AssertionBinding::new(binding(n), QueryVariable::new(format!("b{n}")).unwrap())
                })
                .collect(),
            inputs: if given {
                vec![InputColumn::new(
                    InputColumnId::new(0),
                    QueryVariable::new("input0").unwrap(),
                    ValueTypeTag::String,
                    false,
                )]
            } else {
                vec![]
            },
            given_rows: given.then(|| {
                vec![InputRow::new(vec![Some(CanonicalValue::String(
                    CanonicalString::new("Alice").unwrap(),
                ))])]
            }),
            operation: Operation::Rows,
            pipeline: vec![
                Stage::Match { patterns },
                Stage::Select {
                    bindings: columns.clone(),
                },
            ],
            output: QueryOutput::Rows { columns },
            managed_semantics: authority
                .context()
                .managed_state()
                .managed_semantic_schema()
                .clone(),
            functions: vec![],
        },
        authority.declared(),
        type_bridge_contract::limits::QueryLimits::CANONICAL,
    )
    .unwrap();
    (registry, plan)
}

#[test]
fn generic_model_stack_preserves_mixed_rows_duplicates_and_core_ordinals() {
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::migration_assertion::BindingId;
    use type_bridge_contract::query_plan::Stage;
    use type_bridge_contract::query_remote::RemoteValue;
    use type_bridge_contract::value::CanonicalValue;
    for columns in [&[0][..], &[0, 1][..]] {
        let (registry, plan) = generic_model_selection(columns, true);
        let stack = crate::match_request::build_plan_fetch_stack(
            &registry,
            &plan,
            &[(
                BindingId::new(0).unwrap(),
                DescriptorId::new("entity:person"),
            )],
        )
        .unwrap();
        let mut row = BTreeMap::new();
        row.insert(
            "b0".to_owned(),
            serde_json::to_value(RemoteValue::Thing {
                iid: "0x01".into(),
                type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
            })
            .unwrap(),
        );
        if columns.contains(&1) {
            row.insert(
                "b1".to_owned(),
                serde_json::to_value(RemoteValue::Value {
                    value: CanonicalValue::Long(7),
                })
                .unwrap(),
            );
        }
        let row = serde_json::to_value(row).unwrap();
        let answers = stack.plans().iter().map(|plan| if matches!(plan.pipeline().last(), Some(Stage::Fetch(_))) {
            PlanAnswer::Documents(vec![serde_json::json!({"_iid":"0x01","_type":"person","attributes":{"person-name":"Alice"}})])
        } else { PlanAnswer::Rows(vec![row.clone(), row.clone()]) }).collect::<Vec<_>>();
        let result = stack
            .resolve(
                &registry,
                &answers,
                crate::QueryExecutionResourceLimits::default(),
            )
            .unwrap();
        let changed = DescriptorRegistry::new();
        for descriptor in registry.snapshot() {
            match descriptor {
                crate::_descriptor::TypeDescriptor::Entity(mut entity) => {
                    if entity.type_name == "person" {
                        entity.owned_attributes.clear();
                    }
                    changed.register_entity(entity).unwrap();
                }
                crate::_descriptor::TypeDescriptor::Relation(relation) => {
                    changed.register_relation(relation).unwrap();
                }
            }
        }
        let stale = stack.resolve(
            &changed,
            &answers,
            crate::QueryExecutionResourceLimits::default(),
        );
        assert!(matches!(stale, Err(error) if error.code().as_str() == "stale_schema"));
        let excess = vec![
            (
                BindingId::new(0).unwrap(),
                DescriptorId::new("entity:person")
            );
            17
        ];
        let refused = crate::match_request::build_plan_fetch_stack(&registry, &plan, &excess);
        assert!(
            matches!(refused, Err(error) if error.code().as_str() == "plan_returning_models_slot_limit")
        );
        assert_eq!(result.validation_count(), stack.plans().len());
        assert_eq!(result.model_rows(), &[0, 0]);
        assert_eq!(result.model_bindings().len(), 1);
        assert_eq!(result.model_bindings()[0].get(), 0);
        assert!(matches!(result.selection(), PlanAnswer::Rows(rows) if rows.len()==2));
        let mut missing = answers;
        *missing.last_mut().unwrap() = PlanAnswer::Documents(vec![]);
        assert!(
            stack
                .resolve(
                    &registry,
                    &missing,
                    crate::QueryExecutionResourceLimits::default()
                )
                .is_err()
        );
    }
}

#[test]
fn generic_model_tuple_uses_one_shared_validation_pass_and_refuses_invalid_slots() {
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::migration_assertion::BindingId;
    use type_bridge_contract::query_plan::Stage;
    use type_bridge_contract::query_remote::RemoteValue;
    use type_bridge_contract::value::CanonicalValue;
    let (registry, plan) = generic_model_selection(&[0, 1, 3], false);
    let id = |n| BindingId::new(n).unwrap();
    let person = DescriptorId::new("entity:person");
    for slots in [
        vec![],
        vec![(id(0), person.clone()), (id(0), person.clone())],
        vec![(id(1), person.clone())],
        vec![(id(0), DescriptorId::new("relation:employment"))],
    ] {
        assert!(crate::match_request::build_plan_fetch_stack(&registry, &plan, &slots).is_err());
    }
    let stack = crate::match_request::build_plan_fetch_stack(
        &registry,
        &plan,
        &[(id(0), person.clone()), (id(3), person)],
    )
    .unwrap();
    assert_eq!(stack.plans().len(), 4);
    let row = serde_json::json!({"b0":serde_json::to_value(RemoteValue::Thing { iid:"0x01".into(), type_id:TypeId::new(TypeKind::Entity,"person").unwrap() }).unwrap(),"b1":serde_json::to_value(RemoteValue::Value { value:CanonicalValue::Long(7) }).unwrap(),"b3":serde_json::to_value(RemoteValue::Thing { iid:"0x02".into(),type_id:TypeId::new(TypeKind::Entity,"person").unwrap() }).unwrap()});
    let answers = stack.plans().iter().map(|plan| match plan.pipeline().last() {
        Some(Stage::Fetch(document)) => PlanAnswer::Documents(vec![if document.thing==id(0) { serde_json::json!({"_iid":"0x01","_type":"person","attributes":{"person-name":"Alice"}}) } else { serde_json::json!({"_iid":"0x02","_type":"person","attributes":{"person-name":"Bob"}}) }]),
        _ => PlanAnswer::Rows(vec![row.clone(),row.clone()]),
    }).collect::<Vec<_>>();
    let result = stack
        .resolve(
            &registry,
            &answers,
            crate::QueryExecutionResourceLimits::default(),
        )
        .unwrap();
    assert_eq!(result.validation_count(), 4);
    assert_eq!(result.model_bindings(), &[id(0), id(3)]);
    assert_eq!(result.model_rows(), &[0, 0]);
    assert_eq!(result.model_ordinal(1, id(3)), Some((0, 1)));
    assert_eq!(result.model_ordinal(2, id(3)), None);
    assert!(
        matches!(result.result().for_request(result.request()).unwrap(),crate::match_request::MatchResult::Rows { rows } if rows.len()==1 && rows[0].slots().len()==2)
    );
}

#[test]
fn native_witness_reduction_uses_minimum_witness_iid_with_existing_reducer() {
    use crate::_descriptor::TypeDescriptor;
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::query_plan::Stage;
    let modified = DescriptorRegistry::new();
    for descriptor in registry().snapshot() {
        match descriptor {
            TypeDescriptor::Entity(entity) => {
                modified.register_entity(entity).unwrap();
            }
            TypeDescriptor::Relation(mut relation) => {
                if relation.type_name == "employment" {
                    relation.owned_attributes.push(OwnedAttributeDescriptor {
                        field_name: "score".into(),
                        attr_name: "score".into(),
                        value_type: ValueType::Long,
                        annotations: vec![],
                        is_optional: false,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    });
                }
                modified.register_relation(relation).unwrap();
            }
        }
    }
    let request = validate_match_request(
        &modified,
        MatchRequest::v1(
            representative_plan(),
            MatchOperation::ReduceBy {
                root: V1BindingId::new(0),
                group: None,
                reducers: vec![crate::match_request::ReduceTerm {
                    reduction: crate::match_request::Reduction::Sum,
                    input: Some(BoundFieldId::new(
                        V1BindingId::new(1),
                        FieldId::new(DescriptorId::new("relation:employment"), "score"),
                    )),
                }],
            },
        ),
    )
    .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&modified, &request).unwrap();
    assert!(matches!(
        stack.layout,
        crate::match_request::plan_adapter::Layout::WitnessReduction { .. }
    ));
    let mut answers = native_stack_answers(&stack);
    for (plan, answer) in stack.plans.iter().zip(&mut answers) {
        match answer {
            PlanAnswer::Rows(rows) => {
                let mut second = rows[0].clone();
                second["b1"]["iid"] = serde_json::json!("0x03");
                rows.push(second);
            }
            PlanAnswer::Documents(documents) if matches!(plan.pipeline().last(),Some(Stage::Fetch(document)) if document.thing.get()==1) =>
            {
                documents[0]["attributes"] = serde_json::json!({"score":99});
                let mut second = documents[0].clone();
                second["_iid"] = serde_json::json!("0x03");
                second["attributes"] = serde_json::json!({"score":20});
                documents.push(second);
            }
            _ => {}
        }
    }
    let result = crate::match_request::result_validation::validated_match_result_from_plan_stack(
        &modified,
        &request,
        &stack,
        &answers,
        crate::QueryExecutionResourceLimits::default(),
    )
    .unwrap();
    assert!(
        matches!(result.for_request(&request).unwrap(),crate::match_request::MatchResult::Reduction { rows,.. } if rows.len()==1 && rows[0].values()==[crate::match_request::ReducedValue::Long(Some(99))])
    );
}

#[test]
fn native_abstract_selection_materializes_only_constructible_descendants() {
    let source = registry();
    let registry = DescriptorRegistry::new();
    let mut descendant = None;
    for descriptor in source.snapshot() {
        match descriptor {
            crate::_descriptor::TypeDescriptor::Entity(mut descriptor) => {
                if descriptor.type_name == "person" {
                    let mut child = descriptor.clone();
                    child.type_name = "employee".into();
                    child.parent_type = Some("person".into());
                    descendant = Some(child);
                    descriptor.is_abstract = true;
                }
                registry.register_entity(descriptor).unwrap();
            }
            crate::_descriptor::TypeDescriptor::Relation(descriptor) => {
                registry.register_relation(descriptor).unwrap();
            }
        }
    }
    registry.register_entity(descendant.unwrap()).unwrap();
    let request = validate_match_request(
        &registry,
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![representative_plan().bindings.remove(0)],
                predicate: None,
                allowed_cross_joins: Default::default(),
            },
            fetch_rows_operation(),
        ),
    )
    .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
    assert_eq!(stack.plans.len(), 1);
    let answers = vec![crate::query_plan_exec::PlanAnswer::Documents(vec![
        serde_json::json!({"_iid":"0x01","_type":"employee","attributes":{"person-name":"Alice"}}),
    ])];
    crate::match_request::plan_result::resolve(
        &stack,
        &request,
        &answers,
        crate::QueryExecutionResourceLimits::default(),
    )
    .unwrap();
}

#[test]
fn generic_model_partition_refuses_exhausted_binding_budget_before_io() {
    use type_bridge_contract::migration_assertion::BindingId;
    let (registry, source) = generic_model_selection(&[0, 1, 3], false);
    let authority =
        crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
            &registry,
        )
        .unwrap();
    let mut limits = source.limits();
    limits.structural.bindings = source.bindings().len();
    limits.structural.selected_slots = limits.structural.bindings;
    let source = type_bridge_contract::query_plan::decode_query_plan_with_limits(
        &source.canonical_bytes().unwrap(),
        authority.declared(),
        limits,
    )
    .unwrap();
    let slots = [
        (
            BindingId::new(0).unwrap(),
            DescriptorId::new("entity:person"),
        ),
        (
            BindingId::new(3).unwrap(),
            DescriptorId::new("entity:person"),
        ),
    ];
    let refusal = crate::match_request::build_plan_fetch_stack(&registry, &source, &slots)
        .err()
        .expect("grouping needs one fresh binding");
    assert_eq!(
        refusal.code().as_str(),
        "plan_returning_models_binding_limit"
    );
}
