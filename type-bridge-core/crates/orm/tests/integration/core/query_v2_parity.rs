//! Result parity between the V1 executor and adapted V2 execution.
//!
//! This is the delegation gate: every expressible corpus request runs
//! through the released V1 executor and through adapt -> validate -> lower
//! -> execute on the same live data, and the results must agree exactly —
//! row identities in order, counts, and existence. The V1 executor stays
//! the public default until this corpus stays green.

use std::collections::BTreeSet;

use crate::common::dynamic_crud::{DynamicCrudSchema, attr, setup_dynamic_database};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::QueryInvocation;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::integration_test_support::adapt_match_request_for_live_test;
use type_bridge_orm::query_v2::{QueryRowValue, QueryV2Outcome, execute_validated_query};
use type_bridge_orm::session::backend::{
    AnswerCancellation, BoundedAnswerLimits, QueryV2AnswerLimits,
};
use type_bridge_orm::*;
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

struct ParityFixture {
    managed: ManagedSchemaState,
    person_descriptor_id: DescriptorId,
    registry: DescriptorRegistry,
    resolved: ResolvedSchema,
    schema: DynamicCrudSchema,
}

fn declared_fixture(schema: &DynamicCrudSchema) -> (ManagedSchemaState, ResolvedSchema) {
    let person = TypeId::new(TypeKind::Entity, schema.person_type.as_str()).expect("person type");
    let name = AttributeId::new(schema.name_attr.as_str()).expect("name attribute");
    let age = AttributeId::new(schema.age_attr.as_str()).expect("age attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, schema.name_attr.as_str()).expect("type"),
            )
            .expect("type fact"),
        ),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, schema.age_attr.as_str()).expect("type"),
            )
            .expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).expect("owns id"),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age).expect("owns id"),
        )),
        // The V1 registry declares the name field as a key; the declared
        // schema carries the same fact so windowed adapted plans prove
        // their sort tuple total through the unique ownership.
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(OwnsFactId::new(person, name).expect("owns id")),
                    AnnotationKindId::Key,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("key annotation"),
        ),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-parity").expect("document"),
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
        ManagedScopeId::new("query-v2-parity").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    (
        managed_schema_state(&declared, &context).expect("managed state"),
        resolve(&declared, &profile).expect("resolved schema"),
    )
}

fn name_field(fixture: &ParityFixture) -> BoundFieldId {
    BoundFieldId::new(
        BindingId::new(0),
        FieldId::new(
            fixture.person_descriptor_id.clone(),
            fixture.schema.name_attr.as_str(),
        ),
    )
}

fn age_field(fixture: &ParityFixture) -> BoundFieldId {
    BoundFieldId::new(
        BindingId::new(0),
        FieldId::new(
            fixture.person_descriptor_id.clone(),
            fixture.schema.age_attr.as_str(),
        ),
    )
}

fn person_plan(fixture: &ParityFixture, predicate: Option<MatchExpr>) -> MatchPlan {
    MatchPlan {
        bindings: vec![MatchBinding {
            id: BindingId::new(0),
            descriptor: fixture.person_descriptor_id.clone(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Subtypes,
        }],
        predicate,
        allowed_cross_joins: BTreeSet::new(),
    }
}

fn rows_operation(
    fixture: &ParityFixture,
    direction: SortDirection,
    offset: u64,
    limit: u64,
) -> MatchOperation {
    MatchOperation::FetchRows {
        output: FetchShape::Positional {
            slots: vec![FetchSlot::One {
                binding: BindingId::new(0),
            }],
        },
        order: vec![MatchOrder {
            field: name_field(fixture),
            direction,
            missing: MissingOrder::Reject,
        }],
        window: Window { offset, limit },
        cardinality: RowCardinality::BoundedMany,
    }
}

async fn v1_row_iids(db: &Database, fixture: &ParityFixture, request: MatchRequest) -> Vec<String> {
    let validated = validate_match_request(&fixture.registry, request)
        .expect("corpus request passes V1 validation");
    let result = db
        .execute_match(&fixture.registry, &validated)
        .await
        .expect("V1 execution");
    let MatchResult::Rows { rows } = result.result() else {
        panic!("expected V1 rows: {result:?}");
    };
    rows.iter()
        .map(|row| {
            let SlotValue::One(thing) = &row.slots()[0] else {
                panic!("expected one singular selected thing");
            };
            thing.concept_id().as_str().to_owned()
        })
        .collect()
}

async fn v2_outcome(
    db: &Database,
    fixture: &ParityFixture,
    request: &MatchRequest,
) -> QueryV2Outcome {
    let validated_v1 = validate_match_request(&fixture.registry, request.clone())
        .expect("corpus request passes V1 validation");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let (adapted, operation) = adapt_match_request_for_live_test(
        &validated_v1,
        &fixture.registry,
        &context,
        StructuralLimits::CANONICAL,
    )
    .expect("corpus request adapts");
    let validated = validate_query_plan(adapted.plan(), &context, StructuralLimits::CANONICAL)
        .expect("adapted corpus plan validates");
    let invocation = QueryInvocation::new(validated.plan(), operation, Vec::new())
        .expect("input-free invocation");
    let mut transaction = db.read_transaction().await.expect("read transaction");
    execute_validated_query(
        &mut transaction,
        &validated,
        &invocation,
        QueryV2AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: 1000,
                max_bytes: 1 << 20,
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
            max_collection_members: 65_536,
        },
    )
    .await
    .expect("adapted V2 execution")
}

