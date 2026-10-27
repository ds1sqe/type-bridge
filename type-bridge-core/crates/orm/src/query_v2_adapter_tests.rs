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
use crate::match_request::{
    BindingId as V1BindingId, BindingPair, BoundFieldId, ComparisonOp, DescriptorId, FetchShape,
    FetchSlot, FieldId, MatchBinding, MatchExpr, MatchMode, MatchOperation, MatchOrder, MatchPlan,
    MatchRequest, MissingOrder, NamedFetchSlot, RoleEdgeId, RoleId as V1RoleId, RowCardinality,
    SortDirection, ThingKind, Window, validate_match_request,
};
use crate::query_v2::lower_validated_query;
use crate::query_v2_adapter::{
    AdaptedMatchRequest, MatchRequestAdapterAuthority, adapt_match_request, adapt_value,
};
use crate::query_v2_compatibility::{
    CompatibilityProviderPlan, lower_validated_compatibility_query,
};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{DiagnosticCategory, DiagnosticDetailValue};
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
    adapt_match_request(&validated, &registry, &context)
}

/// Lower one adapted request to the provider plan the executor runs.
fn provider_plan(adapted: &AdaptedMatchRequest) -> CompatibilityProviderPlan {
    let invocation = QueryInvocation::new(
        adapted.validated().plan(),
        adapted.operation(),
        adapted.inputs().to_vec(),
    )
    .expect("adapted invocation");
    lower_validated_compatibility_query(adapted.validated(), &invocation, adapted.operation())
        .expect("compatibility lowering succeeds")
        .expect("adapter-authored plan has one compatibility provider plan")
        .provider_plan()
        .clone()
}

