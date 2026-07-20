//! Validated V1 requests adapt onto V2 plans faithfully or reject by name.
//!
//! The registry deliberately renames the person name member (field `name`,
//! provider attribute `person-name`) so any syntactic field-name emission
//! regresses loudly, and marks it a key so the validator's stable-order
//! synthesis is observable through the adapter.

use std::collections::BTreeSet;
use std::sync::Arc;

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
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId,
    SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact,
    ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::AttributeValue;
use type_bridge_orm::match_request::{
    BindingId as V1BindingId, BindingPair, BoundFieldId, ComparisonOp, DescriptorId, FetchShape,
    FetchSlot, FieldId, MatchBinding, MatchExpr, MatchMode, MatchOperation, MatchOrder, MatchPlan,
    MatchRequest, MissingOrder, RoleEdgeId, RoleId as V1RoleId, RowCardinality, SortDirection,
    ThingKind, Window, validate_match_request,
};
use type_bridge_orm::query_v2::lower_validated_query;
use type_bridge_orm::query_v2_adapter::{AdaptedMatchRequest, adapt_match_request};
use type_bridge_orm::{
    Annotation, DescriptorRegistry, EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor,
    RoleDescriptor, ValueType,
};
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

struct SchemaFixture {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

fn schema_fixture() -> SchemaFixture {
    let person = TypeId::new(TypeKind::Entity, "person").expect("type");
    let employment = TypeId::new(TypeKind::Relation, "employment").expect("type");
    let worker = type_bridge_contract::id::RoleId::new("employment", "worker").expect("role");
    let name = AttributeId::new("person-name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, "person-name").expect("type"))
                .expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).expect("owns id"),
        )),
        // The V1 registry declares person-name as a key field; the V2
        // schema carries the same fact so windowed adapted plans prove
        // their sort tuple total through the unique ownership.
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(
                        OwnsFactId::new(person.clone(), name).expect("owns id"),
                    ),
                    AnnotationKindId::Key,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("key annotation"),
        ),
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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
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

fn registry() -> Arc<DescriptorRegistry> {
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

fn fetch_rows_operation() -> MatchOperation {
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

fn adapt(
    plan: MatchPlan,
    operation: MatchOperation,
    fixture: &SchemaFixture,
) -> Result<AdaptedMatchRequest, type_bridge_contract::diagnostic::Diagnostic> {
    let registry = registry();
    let validated = validate_match_request(&registry, MatchRequest::v1(plan, operation))
        .expect("corpus request passes V1 validation");
    adapt_match_request(
        &validated,
        &registry,
        fixture.managed.managed_semantic_schema(),
    )
}

#[test]
fn a_representative_v1_request_adapts_validates_and_lowers() {
    let fixture = schema_fixture();
    let adapted = adapt(representative_plan(), fetch_rows_operation(), &fixture)
        .expect("representative adaptation");
    assert_eq!(adapted.operation(), QueryOperation::Rows);

    let plan = adapted.plan();
    assert!(
        plan.inputs().is_empty(),
        "V1 requests carry no input columns"
    );
    let QueryOutput::Rows { columns } = plan.output() else {
        panic!("adapted plans project rows");
    };
    assert_eq!(columns.len(), 1, "one selected V1 slot projects one column");
    let ReadStage::Match { patterns } = &plan.pipeline()[0] else {
        panic!("match opens the adapted pipeline");
    };
    assert!(patterns.iter().any(|pattern| matches!(
        pattern,
        QueryPattern::Isa {
            include_subtypes: true,
            ..
        }
    )));
    assert!(
        patterns
            .iter()
            .any(|pattern| matches!(pattern, QueryPattern::Links { .. }))
    );
    assert!(
        patterns
            .iter()
            .any(|pattern| matches!(pattern, QueryPattern::Not { .. }))
    );

    // The adapted plan is a real V2 plan: it validates and lowers, and the
    // renamed member surfaces as its provider label, never its field name.
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let validated = validate_query_plan(plan, &context, StructuralLimits::CANONICAL)
        .expect("adapted plan validates against the schema");
    let invocation =
        QueryInvocation::new(plan, adapted.operation(), Vec::new()).expect("input-free invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("adapted lowering");
    for syntax in [
        "$b0 isa person",
        "$b1 isa! employment, links (worker: $b0)",
        "$b0 has person-name $f0",
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
    assert!(
        !lowered.typeql().contains("has name "),
        "field name must not leak as an attribute label:\n{}",
        lowered.typeql(),
    );

    // Count and exists adapt over the same plan graph.
    let count = adapt(
        representative_plan(),
        MatchOperation::CountBy {
            root: V1BindingId::new(0),
        },
        &fixture,
    )
    .expect("count adaptation");
    assert_eq!(count.operation(), QueryOperation::Count);
    validate_query_plan(count.plan(), &context, StructuralLimits::CANONICAL)
        .expect("adapted count plan validates");

    let exists = adapt(
        representative_plan(),
        MatchOperation::ExistsBy {
            root: V1BindingId::new(0),
        },
        &fixture,
    )
    .expect("exists adaptation");
    assert_eq!(exists.operation(), QueryOperation::Exists);
}

#[test]
fn a_window_without_public_order_adapts_through_the_synthesized_stable_order() {
    // V1 validation synthesizes a unique-key tie breaker for an unordered
    // window over a keyed root; the adapter must consume that proof so the
    // adapted page keeps V1 membership instead of rejecting or truncating
    // over provider order.
    let fixture = schema_fixture();
    let adapted = adapt(
        representative_plan(),
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: V1BindingId::new(0),
                }],
            },
            order: Vec::new(),
            window: Window {
                offset: 1,
                limit: 5,
            },
            cardinality: RowCardinality::BoundedMany,
        },
        &fixture,
    )
    .expect("unordered keyed window adapts through the stable order");
    assert!(
        adapted
            .plan()
            .pipeline()
            .iter()
            .any(|stage| matches!(stage, ReadStage::Sort { .. })),
        "the synthesized stable order must appear as a sort stage"
    );
    assert!(
        adapted
            .plan()
            .pipeline()
            .iter()
            .any(|stage| matches!(stage, ReadStage::Limit { .. })),
    );
}