fn v2_row_iids(outcome: &QueryV2Outcome) -> Vec<String> {
    let QueryV2Outcome::Rows(rows) = outcome else {
        panic!("expected V2 rows: {outcome:?}");
    };
    rows.iter()
        .map(|row| {
            let QueryRowValue::Thing { iid, .. } = &row.values()[0] else {
                panic!("expected the person reference column");
            };
            iid.clone()
        })
        .collect()
}

#[tokio::test]
async fn v1_and_adapted_v2_execution_agree_on_the_expressible_corpus() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("query-v2-parity").await;
    // Field names equal the live attribute labels so the V1 canonical field
    // identity and the adapter's syntactic attribute mapping name one type.
    let descriptor = std::sync::Arc::new(EntityDescriptor {
        type_name: schema.person_type.clone(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            attr(
                &schema.name_attr,
                &schema.name_attr,
                ValueType::String,
                true,
            ),
            attr(&schema.age_attr, &schema.age_attr, ValueType::Long, false),
        ],
        doc: None,
        meta: Default::default(),
    });
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    for (name, age) in [("Ada", 30i64), ("Grace", 40), ("Alan", 25)] {
        manager
            .insert(&vec![
                (
                    schema.name_attr.clone(),
                    AttributeValue::String(name.into()),
                ),
                (schema.age_attr.clone(), AttributeValue::Long(age)),
            ])
            .await
            .expect("parity fixture row");
    }
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("register person descriptor");
    let person_descriptor_id = registry
        .descriptor_id(&schema.person_type)
        .expect("person descriptor id");
    let (managed, resolved) = declared_fixture(&schema);
    let fixture = ParityFixture {
        managed,
        person_descriptor_id,
        registry,
        resolved,
        schema,
    };

    // Ordered, filtered, windowed rows agree in identity and order.
    let filtered = |direction, offset, limit| {
        MatchRequest::v1(
            person_plan(
                &fixture,
                Some(MatchExpr::FieldValue {
                    field: name_field(&fixture),
                    operator: ComparisonOp::GreaterThanOrEqual,
                    value: AttributeValue::String("Al".into()),
                }),
            ),
            rows_operation(&fixture, direction, offset, limit),
        )
    };
    for request in [
        filtered(SortDirection::Ascending, 0, 10),
        filtered(SortDirection::Descending, 0, 10),
        filtered(SortDirection::Ascending, 1, 1),
    ] {
        let v1 = v1_row_iids(&db, &fixture, request.clone()).await;
        let v2 = v2_row_iids(&v2_outcome(&db, &fixture, &request).await);
        assert_eq!(v1, v2, "row parity for {request:?}");
        assert!(!v1.is_empty(), "corpus rows are non-trivial");
    }

    // Negated and numeric predicates agree.
    let negated = MatchRequest::v1(
        person_plan(
            &fixture,
            Some(MatchExpr::And {
                expressions: vec![
                    MatchExpr::FieldValue {
                        field: age_field(&fixture),
                        operator: ComparisonOp::GreaterThan,
                        value: AttributeValue::Long(24),
                    },
                    MatchExpr::Not {
                        expression: Box::new(MatchExpr::FieldValue {
                            field: name_field(&fixture),
                            operator: ComparisonOp::Equal,
                            value: AttributeValue::String("Ada".into()),
                        }),
                    },
                ],
            }),
        ),
        rows_operation(&fixture, SortDirection::Ascending, 0, 10),
    );
    let v1 = v1_row_iids(&db, &fixture, negated.clone()).await;
    let v2 = v2_row_iids(&v2_outcome(&db, &fixture, &negated).await);
    assert_eq!(v1, v2, "negation parity");
    assert_eq!(v1.len(), 2, "Grace and Alan survive the negated filter");

    // Count and exists agree with V1 distinct-root semantics.
    let count_request = MatchRequest::v1(
        person_plan(&fixture, None),
        MatchOperation::CountBy {
            root: BindingId::new(0),
        },
    );
    let v1_count = validate_match_request(&fixture.registry, count_request.clone())
        .expect("count request validates");
    let v1_count = db
        .execute_match(&fixture.registry, &v1_count)
        .await
        .expect("V1 count execution");
    let MatchResult::Count {
        value: v1_count, ..
    } = v1_count.result()
    else {
        panic!("expected V1 count");
    };
    let v2_count = v2_outcome(&db, &fixture, &count_request).await;
    assert_eq!(v2_count, QueryV2Outcome::Count(*v1_count), "count parity");
    assert_eq!(*v1_count, 3);

    let exists = |value: &str| {
        MatchRequest::v1(
            person_plan(
                &fixture,
                Some(MatchExpr::FieldValue {
                    field: name_field(&fixture),
                    operator: ComparisonOp::Equal,
                    value: AttributeValue::String(value.into()),
                }),
            ),
            MatchOperation::ExistsBy {
                root: BindingId::new(0),
            },
        )
    };
    for (value, expected) in [("Grace", true), ("Nobody", false)] {
        let request = exists(value);
        let v1 = validate_match_request(&fixture.registry, request.clone())
            .expect("exists request validates");
        let v1 = db
            .execute_match(&fixture.registry, &v1)
            .await
            .expect("V1 exists execution");
        let MatchResult::Exists { value: v1, .. } = v1.result() else {
            panic!("expected V1 exists");
        };
        assert_eq!(*v1, expected, "V1 exists for {value:?}");
        let v2 = v2_outcome(&db, &fixture, &request).await;
        assert_eq!(v2, QueryV2Outcome::Exists(expected), "exists parity");
    }
}
