use std::collections::BTreeSet;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::{
    QueryInvocation, QueryOperation, QueryOutput, QueryPattern, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
    RelatesFact, RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_orm::AttributeValue;
use type_bridge_orm::match_request::{
    BindingId as V1BindingId, BindingPair, BoundFieldId, ComparisonOp,
    DescriptorId, FetchShape, FetchSlot, FieldId, MatchBinding, MatchExpr,
    MatchMode, MatchOperation, MatchOrder, MatchPlan, MatchRequest,
    MissingOrder, RoleEdgeId, RoleId as V1RoleId, RowCardinality,
    SortDirection, ThingKind, Window,
};
use type_bridge_orm::query_v2::lower_validated_query;
use type_bridge_orm::query_v2_adapter::adapt_match_request;
use type_bridge_query::{
    MigrationAssertionValidationContext, validate_query_plan,
};
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve,
};
use type_bridge_contract::value::ValueTypeTag;

struct SchemaFixture {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

fn schema_fixture() -> SchemaFixture {
    let person = TypeId::new(TypeKind::Entity, "person").expect("type");
    let employment = TypeId::new(TypeKind::Relation, "employment").expect("type");
    let worker = type_bridge_contract::id::RoleId::new("employment", "worker")
        .expect("role");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, "name").expect("type"))
                .expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name).expect("owns id"),
        )),
        SchemaFact::Type(TypeFact::new(employment.clone()).expect("type fact")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(employment, worker.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person, worker).expect("plays id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("adapter-fixture").expect("document"),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .expect("span"),
        )
    });
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared schema");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("adapter-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    SchemaFixture {
        managed: managed_schema_state(&declared, &context).expect("managed state"),
        resolved: resolve(&declared, &profile).expect("resolved schema"),
    }
}

fn name_field() -> BoundFieldId {
    BoundFieldId::new(
        V1BindingId::new(0),
        FieldId::new(DescriptorId::new("entity:person"), "name"),
    )
}

