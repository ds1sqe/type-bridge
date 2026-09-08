//! Validated V1 requests adapt onto V2 plans faithfully or reject by name.
//!
//! The registry deliberately renames the person name member (field `name`,
//! provider attribute `person-name`) so any syntactic field-name emission
//! regresses loudly, and marks it a key so the validator's stable-order
//! synthesis is observable through the adapter.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
};
use crate::_entity::Annotation;
use crate::_registry::DescriptorRegistry;
use crate::AttributeValue;
use crate::ValueType;
use crate::match_request::lowering::{LoweredMatchExecution, lower_match_execution};
use crate::match_request::{
    BindingId as V1BindingId, BindingPair, BoundFieldId, ComparisonOp, DescriptorId, FetchShape,
    FetchSlot, FieldId, MatchBinding, MatchExpr, MatchMode, MatchOperation, MatchOrder, MatchPlan,
    MatchRequest, MissingOrder, NamedFetchSlot, RoleEdgeId, RoleId as V1RoleId, RowCardinality,
    SortDirection, ThingKind, Window, validate_match_request,
};
use crate::query_v2::lower_validated_query;
use crate::query_v2_adapter::{
    AdaptedMatchRequest, MatchRequestAdaptation, MatchRequestAdapterAuthority, adapt_match_request,
    adapt_value,
};
use crate::query_v2_compatibility::{
    CompatibilityProviderPlan, lower_validated_compatibility_query,
};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::{
    ModelQueryV2, QueryComparatorV2, QueryInvocation, QueryOperation, QueryOutput, QueryPattern,
    QueryPatternV2, ReadStage,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId,
    SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId, TypeFact,
    ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

struct SchemaFixture {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

#[test]
fn production_adapter_has_one_builder_owned_v2_construction_gate() {
    let adapter = include_str!("query_v2_adapter.rs");
    assert!(
        adapter.contains("QueryPlanBuilder::finalize_compatibility"),
        "released compatibility adaptation must enter the shared builder"
    );
    assert!(
        !adapter.contains("QueryPlan::new_v2_with_functions"),
        "the adapter must not construct a second V2 plan program"
    );

    let builder = include_str!("query_v2_builder.rs");
    assert_eq!(
        builder.matches("QueryPlan::new_v2_with_functions").count(),
        1,
        "ordinary and compatibility plans must share one construction gate"
    );
}

fn schema_fixture() -> SchemaFixture {
    let person = TypeId::new(TypeKind::Entity, "person").expect("type");
    let employment = TypeId::new(TypeKind::Relation, "employment").expect("type");
    let worker = type_bridge_contract::id::RoleId::new("employment", "worker").expect("role");
    let edge = TypeId::new(TypeKind::Relation, "directed-edge").expect("type");
    let origin = type_bridge_contract::id::RoleId::new("directed-edge", "origin").expect("role");
    let destination =
        type_bridge_contract::id::RoleId::new("directed-edge", "destination").expect("role");
    let interaction = TypeId::new(TypeKind::Relation, "interaction").expect("type");
    let assignment = TypeId::new(TypeKind::Relation, "assignment").expect("type");
    let participant =
        type_bridge_contract::id::RoleId::new("interaction", "participant").expect("role");
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
            PlaysFactId::new(person.clone(), worker).expect("plays id"),
        )),
        SchemaFact::Type(TypeFact::new(edge.clone()).expect("type fact")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), origin.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge, destination.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), origin).expect("plays id"),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), destination).expect("plays id"),
        )),
        SchemaFact::Type(TypeFact::new(interaction.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(assignment.clone()).expect("type fact")),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(assignment, interaction.clone()).expect("sub id"),
        )),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(interaction, participant.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person, participant).expect("plays id"),
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

fn name_field() -> BoundFieldId {
    name_field_for(0)
}

