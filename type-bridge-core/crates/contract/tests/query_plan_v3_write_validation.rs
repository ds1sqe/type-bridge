#[path = "support/plan_v3.rs"]
#[allow(dead_code)]
mod support;

use support::*;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::query_plan::v3::{InsertThing, QueryPlanV3, Set, Stage};
use type_bridge_contract::query_plan::{QueryOutput, QueryPattern, QueryPlan, ReadStage};
use type_bridge_contract::schema::*;

#[test]
fn insert_and_put_reject_attribute_types_and_playerless_relations() {
    for (type_id, code) in [
        (
            TypeId::new(TypeKind::Attribute, "score").unwrap(),
            "query_plan_insert_attribute_type",
        ),
        (relation(), "query_plan_insert_relation_players"),
    ] {
        for put in [false, true] {
            let things = vec![InsertThing {
                binding: id(0),
                type_id: type_id.clone(),
                fields: vec![],
                players: vec![],
            }];
            let stage = if put {
                Stage::Put { things }
            } else {
                Stage::Insert { things }
            };
            let error = QueryPlanV3::new(spec(vec![stage], vec![0]), &schema()).unwrap_err();
            assert_eq!(error.code().as_str(), code);
        }
    }
}

#[test]
fn relation_insert_checks_player_type_and_accepts_same_stage_player() {
    for (player_type, accepted) in [(entity("account"), true), (entity("other"), false)] {
        let player = InsertThing {
            binding: id(0),
            type_id: player_type,
            fields: vec![],
            players: vec![],
        };
        let relation = InsertThing {
            binding: id(1),
            type_id: relation(),
            fields: vec![],
            players: vec![(role(), id(0))],
        };
        // Register all fresh types before checking players: this is one stage.
        let result = QueryPlanV3::new(
            spec(
                vec![Stage::Insert {
                    things: vec![player, relation],
                }],
                vec![1],
            ),
            &schema(),
        );
        if accepted {
            let plan = result.unwrap();
            assert_eq!(
                type_bridge_contract::query_plan::v3::decode_query_plan_v3(
                    &plan.canonical_bytes().unwrap(),
                    &schema()
                )
                .unwrap(),
                plan
            );
        } else {
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_plan_undeclared_plays"
            );
        }
    }
}

#[test]
fn v2_lift_revalidates_schema_instead_of_trusting_read_type_labels() {
    let old = QueryPlan::new_v2(
        vec![binding(0, "missing")],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![isa(0, entity("missing"))],
        }],
        QueryOutput::Rows {
            columns: vec![id(0)],
        },
        semantics(),
    )
    .unwrap();
    let original = old.canonical_bytes().unwrap();
    assert_eq!(
        old.to_v3(&schema()).unwrap_err().code().as_str(),
        "query_plan_unknown_type"
    );
    assert_eq!(old.canonical_bytes().unwrap(), original);
}

#[test]
fn subtype_inclusive_updates_check_keys_declared_on_descendants() {
    let schema = schema();
    let child_field =
        OwnsFactId::new(entity("child_account"), AttributeId::new("score").unwrap()).unwrap();
    let mut facts: Vec<_> = schema.facts().cloned().collect();
    facts.push(SchemaFact::Owns(OwnsFact::new(child_field.clone())));
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(child_field),
                AnnotationKindId::Key,
            ),
            SchemaAnnotationValue::Presence,
        )
        .unwrap(),
    ));
    let source = schema
        .source(&schema.facts().next().unwrap().id())
        .unwrap()
        .clone();
    let schema = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts
            .into_iter()
            .map(|fact| SourcedSchemaFact::new(fact, source.clone())),
    )
    .unwrap();
    for (type_id, include_subtypes, accepted) in [
        (entity("account"), false, true),
        (entity("account"), true, false),
        (entity("child_account"), false, false),
    ] {
        let result = QueryPlanV3::new(
            spec(
                vec![
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Isa {
                                binding: id(0),
                                type_id,
                                include_subtypes,
                            }
                            .into(),
                        ],
                    },
                    Stage::Update {
                        sets: vec![Set::Field {
                            owner: id(0),
                            field: field("score"),
                            operand: literal(1),
                        }],
                    },
                ],
                vec![0],
            ),
            &schema,
        );
        if accepted {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code().as_str(), "query_plan_key_update");
        }
    }
}