#[test]
fn every_released_operation_lowers_to_its_provider_plan() {
    let registry = registry();
    let authority = MatchRequestAdapterAuthority::from_registry(&registry)
        .expect("production authority projects");
    let one = FetchShape::Positional {
        slots: vec![FetchSlot::One {
            binding: V1BindingId::new(0),
        }],
    };
    let lower = |plan: MatchPlan, operation: MatchOperation| {
        let validated = validate_match_request(&registry, MatchRequest::v1(plan, operation))
            .expect("released operation validates");
        let adapted = adapt_match_request(&validated, &registry, &authority.context())
            .expect("released operation adapts");
        provider_plan(&adapted)
    };
    let rows = |window: Window, cardinality: RowCardinality| {
        lower(
            representative_plan(),
            MatchOperation::FetchRows {
                output: one.clone(),
                order: Vec::new(),
                window,
                cardinality,
            },
        )
    };
    let page = |window: Window, include_total: bool| {
        lower(
            representative_plan(),
            MatchOperation::PageBy {
                root: V1BindingId::new(0),
                output: one.clone(),
                order: Vec::new(),
                window,
                include_total,
            },
        )
    };

    // A bounded window is sorted and cut by the provider.
    let CompatibilityProviderPlan::Rows {
        statement,
        tuple_proof: None,
    } = rows(
        Window {
            offset: 3,
            limit: 7,
        },
        RowCardinality::BoundedMany,
    )
    else {
        panic!("bounded rows lower to one row statement")
    };
    assert!(statement.distinct);
    assert!(!statement.order.is_empty());
    assert_eq!((statement.offset, statement.limit), (3, 7));

    // Exactly-one asks for two rows, unordered, and proves the tuple count.
    let CompatibilityProviderPlan::Rows {
        statement,
        tuple_proof: Some(proof),
    } = rows(
        Window {
            offset: 0,
            limit: 1,
        },
        RowCardinality::ExactlyOne,
    )
    else {
        panic!("exactly-one lowers to one row statement with its proof")
    };
    assert!(statement.order.is_empty());
    assert_eq!((statement.offset, statement.limit), (0, 2));
    assert_eq!(proof, statement);

    // Complete rows ask for one row past the accepted count, unordered.
    let CompatibilityProviderPlan::Rows {
        statement,
        tuple_proof: None,
    } = rows(
        Window {
            offset: 0,
            limit: 7,
        },
        RowCardinality::Complete,
    )
    else {
        panic!("complete rows lower to one row statement")
    };
    assert!(statement.order.is_empty());
    assert_eq!((statement.offset, statement.limit), (0, 8));

    for include_total in [false, true] {
        let CompatibilityProviderPlan::Page {
            selection,
            total,
            rematch,
        } = page(
            Window {
                offset: 1,
                limit: 2,
            },
            include_total,
        )
        else {
            panic!("a page lowers to a root selection and a rematch")
        };
        assert_eq!(selection.root, 0);
        assert_eq!((selection.offset, selection.limit), (Some(1), Some(2)));
        assert_eq!(total.is_some(), include_total);
        assert_eq!(rematch.root, 0);
    }

    let CompatibilityProviderPlan::DistinctCount { scan } = lower(
        representative_plan(),
        MatchOperation::CountBy {
            root: V1BindingId::new(0),
        },
    ) else {
        panic!("count lowers to a distinct root scan")
    };
    assert_eq!((scan.root, scan.offset, scan.limit), (0, None, None));

    let CompatibilityProviderPlan::DistinctExists { scan } = lower(
        representative_plan(),
        MatchOperation::ExistsBy {
            root: V1BindingId::new(0),
        },
    ) else {
        panic!("exists lowers to a one-root scan")
    };
    assert_eq!((scan.root, scan.limit), (0, Some(1)));

    assert!(matches!(
        lower(inherited_role_plan(), fetch_rows_operation()),
        CompatibilityProviderPlan::Rows { .. }
    ));
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

    let error = adapt_match_request(&validated, &foreign, &context)
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
        let adapted = adapt_match_request(&validated, &registry, &authority.context())
            .expect("production authority adapts its own validated registry");
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
    let adapted = adapt_match_request(&validated, &registry, &authority.context())
        .expect("lexical subtype order is canonicalized");
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
    let adapted = adapt_match_request(&validated, &registry, &authority.context())
        .expect("the exact subtype request adapts");
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
                    ordered: true,
                    plays_cardinality: Some((0, Some(1))),
                    ..Default::default()
                },
            ],
            doc: None,
            meta: BTreeMap::new(),
        })
        .expect("register specialized relation");

    let generated = crate::_schema::SchemaInfo::from_descriptors(&registry.snapshot())
        .to_typeql()
        .expect("fixture schema");
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
    adapt_match_request(&validated, &registry, &authority.context())
        .expect("annotated shape adapts");
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
fn a_request_beyond_the_artifact_envelope_is_refused_with_its_limit() {
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
        let limits = type_bridge_contract::limits::QueryLimits::from_overrides([(
            "canonical_string_bytes",
            16 * 1024 * 1024,
        )])
        .unwrap();
        let validated = crate::match_request::validate_match_request_with_limits(
            &registry,
            MatchRequest::v1(plan, fetch_rows_operation()),
            limits,
        )
        .expect("released V1 validation admits arbitrary string bytes");
        adapt_match_request(
            &validated,
            &registry,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
        )
    };

    let cause = |literal: String| match disposition(literal) {
        Err(cause) => cause,
        Ok(_) => panic!("oversized request adapted"),
    };

    let literal = cause("x".repeat(16 * 1024 * 1024 + 1));
    assert_eq!(literal.category(), DiagnosticCategory::ResourceLimit);
    assert_eq!(
        literal.code().as_str(),
        "query_v2_adapter_literal_byte_limit"
    );
    assert_eq!(
        literal.details().get("actual_bytes"),
        Some(&DiagnosticDetailValue::Long(16 * 1024 * 1024 + 1))
    );
    assert_eq!(
        literal.details().get("maximum_bytes"),
        Some(&DiagnosticDetailValue::Long(16 * 1024 * 1024))
    );

    let encoded = cause("\\".repeat(9 * 1024 * 1024));
    assert_eq!(encoded.category(), DiagnosticCategory::ResourceLimit);
    assert_eq!(encoded.code().as_str(), "canonical_json_too_large");
}

fn conjunction_request(
    predicate: MatchExpr,
) -> Result<AdaptedMatchRequest, type_bridge_contract::diagnostic::Diagnostic> {
    let fixture = schema_fixture();
    let registry = registry();
    let plan = MatchPlan {
        bindings: vec![MatchBinding {
            id: V1BindingId::new(0),
            descriptor: DescriptorId::new("entity:person"),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        }],
        predicate: Some(predicate),
        allowed_cross_joins: BTreeSet::new(),
    };
    let validated =
        validate_match_request(&registry, MatchRequest::v1(plan, fetch_rows_operation()))
            .expect("conjunction passes V1 validation");
    adapt_match_request(
        &validated,
        &registry,
        &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
    )
}