fn representative_plan() -> MatchPlan {
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
                    role: V1RoleId::new(
                        DescriptorId::new("relation:employment"),
                        "worker",
                    ),
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

fn fetch_rows_operation() -> MatchOperation {
    MatchOperation::FetchRows {
        output: FetchShape::Positional {
            slots: vec![FetchSlot::One { binding: V1BindingId::new(0) }],
        },
        order: vec![MatchOrder {
            field: name_field(),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        }],
        window: Window { offset: 0, limit: 5 },
        cardinality: RowCardinality::BoundedMany,
    }
}

#[test]
fn a_representative_v1_request_adapts_validates_and_lowers() {
    let fixture = schema_fixture();
    let request = MatchRequest::v1(representative_plan(), fetch_rows_operation());
    let adapted = adapt_match_request(
        &request,
        fixture.managed.managed_semantic_schema(),
    )
    .expect("representative adaptation");
    assert_eq!(adapted.operation(), QueryOperation::Rows);

    let plan = adapted.plan();
    assert!(plan.inputs().is_empty(), "V1 requests carry no input columns");
    let QueryOutput::Rows { columns } = plan.output() else {
        panic!("adapted plans project rows");
    };
    assert_eq!(columns.len(), 1, "one selected V1 slot projects one column");
    let ReadStage::Match { patterns } = &plan.pipeline()[0] else {
        panic!("match opens the adapted pipeline");
    };
    assert!(patterns.iter().any(|pattern| matches!(
        pattern,
        QueryPattern::Isa { include_subtypes: true, .. }
    )));
    assert!(patterns.iter().any(|pattern| matches!(
        pattern,
        QueryPattern::Links { .. }
    )));
    assert!(patterns.iter().any(|pattern| matches!(
        pattern,
        QueryPattern::Not { .. }
    )));

    // The adapted plan is a real V2 plan: it validates and lowers.
    let context =
        MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let validated = validate_query_plan(plan, &context, StructuralLimits::CANONICAL)
        .expect("adapted plan validates against the schema");
    let invocation =
        QueryInvocation::new(plan, adapted.operation(), Vec::new())
            .expect("input-free invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("adapted lowering");
    for syntax in [
        "$b0 isa person",
        "$b1 isa! employment, links (worker: $b0)",
        "$b0 has name $f0",
        "$f0 >= \"A\"",
        "not {",
        "select $b0, $f0;",
        "distinct;",
        "sort $f0 asc;",
        "limit 5;",
    ] {
        assert!(
            lowered.typeql().contains(syntax),
            "missing {syntax:?} in:\n{}",
            lowered.typeql(),
        );
    }

    // Count and exists adapt over the same plan graph.
    let count = adapt_match_request(
        &MatchRequest::v1(
            representative_plan(),
            MatchOperation::CountBy { root: V1BindingId::new(0) },
        ),
        fixture.managed.managed_semantic_schema(),
    )
    .expect("count adaptation");
    assert_eq!(count.operation(), QueryOperation::Count);
    validate_query_plan(count.plan(), &context, StructuralLimits::CANONICAL)
        .expect("adapted count plan validates");

    let exists = adapt_match_request(
        &MatchRequest::v1(
            representative_plan(),
            MatchOperation::ExistsBy { root: V1BindingId::new(0) },
        ),
        fixture.managed.managed_semantic_schema(),
    )
    .expect("exists adaptation");
    assert_eq!(exists.operation(), QueryOperation::Exists);
}

#[test]
fn inexpressible_v1_shapes_reject_by_name() {
    let fixture = schema_fixture();
    let semantics = fixture.managed.managed_semantic_schema();
    let adapt = |plan: MatchPlan, operation: MatchOperation| {
        adapt_match_request(&MatchRequest::v1(plan, operation), semantics)
            .expect_err("inexpressible shape must reject")
            .code()
            .as_str()
            .to_owned()
    };

    let with_predicate = |predicate: MatchExpr| {
        let mut plan = representative_plan();
        plan.predicate = Some(predicate);
        plan
    };
    assert_eq!(
        adapt(
            with_predicate(MatchExpr::Or {
                expressions: vec![MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::Equal,
                    value: AttributeValue::String("a".to_owned()),
                }],
            }),
            fetch_rows_operation(),
        ),
        "query_v2_adapter_disjunction_unsupported",
    );
    assert_eq!(
        adapt(
            with_predicate(MatchExpr::FieldValue {
                field: name_field(),
                operator: ComparisonOp::Contains,
                value: AttributeValue::String("a".to_owned()),
            }),
            fetch_rows_operation(),
        ),
        "query_v2_adapter_string_operator_unsupported",
    );

    assert_eq!(
        adapt(
            representative_plan(),
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: V1BindingId::new(0) }],
                },
                order: Vec::new(),
                window: Window { offset: 0, limit: 5 },
                include_total: false,
            },
        ),
        "query_v2_adapter_paging_unsupported",
    );
    assert_eq!(
        adapt(
            representative_plan(),
            MatchOperation::FetchRows {
                output: FetchShape::Named { slots: Vec::new() },
                order: Vec::new(),
                window: Window { offset: 0, limit: 5 },
                cardinality: RowCardinality::BoundedMany,
            },
        ),
        "query_v2_adapter_named_shape_unsupported",
    );
    assert_eq!(
        adapt(
            representative_plan(),
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::Collect {
                        binding: V1BindingId::new(0),
                        distinct: true,
                        order: Vec::new(),
                    }],
                },
                order: Vec::new(),
                window: Window { offset: 0, limit: 5 },
                cardinality: RowCardinality::BoundedMany,
            },
        ),
        "query_v2_adapter_collection_unsupported",
    );
    assert_eq!(
        adapt(
            representative_plan(),
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: V1BindingId::new(0) }],
                },
                order: vec![MatchOrder {
                    field: name_field(),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                }],
                window: Window { offset: 0, limit: 1 },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
        "query_v2_adapter_cardinality_unsupported",
    );
    assert_eq!(
        adapt(
            representative_plan(),
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: V1BindingId::new(0) }],
                },
                order: Vec::new(),
                window: Window { offset: 0, limit: 5 },
                cardinality: RowCardinality::BoundedMany,
            },
        ),
        "query_v2_adapter_unordered_window",
    );

    let mut crossed = representative_plan();
    crossed
        .allowed_cross_joins
        .insert(BindingPair::new(V1BindingId::new(0), V1BindingId::new(1)));
    assert_eq!(
        adapt(crossed, fetch_rows_operation()),
        "query_v2_adapter_cross_join_unsupported",
    );
}