fn name_field_for(binding: u16) -> BoundFieldId {
    BoundFieldId::new(
        V1BindingId::new(binding),
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

fn inherited_role_plan() -> MatchPlan {
    MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("relation:assignment"),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: V1BindingId::new(1),
            role: V1RoleId::new(DescriptorId::new("relation:interaction"), "participant"),
            player: V1BindingId::new(0),
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
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    match adapt_match_request(&validated, &registry, &context, StructuralLimits::CANONICAL)? {
        MatchRequestAdaptation::Adapted(adapted) => Ok(adapted),
        MatchRequestAdaptation::LegacyRequired(reason) => panic!(
            "small adapter fixture unexpectedly requires the V1 resource fallback: {reason:?}"
        ),
        MatchRequestAdaptation::NativeOnly => {
            panic!("small adapter fixture unexpectedly declared a native-only operation")
        }
    }
}

fn assert_provider_ast_parity(
    registry: &DescriptorRegistry,
    validated: &crate::match_request::ValidatedMatchRequest,
    adapted: &AdaptedMatchRequest,
) {
    let direct =
        lower_match_execution(registry, validated).expect("released typed lowering succeeds");
    let compatibility =
        lower_validated_compatibility_query(adapted.validated(), adapted.operation())
            .expect("compatibility lowering succeeds")
            .expect("adapter-authored plan has one compatibility provider plan");
    match (direct, compatibility.provider_plan()) {
        (
            LoweredMatchExecution::FetchRows(direct),
            CompatibilityProviderPlan::Rows {
                statement,
                tuple_proof,
            },
        ) => {
            assert_eq!(&direct, statement);
            assert!(tuple_proof.is_none());
        }
        (
            LoweredMatchExecution::ExactlyOneBy {
                selection,
                evidence,
            },
            CompatibilityProviderPlan::Rows {
                statement,
                tuple_proof,
            },
        ) => {
            // V2 executes the two-row terminal selection directly. The
            // released evidence statement differs only in window/order; its
            // targets, fields, predicate, projection, and distinctness are the
            // exact same cross-band typed vocabulary.
            assert_eq!(&selection, statement);
            assert_eq!(tuple_proof.as_ref(), Some(&selection));
            assert_eq!(evidence.targets, statement.targets);
            assert_eq!(evidence.fields, statement.fields);
            assert_eq!(evidence.predicate, statement.predicate);
            assert_eq!(evidence.projection, statement.projection);
            assert_eq!(evidence.distinct, statement.distinct);
        }
        (
            LoweredMatchExecution::CountBy { root, scan: direct },
            CompatibilityProviderPlan::DistinctCount { scan },
        ) => {
            assert_eq!(root.get(), scan.root);
            assert_eq!(&direct, scan);
        }
        (
            LoweredMatchExecution::ExistsBy { root, scan: direct },
            CompatibilityProviderPlan::DistinctExists { scan },
        ) => {
            assert_eq!(root.get(), scan.root);
            assert_eq!(&direct, scan);
        }
        (
            LoweredMatchExecution::PageBy {
                root,
                total,
                selection,
                rematch,
            },
            CompatibilityProviderPlan::Page {
                selection: adapted_selection,
                total: adapted_total,
                rematch: adapted_rematch,
            },
        ) => {
            assert_eq!(root.get(), adapted_selection.root);
            assert_eq!(selection.as_ref(), adapted_selection);
            assert_eq!(&total, adapted_total);
            assert_eq!(&rematch, adapted_rematch);
        }
        (direct, adapted) => {
            panic!("released/adapted provider plan variants diverged: {direct:?} vs {adapted:?}")
        }
    }
}

#[test]
fn production_adapter_provider_asts_are_band_neutral_for_every_released_operation() {
    let registry = registry();
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("production authority projects");
    let one = FetchShape::Positional {
        slots: vec![FetchSlot::One {
            binding: V1BindingId::new(0),
        }],
    };
    let cases = vec![
        (representative_plan(), fetch_rows_operation()),
        (
            representative_plan(),
            MatchOperation::FetchRows {
                output: one.clone(),
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
        (
            representative_plan(),
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: one.clone(),
                order: Vec::new(),
                window: Window {
                    offset: 1,
                    limit: 2,
                },
                include_total: false,
            },
        ),
        (
            representative_plan(),
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: one,
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 3,
                },
                include_total: true,
            },
        ),
        (
            representative_plan(),
            MatchOperation::CountBy {
                root: V1BindingId::new(0),
            },
        ),
        (
            representative_plan(),
            MatchOperation::ExistsBy {
                root: V1BindingId::new(0),
            },
        ),
        (inherited_role_plan(), fetch_rows_operation()),
    ];

    for (plan, operation) in cases {
        let validated = validate_match_request(&registry, MatchRequest::v1(plan, operation))
            .expect("released operation validates");
        let MatchRequestAdaptation::Adapted(adapted) = adapt_match_request(
            &validated,
            &registry,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .expect("released operation adapts") else {
            panic!("small released operation must not take the artifact-size fallback")
        };
        assert_provider_ast_parity(&registry, &validated, &adapted);
    }
}

#[test]
fn a_representative_v1_request_adapts_validates_and_lowers() {
    let fixture = schema_fixture();
    let adapted = adapt(representative_plan(), fetch_rows_operation(), &fixture)
        .expect("representative adaptation");
    assert_eq!(adapted.operation(), QueryOperation::Rows);

    let plan = adapted.validated().plan();
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
            .all(|pattern| matches!(pattern, QueryPattern::Isa { .. })),
        "the native program is only the canonical target skeleton"
    );
    let compatibility = plan
        .v2_compatibility()
        .expect("adapted plans carry compatibility semantics");
    let Some(QueryPatternV2::And { patterns }) = compatibility.predicate() else {
        panic!("representative predicate remains a compatibility conjunction");
    };
    assert!(matches!(patterns[0], QueryPatternV2::RoleEdge { .. }));
    assert!(matches!(patterns[1], QueryPatternV2::FieldValue { .. }));
    assert!(matches!(patterns[2], QueryPatternV2::Not { .. }));

    // The adapted plan is a real V2 plan: it validates and lowers, and the
    // renamed member surfaces as its provider label, never its field name.
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let validated = adapted.validated();
    let invocation =
        QueryInvocation::new(plan, adapted.operation(), Vec::new()).expect("input-free invocation");
    let lowered = lower_validated_query(validated, &invocation).expect("adapted lowering");
    for syntax in [
        "$b0 isa person",
        "$b1 isa! employment;",
        "$b1 links (worker: $b0)",
        "$b0 has person-name $f0",
        "$f0 >= \"A\"",
        "not {",
        "select $b0, $b1, $f0;",
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
    validate_query_plan(
        count.validated().plan(),
        &context,
        StructuralLimits::CANONICAL,
    )
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
fn bounded_reachability_adapts_to_the_canonical_v2_pattern() {
    let fixture = schema_fixture();
    let relation = DescriptorId::new("relation:directed-edge");
    let plan = MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Subtypes,
            },
        ],
        predicate: Some(MatchExpr::Reachable {
            relation: relation.clone(),
            role_from: V1RoleId::new(relation.clone(), "origin"),
            role_to: V1RoleId::new(relation, "destination"),
            source: V1BindingId::new(0),
            target: V1BindingId::new(1),
            min_depth: 0,
            max_depth: 3,
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let adapted = adapt(
        plan,
        MatchOperation::CountBy {
            root: V1BindingId::new(0),
        },
        &fixture,
    )
    .expect("bounded reachability adapts");
    let compatibility = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .expect("adapted plan");
    assert!(matches!(
        compatibility.predicate(),
        Some(QueryPatternV2::Reachable {
            min_depth: 0,
            max_depth: 3,
            source,
            target,
            ..
        }) if source.get() == 0 && target.get() == 1
    ));

    let invocation =
        QueryInvocation::new(adapted.validated().plan(), adapted.operation(), Vec::new())
            .expect("input-free invocation");
    let lowered =
        lower_validated_query(adapted.validated(), &invocation).expect("adapted lowering");
    assert!(
        lowered
            .typeql()
            .contains("$R0z isa person; $b0 is $R0z; $b1 is $R0z;"),
        "{}",
        lowered.typeql(),
    );
    assert!(lowered.typeql().contains("isa! directed-edge"));
    assert!(
        lowered
            .typeql()
            .contains("reduce $RreachableProofCount = count groupby $b0, $b1")
    );
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
    let Some(ModelQueryV2::Rows { order, window, .. }) = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        panic!("bounded rows carry a compatibility model terminal");
    };
    let order = order
        .as_ref()
        .expect("V1 validator synthesized a total order");
    assert_eq!(order.terms().len(), 1);
    assert_eq!(
        order.terms()[0].field().attribute().label().as_str(),
        "person-name"
    );
    assert_eq!(window.offset(), 1);
    assert_eq!(window.limit(), 5);
}

#[test]
fn an_inherited_role_keeps_its_canonical_declaring_relation() {
    let fixture = schema_fixture();
    let adapted = adapt(inherited_role_plan(), fetch_rows_operation(), &fixture)
        .expect("an exact child relation can use an inherited canonical role");
    let Some(QueryPatternV2::RoleEdge {
        relation_type,
        role,
        ..
    }) = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.predicate())
    else {
        panic!("role edge remains the sole compatibility predicate");
    };

    assert_eq!(relation_type.label().as_str(), "assignment");
    assert_eq!(role.declaring_relation().as_str(), "interaction");
    assert_eq!(role.label().as_str(), "participant");
}

#[test]
fn a_foreign_registry_rejects_before_plan_construction() {
    let fixture = schema_fixture();
    let original = registry();
    let validated = validate_match_request(
        &original,
        MatchRequest::v1(representative_plan(), fetch_rows_operation()),
    )
    .expect("request validates against its original registry");
    let foreign = DescriptorRegistry::new();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);

    let error = adapt_match_request(&validated, &foreign, &context, StructuralLimits::CANONICAL)
        .expect_err("foreign registry must not relabel a validated request");
    assert_eq!(error.code().as_str(), "query_v2_adapter_registry_mismatch");
}