#[test]
fn inexpressible_v1_shapes_reject_by_name() {
    let fixture = schema_fixture();
    let adapt_err = |plan: MatchPlan, operation: MatchOperation| {
        adapt(plan, operation, &fixture)
            .expect_err("inexpressible shape must reject")
            .code()
            .as_str()
            .to_owned()
    };

    // A predicate without the role edge leaves the employment binding
    // disconnected, which V1 validation itself rejects; predicate-shape
    // cases therefore run over the single person binding.
    let with_predicate = |predicate: MatchExpr| MatchPlan {
        bindings: vec![MatchBinding {
            id: V1BindingId::new(0),
            descriptor: DescriptorId::new("entity:person"),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Subtypes,
        }],
        predicate: Some(predicate),
        allowed_cross_joins: BTreeSet::new(),
    };
    assert_eq!(
        adapt_err(
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
        adapt_err(
            with_predicate(MatchExpr::FieldValue {
                field: name_field(),
                operator: ComparisonOp::Contains,
                value: AttributeValue::String("a".to_owned()),
            }),
            fetch_rows_operation(),
        ),
        "query_v2_adapter_string_operator_unsupported",
    );

    // A subtype-inclusive relation binding under a role edge would be
    // silently narrowed by V2's exact links; it must reject instead.
    let mut subtype_links = representative_plan();
    subtype_links.bindings[1].match_mode = MatchMode::Subtypes;
    assert_eq!(
        adapt_err(subtype_links, fetch_rows_operation()),
        "query_v2_adapter_subtype_links_unsupported",
    );

    assert_eq!(
        adapt_err(
            representative_plan(),
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: V1BindingId::new(0)
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 5
                },
                include_total: false,
            },
        ),
        "query_v2_adapter_paging_unsupported",
    );
    // Collect slots outside PageBy are rejected by V1 validation itself
    // ("collection_requires_page_root"); the adapter's collection rejection
    // stays as unreachable defense and has no validated reproduction.
    assert_eq!(
        adapt_err(
            representative_plan(),
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: V1BindingId::new(0)
                    }],
                },
                order: vec![MatchOrder {
                    field: name_field(),
                    direction: SortDirection::Ascending,
                    missing: MissingOrder::Reject,
                }],
                window: Window {
                    offset: 0,
                    limit: 1
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
        "query_v2_adapter_cardinality_unsupported",
    );

    let mut crossed = representative_plan();
    crossed
        .allowed_cross_joins
        .insert(BindingPair::new(V1BindingId::new(0), V1BindingId::new(1)));
    assert_eq!(
        adapt_err(crossed, fetch_rows_operation()),
        "query_v2_adapter_cross_join_unsupported",
    );
}