fn name_at_least(index: usize) -> MatchExpr {
    MatchExpr::FieldValue {
        field: name_field(),
        operator: ComparisonOp::GreaterThanOrEqual,
        value: AttributeValue::String(format!("A{index}")),
    }
}

#[test]
fn a_wide_flat_conjunction_fits_the_canonical_artifact() {
    // The width of the two-anchor automotive query that a depth-linear
    // conjunction could not encode.
    let flat = MatchExpr::And {
        expressions: (0..38).map(name_at_least).collect(),
    };
    conjunction_request(flat).expect("a wide flat conjunction adapts");
}

#[test]
fn a_plan_nested_past_the_codec_depth_reports_that_limit() {
    let mut nested = name_at_least(0);
    for index in 1..38 {
        nested = MatchExpr::And {
            expressions: vec![nested, name_at_least(index)],
        };
    }
    let cause = conjunction_request(nested)
        .expect_err("a plan deeper than the canonical codec cannot adapt");
    assert_eq!(cause.category(), DiagnosticCategory::ResourceLimit);
    assert_eq!(cause.code().as_str(), "canonical_json_too_deep");
    assert_eq!(
        cause.details().get("maximum_depth"),
        Some(&DiagnosticDetailValue::Long(64))
    );
}

fn adapt_large_string(
    limits: type_bridge_contract::limits::QueryLimits,
) -> Result<AdaptedMatchRequest, type_bridge_contract::diagnostic::Diagnostic> {
    let fixture = schema_fixture();
    let registry = registry();
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
            value: AttributeValue::String("x".repeat(1024 * 1024 + 1)),
        }),
        allowed_cross_joins: BTreeSet::new(),
    };
    let request = crate::match_request::validate_match_request_with_limits(
        &registry,
        MatchRequest::v1(plan, fetch_rows_operation()),
        limits,
    )
    .unwrap();
    adapt_match_request(
        &request,
        &registry,
        &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
    )
}

#[test]
fn typed_strings_default_to_one_mib_with_a_named_refusal() {
    let error =
        adapt_large_string(type_bridge_contract::limits::QueryLimits::CANONICAL).unwrap_err();
    assert_eq!(error.code().as_str(), "query_string_value_byte_limit");
    assert_eq!(
        error.details().get("limit"),
        Some(&DiagnosticDetailValue::Text(
            "canonical_string_bytes".into()
        ))
    );
    assert_eq!(
        error.details().get("maximum"),
        Some(&DiagnosticDetailValue::Long(1024 * 1024))
    );
}

#[test]
fn typed_strings_honor_an_explicitly_raised_string_limit() {
    let limits = type_bridge_contract::limits::QueryLimits::from_overrides([(
        "canonical_string_bytes",
        2 * 1024 * 1024,
    )])
    .unwrap();
    let adapted = adapt_large_string(limits).unwrap();
    assert!(
        matches!(adapted.validated().plan().v2_compatibility().and_then(|compatibility| compatibility.predicate()), Some(QueryPatternV2::FieldValue { value, .. }) if value.released_text().is_some())
    );
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
            assert!(plan.compatibility().is_none());
            assert!(!plan.writes());
            crate::query_plan_exec::lower(plan).unwrap();
        }
    }
}

fn native_stack_answers(
    stack: &crate::match_request::plan_adapter::AdaptedMatchRequest,
) -> Vec<crate::query_plan_exec::PlanAnswer> {
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::query_plan::v3::{Operation, Stage};
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
        if let Some(type_bridge_contract::query_plan::v3::Stage::Fetch(document)) =
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
    let database_name = format!("tb270_04a_native_{}", std::process::id());
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
            let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows {rows} = result.selection().to_staged_v3(stack.selection_plan()).unwrap() else { panic!("generic selection rows"); };
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

fn generic_model_selection(
    columns: &[u16],
    given: bool,
) -> (
    Arc<DescriptorRegistry>,
    type_bridge_contract::query_plan::v3::QueryPlanV3,
) {
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::v3::{Expr, Operation, PlanSpec, QueryPlanV3, Stage};
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
        type_bridge_contract::query_plan::v3::Pattern::Let {
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
    let plan = QueryPlanV3::new_with_limits(
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
            compatibility: None,
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
    use type_bridge_contract::query_plan::v3::Stage;
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
    use type_bridge_contract::query_plan::v3::Stage;
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
    use type_bridge_contract::query_plan::v3::Stage;
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
    let source = type_bridge_contract::query_plan::v3::decode_query_plan_v3_with_limits(
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