#[test]
fn production_authority_round_trips_aliases_subtypes_and_inherited_roles() {
    let registry = registry();
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("the production descriptor projection forms V2 authority");
    for (plan, operation) in [
        (representative_plan(), fetch_rows_operation()),
        (inherited_role_plan(), fetch_rows_operation()),
    ] {
        let validated = validate_match_request(&registry, MatchRequest::v1(plan, operation))
            .expect("released request validates");
        let MatchRequestAdaptation::Adapted(adapted) = adapt_match_request(
            &validated,
            &registry,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .expect("production authority adapts its own validated registry") else {
            panic!("small released request must not use the resource fallback");
        };
        let invocation =
            QueryInvocation::new(adapted.validated().plan(), adapted.operation(), Vec::new())
                .expect("input-free invocation");
        let lowered = lower_validated_query(adapted.validated(), &invocation)
            .expect("production-authority plan lowers");
        assert!(lowered.typeql().contains("person-name"));
    }
}

#[test]
fn subtype_hydration_is_canonical_when_a_child_sorts_before_its_parent() {
    let registry = registry();
    registry
        .register_entity(EntityDescriptor {
            type_name: "employee".into(),
            is_abstract: false,
            parent_type: Some("person".into()),
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
        .expect("register lexically earlier entity subtype");
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("production descriptor projection forms V2 authority");
    let validated = validate_match_request(
        &registry,
        MatchRequest::v1(representative_plan(), fetch_rows_operation()),
    )
    .expect("subtype-inclusive released request validates");
    let MatchRequestAdaptation::Adapted(adapted) = adapt_match_request(
        &validated,
        &registry,
        &authority.context(),
        StructuralLimits::CANONICAL,
    )
    .expect("lexical subtype order is canonicalized") else {
        panic!("small released request must not use the resource fallback");
    };
    let Some(ModelQueryV2::Rows { hydration, .. }) = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        panic!("released rows carry hydration authority");
    };
    let person = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding().get() == 0)
        .expect("person hydration binding");
    assert_eq!(
        person
            .concrete_descriptors()
            .iter()
            .map(|descriptor| descriptor.label().as_str())
            .collect::<Vec<_>>(),
        ["employee", "person"],
    );
    let worker = hydration
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.descriptor().label().as_str() == "employment")
        .and_then(|descriptor| descriptor.roles().first())
        .and_then(|role| role.players().first())
        .expect("employment worker role-player hydration");
    assert_eq!(
        worker
            .concrete_descriptors()
            .iter()
            .map(|descriptor| descriptor.label().as_str())
            .collect::<Vec<_>>(),
        ["employee", "person"],
    );
}

#[test]
fn exact_subtype_binding_adapts_through_base_declared_role_authority() {
    let registry = registry();
    registry
        .register_entity(EntityDescriptor {
            type_name: "employee".into(),
            is_abstract: false,
            parent_type: Some("person".into()),
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
        .expect("register exact employee subtype");
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("production descriptor projection forms subtype authority");
    let plan = MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:employee"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("relation:employment"),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: V1BindingId::new(1),
            role: V1RoleId::new(DescriptorId::new("relation:employment"), "worker"),
            player: V1BindingId::new(0),
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let validated =
        validate_match_request(&registry, MatchRequest::v1(plan, fetch_rows_operation()))
            .expect("released V1 validation accepts an exact subtype for a base-declared role");
    let MatchRequestAdaptation::Adapted(adapted) = adapt_match_request(
        &validated,
        &registry,
        &authority.context(),
        StructuralLimits::CANONICAL,
    )
    .expect("the exact subtype request adapts") else {
        panic!("small released request must not use the resource fallback");
    };
    let Some(ModelQueryV2::Rows { hydration, .. }) = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        panic!("released rows carry hydration authority");
    };
    let employee = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding().get() == 0)
        .expect("exact employee binding authority");
    assert_eq!(employee.declared_descriptor().label().as_str(), "employee");
    assert_eq!(
        employee
            .concrete_descriptors()
            .iter()
            .map(|descriptor| descriptor.label().as_str())
            .collect::<Vec<_>>(),
        ["employee"],
    );
    let worker = hydration
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.descriptor().label().as_str() == "employment")
        .and_then(|descriptor| descriptor.roles().first())
        .and_then(|role| role.players().first())
        .expect("base-declared worker role authority");
    assert_eq!(worker.declared_descriptor().label().as_str(), "person");
    assert_eq!(
        worker
            .concrete_descriptors()
            .iter()
            .map(|descriptor| descriptor.label().as_str())
            .collect::<Vec<_>>(),
        ["employee", "person"],
    );
}

#[test]
fn production_authority_accepts_released_annotation_and_role_specialization_shapes() {
    let registry = DescriptorRegistry::new();
    let code = OwnedAttributeDescriptor {
        field_name: "external_id".into(),
        attr_name: "external-code".into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Key],
        is_optional: false,
        is_ordered: false,
        doc: Some("stable external identifier".into()),
        meta: BTreeMap::from([("source".into(), "legacy".into())]),
    };
    let tags = OwnedAttributeDescriptor {
        field_name: "labels".into(),
        attr_name: "legacy-tag".into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Card(0, None), Annotation::Distinct],
        is_optional: true,
        is_ordered: true,
        doc: None,
        meta: BTreeMap::new(),
    };
    registry
        .register_entity(EntityDescriptor {
            type_name: "actor".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![code.clone(), tags.clone()],
            doc: Some("abstract actor".into()),
            meta: BTreeMap::new(),
        })
        .expect("register abstract entity");
    registry
        .register_entity(EntityDescriptor {
            type_name: "worker".into(),
            is_abstract: false,
            parent_type: Some("actor".into()),
            owned_attributes: vec![code, tags],
            doc: None,
            meta: BTreeMap::new(),
        })
        .expect("register entity subtype");
    let participant = RoleDescriptor {
        role_name: "participant".into(),
        player_type_names: vec!["worker".into()],
        cardinality: Some((1, None)),
        is_abstract: true,
        ordered: true,
        distinct: true,
        plays_cardinality: Some((0, Some(2))),
        doc: Some("base participant".into()),
        ..Default::default()
    };
    registry
        .register_relation(RelationDescriptor {
            type_name: "activity".into(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: Vec::new(),
            roles: vec![participant.clone()],
            doc: None,
            meta: BTreeMap::new(),
        })
        .expect("register abstract relation");
    registry
        .register_relation(RelationDescriptor {
            type_name: "task".into(),
            is_abstract: false,
            parent_type: Some("activity".into()),
            owned_attributes: Vec::new(),
            roles: vec![
                participant,
                RoleDescriptor {
                    role_name: "assignee".into(),
                    player_type_names: vec!["worker".into()],
                    cardinality: Some((1, Some(1))),
                    overrides: Some("participant".into()),
                    plays_cardinality: Some((0, Some(1))),
                    ..Default::default()
                },
            ],
            doc: None,
            meta: BTreeMap::new(),
        })
        .expect("register specialized relation");

    let generated = crate::_schema::generator::generate_define_block(
        &crate::_schema::SchemaInfo::from_descriptors(&registry.snapshot()),
    );
    type_bridge_schema_compat::released_typeql_to_declared_projection(
        DocumentId::new("adapter-rich-authority").expect("document"),
        &generated,
    )
    .unwrap_or_else(|error| {
        panic!("released annotation projection failed: {error:?}\n{generated}")
    });
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("released annotations project through schema-compat authority");
    let plan = MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:worker"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Subtypes,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("relation:task"),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: V1BindingId::new(1),
            role: V1RoleId::new(DescriptorId::new("relation:task"), "assignee"),
            player: V1BindingId::new(0),
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let operation = MatchOperation::FetchRows {
        output: FetchShape::Positional {
            slots: vec![FetchSlot::One {
                binding: V1BindingId::new(0),
            }],
        },
        order: vec![MatchOrder {
            field: BoundFieldId::new(
                V1BindingId::new(0),
                FieldId::new(DescriptorId::new("entity:worker"), "external_id"),
            ),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        }],
        window: Window {
            offset: 0,
            limit: 2,
        },
        cardinality: RowCardinality::BoundedMany,
    };
    let validated = validate_match_request(&registry, MatchRequest::v1(plan, operation))
        .expect("released annotated shape validates");
    assert!(matches!(
        adapt_match_request(
            &validated,
            &registry,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .expect("annotated shape adapts"),
        MatchRequestAdaptation::Adapted(_)
    ));
}

#[test]
fn every_released_scalar_domain_preserves_its_value_contract() {
    let cases = [
        (
            AttributeValue::String("quotes \" slashes \\\\ and newline\n".into()),
            ValueTypeTag::String,
            None,
        ),
        (AttributeValue::Long(i64::MIN), ValueTypeTag::Long, None),
        (AttributeValue::Long(i64::MAX), ValueTypeTag::Long, None),
        (AttributeValue::Double(-0.0), ValueTypeTag::Double, None),
        (AttributeValue::Double(1.5), ValueTypeTag::Double, None),
        (AttributeValue::Boolean(false), ValueTypeTag::Boolean, None),
        (
            AttributeValue::Date("2024-02-29".into()),
            ValueTypeTag::Date,
            None,
        ),
        (
            AttributeValue::DateTime("2024-01-01T12:30:00".into()),
            ValueTypeTag::DateTime,
            None,
        ),
        (
            AttributeValue::DateTime("2024-01-01T12:30".into()),
            ValueTypeTag::DateTime,
            Some("2024-01-01T12:30"),
        ),
        (
            AttributeValue::DateTimeTZ("2024-01-01T12:30:00Z".into()),
            ValueTypeTag::DateTimeTz,
            None,
        ),
        (
            AttributeValue::DateTimeTZ("2024-01-01T12:30Z".into()),
            ValueTypeTag::DateTimeTz,
            Some("2024-01-01T12:30Z"),
        ),
        (
            AttributeValue::Decimal("123.4500".into()),
            ValueTypeTag::Decimal,
            Some("123.4500"),
        ),
        (
            AttributeValue::Decimal("00123.4500dec".into()),
            ValueTypeTag::Decimal,
            Some("00123.4500dec"),
        ),
        (
            AttributeValue::Duration("P1DT7200S".into()),
            ValueTypeTag::Duration,
            None,
        ),
        (
            AttributeValue::Duration("P1DT2H".into()),
            ValueTypeTag::Duration,
            Some("P1DT2H"),
        ),
        (
            AttributeValue::Duration("P1Y".into()),
            ValueTypeTag::Duration,
            Some("P1Y"),
        ),
    ];
    for (value, expected_type, expected_released) in cases {
        let adapted = adapt_value(&value).expect("released scalar adapts");
        assert_eq!(adapted.value_type(), expected_type, "{value:?}");
        assert_eq!(
            adapted.released_text().as_deref(),
            expected_released,
            "{value:?}"
        );
    }
}

#[test]
fn every_comparator_and_nested_boolean_shape_adapts_exactly() {
    let fixture = schema_fixture();
    let comparators = [
        (ComparisonOp::Equal, QueryComparatorV2::Equal),
        (ComparisonOp::NotEqual, QueryComparatorV2::NotEqual),
        (ComparisonOp::LessThan, QueryComparatorV2::Less),
        (
            ComparisonOp::LessThanOrEqual,
            QueryComparatorV2::LessOrEqual,
        ),
        (ComparisonOp::GreaterThan, QueryComparatorV2::Greater),
        (
            ComparisonOp::GreaterThanOrEqual,
            QueryComparatorV2::GreaterOrEqual,
        ),
        (ComparisonOp::Contains, QueryComparatorV2::Contains),
        (ComparisonOp::StartsWith, QueryComparatorV2::StartsWith),
        (ComparisonOp::EndsWith, QueryComparatorV2::EndsWith),
        (ComparisonOp::Regex, QueryComparatorV2::Regex),
    ];
    for (released, expected) in comparators {
        let adapted = adapt(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: V1BindingId::new(0),
                    descriptor: DescriptorId::new("entity:person"),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: Some(MatchExpr::Not {
                    expression: Box::new(MatchExpr::Or {
                        expressions: vec![
                            MatchExpr::FieldValue {
                                field: name_field(),
                                operator: released,
                                value: AttributeValue::String(
                                    if released == ComparisonOp::Regex {
                                        "^value$"
                                    } else {
                                        "value"
                                    }
                                    .into(),
                                ),
                            },
                            MatchExpr::And {
                                expressions: vec![MatchExpr::FieldValue {
                                    field: name_field(),
                                    operator: ComparisonOp::NotEqual,
                                    value: AttributeValue::String("other".into()),
                                }],
                            },
                        ],
                    }),
                }),
                allowed_cross_joins: BTreeSet::new(),
            },
            fetch_rows_operation(),
            &fixture,
        )
        .expect("comparator shape adapts");
        let Some(QueryPatternV2::Not { pattern }) = adapted
            .validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.predicate())
        else {
            panic!("outer negation preserved");
        };
        let QueryPatternV2::Or { patterns } = pattern.as_ref() else {
            panic!("nested disjunction preserved");
        };
        assert!(matches!(
            &patterns[0],
            QueryPatternV2::FieldValue { comparator, .. } if *comparator == expected
        ));
        assert!(matches!(&patterns[1], QueryPatternV2::And { patterns } if patterns.len() == 1));
    }
}

#[test]
fn field_comparison_order_and_output_variants_keep_their_exact_contracts() {
    let fixture = schema_fixture();
    for (operator, expected_comparator) in [
        (ComparisonOp::Equal, QueryComparatorV2::Equal),
        (ComparisonOp::NotEqual, QueryComparatorV2::NotEqual),
        (ComparisonOp::LessThan, QueryComparatorV2::Less),
        (
            ComparisonOp::LessThanOrEqual,
            QueryComparatorV2::LessOrEqual,
        ),
        (ComparisonOp::GreaterThan, QueryComparatorV2::Greater),
        (
            ComparisonOp::GreaterThanOrEqual,
            QueryComparatorV2::GreaterOrEqual,
        ),
    ] {
        let plan = MatchPlan {
            bindings: vec![
                MatchBinding {
                    id: V1BindingId::new(0),
                    descriptor: DescriptorId::new("entity:person"),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                },
                MatchBinding {
                    id: V1BindingId::new(1),
                    descriptor: DescriptorId::new("entity:person"),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Subtypes,
                },
            ],
            predicate: Some(MatchExpr::FieldComparison {
                left: name_field_for(0),
                operator,
                right: name_field_for(1),
            }),
            allowed_cross_joins: BTreeSet::new(),
        };
        for (direction, missing) in [
            (SortDirection::Ascending, MissingOrder::First),
            (SortDirection::Descending, MissingOrder::Last),
            (SortDirection::Descending, MissingOrder::Reject),
        ] {
            let adapted = adapt(
                plan.clone(),
                MatchOperation::FetchRows {
                    output: FetchShape::Named {
                        slots: vec![
                            NamedFetchSlot {
                                name: "left".into(),
                                slot: FetchSlot::One {
                                    binding: V1BindingId::new(0),
                                },
                            },
                            NamedFetchSlot {
                                name: "right".into(),
                                slot: FetchSlot::One {
                                    binding: V1BindingId::new(1),
                                },
                            },
                        ],
                    },
                    order: vec![MatchOrder {
                        field: name_field_for(0),
                        direction,
                        missing,
                    }],
                    window: Window {
                        offset: 0,
                        limit: 3,
                    },
                    cardinality: RowCardinality::BoundedMany,
                },
                &fixture,
            )
            .expect("field comparison, named output, and order policy adapt");
            assert!(matches!(
                adapted
                    .validated()
                    .plan()
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.predicate()),
                Some(QueryPatternV2::FieldComparison { comparator, .. })
                    if *comparator == expected_comparator
            ));
            assert!(matches!(
                adapted
                    .validated()
                    .plan()
                    .v2_compatibility()
                    .and_then(|compatibility| compatibility.model_query()),
                Some(ModelQueryV2::Rows {
                    output: type_bridge_contract::query_plan::QueryModelOutputV2::Named { slots },
                    ..
                }) if slots.len() == 2
            ));
        }
    }
}

#[test]
fn the_complete_released_shape_inventory_adapts() {
    let fixture = schema_fixture();
    let adapt_ok = |plan: MatchPlan, operation: MatchOperation| {
        adapt(plan, operation, &fixture)
            .expect("every released semantic shape has a V2 compatibility representation")
    };

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
    let disjunction = adapt_ok(
        with_predicate(MatchExpr::Or {
            expressions: vec![
                MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::Contains,
                    value: AttributeValue::String("a".to_owned()),
                },
                MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::StartsWith,
                    value: AttributeValue::String("b".to_owned()),
                },
                MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::EndsWith,
                    value: AttributeValue::String("c".to_owned()),
                },
                MatchExpr::FieldValue {
                    field: name_field(),
                    operator: ComparisonOp::Regex,
                    value: AttributeValue::String("^d$".to_owned()),
                },
            ],
        }),
        fetch_rows_operation(),
    );
    assert!(matches!(
        disjunction
            .validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.predicate()),
        Some(QueryPatternV2::Or { patterns }) if patterns.len() == 4
    ));

    let released_string = adapt_ok(
        with_predicate(MatchExpr::FieldValue {
            field: name_field(),
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("x".repeat(1024 * 1024 + 1)),
        }),
        fetch_rows_operation(),
    );
    assert!(matches!(
        released_string
            .validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.predicate()),
        Some(QueryPatternV2::FieldValue { value, .. }) if value.released_text().is_some()
    ));

    let mut subtype_links = representative_plan();
    subtype_links.bindings[1].match_mode = MatchMode::Subtypes;
    let subtype_links = adapt_ok(subtype_links, fetch_rows_operation());
    assert!(matches!(
        subtype_links
            .validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.predicate()),
        Some(QueryPatternV2::And { patterns })
            if matches!(
                patterns.first(),
                Some(QueryPatternV2::RoleEdge {
                    include_relation_subtypes: true,
                    ..
                })
            )
    ));

    let relation = DescriptorId::new("relation:directed-edge");
    let page_plan = MatchPlan {
        bindings: vec![
            MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: V1BindingId::new(1),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
        ],
        predicate: Some(MatchExpr::Reachable {
            relation: relation.clone(),
            role_from: V1RoleId::new(relation.clone(), "origin"),
            role_to: V1RoleId::new(relation, "destination"),
            source: V1BindingId::new(0),
            target: V1BindingId::new(1),
            min_depth: 1,
            max_depth: 1,
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let page = adapt_ok(
        page_plan,
        MatchOperation::PageBy {
            root: V1BindingId::new(0),
            output: FetchShape::Named {
                slots: vec![
                    NamedFetchSlot {
                        name: "person".to_owned(),
                        slot: FetchSlot::One {
                            binding: V1BindingId::new(0),
                        },
                    },
                    NamedFetchSlot {
                        name: "destinations".to_owned(),
                        slot: FetchSlot::Collect {
                            binding: V1BindingId::new(1),
                            distinct: true,
                            order: vec![MatchOrder {
                                field: name_field_for(1),
                                direction: SortDirection::Ascending,
                                missing: MissingOrder::Reject,
                            }],
                        },
                    },
                ],
            },
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 5,
            },
            include_total: true,
        },
    );
    assert!(matches!(
        page.validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.model_query()),
        Some(ModelQueryV2::Page {
            include_total: true,
            output: type_bridge_contract::query_plan::QueryModelOutputV2::Named { slots },
            ..
        }) if slots.len() == 2
    ));

    let exactly_one = adapt_ok(
        representative_plan(),
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
                limit: 1,
            },
            cardinality: RowCardinality::ExactlyOne,
        },
    );
    assert!(matches!(
        exactly_one
            .validated()
            .plan()
            .v2_compatibility()
            .and_then(|compatibility| compatibility.model_query()),
        Some(ModelQueryV2::Rows {
            cardinality: type_bridge_contract::query_plan::QueryRowCardinalityV2::ExactlyOne,
            ..
        })
    ));

    let mut crossed = representative_plan();
    crossed
        .allowed_cross_joins
        .insert(BindingPair::new(V1BindingId::new(0), V1BindingId::new(1)));
    let crossed = adapt_ok(crossed, fetch_rows_operation());
    assert_eq!(
        crossed
            .validated()
            .plan()
            .v2_compatibility()
            .expect("compatibility")
            .allowed_cross_joins()
            .len(),
        1,
    );
}

#[test]
fn only_the_fixed_v2_artifact_envelope_uses_the_legacy_executor() {
    use crate::query_v2_adapter::V1ResourceEnvelopeReason;

    let fixture = schema_fixture();
    let registry = registry();
    let disposition = |literal: String| {
        let plan = MatchPlan {
            bindings: vec![MatchBinding {
                id: V1BindingId::new(0),
                descriptor: DescriptorId::new("entity:person"),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            }],
            predicate: Some(MatchExpr::FieldValue {
                field: name_field(),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String(literal),
            }),
            allowed_cross_joins: BTreeSet::new(),
        };
        let validated =
            validate_match_request(&registry, MatchRequest::v1(plan, fetch_rows_operation()))
                .expect("released V1 validation admits arbitrary string bytes");
        adapt_match_request(
            &validated,
            &registry,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
            StructuralLimits::CANONICAL,
        )
        .expect("resource mismatch is a typed disposition, not a semantic error")
    };

    assert!(matches!(
        disposition("x".repeat(16 * 1024 * 1024 + 1)),
        MatchRequestAdaptation::LegacyRequired(
            V1ResourceEnvelopeReason::LiteralExceedsCanonicalArtifact
        )
    ));
    assert!(matches!(
        disposition("\\".repeat(9 * 1024 * 1024)),
        MatchRequestAdaptation::LegacyRequired(
            V1ResourceEnvelopeReason::EncodedPlanExceedsCanonicalArtifact
        )
    ));
}
