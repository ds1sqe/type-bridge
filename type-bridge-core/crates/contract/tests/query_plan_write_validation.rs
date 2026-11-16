#[path = "support/plan.rs"]
#[allow(dead_code)]
mod support;

use support::*;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::query_plan::QueryPattern;
use type_bridge_contract::query_plan::{InsertThing, QueryPlan, Set, Stage};
use type_bridge_contract::schema::*;
use type_bridge_contract::value::Cardinality;

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
                Stage::Put {
                    things,
                    include_subtypes: false,
                }
            } else {
                Stage::Insert { things }
            };
            let error = QueryPlan::new(spec(vec![stage], vec![0]), &schema()).unwrap_err();
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
        let result = QueryPlan::new(
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
                type_bridge_contract::query_plan::decode_query_plan(
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
fn native_read_revalidates_schema_instead_of_trusting_type_labels() {
    let plan = spec(
        vec![Stage::Match {
            patterns: vec![isa(0, entity("missing")).into()],
        }],
        vec![0],
    );
    assert_eq!(
        QueryPlan::new(plan, &schema()).unwrap_err().code().as_str(),
        "query_plan_unknown_type"
    );
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
        let result = QueryPlan::new(
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

#[test]
fn update_rejects_multi_valued_ownerships_before_dispatch() {
    let original = schema();
    let source = original
        .source(&original.facts().next().unwrap().id())
        .unwrap()
        .clone();
    let mut facts: Vec<_> = original.facts().cloned().collect();
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(field("score")),
                AnnotationKindId::Card,
            ),
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, None).unwrap()),
        )
        .unwrap(),
    ));
    let schema = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts
            .into_iter()
            .map(|fact| SourcedSchemaFact::new(fact, source.clone())),
    )
    .unwrap();
    let error = QueryPlan::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![isa(0, entity("account")).into()],
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
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_plan_update_cardinality");
}

#[test]
fn delete_field_rejects_keys_and_required_ownerships_but_allows_replacement() {
    use type_bridge_contract::query_plan::{AttachItem, DeleteItem};
    let original = schema();
    let source = original
        .source(&original.facts().next().unwrap().id())
        .unwrap()
        .clone();
    let mut facts: Vec<_> = original.facts().cloned().collect();
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(field("score")),
                AnnotationKindId::Card,
            ),
            SchemaAnnotationValue::Cardinality(Cardinality::new(1, Some(1)).unwrap()),
        )
        .unwrap(),
    ));
    let schema = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts
            .into_iter()
            .map(|fact| SourcedSchemaFact::new(fact, source.clone())),
    )
    .unwrap();
    for (name, code) in [
        ("id", "query_plan_key_delete"),
        ("score", "query_plan_required_field_delete"),
    ] {
        let mut stages = vec![
            Stage::Match {
                patterns: vec![isa(0, entity("account")).into()],
            },
            Stage::Delete {
                items: vec![DeleteItem::Has {
                    owner: id(0),
                    field: field(name),
                }],
            },
        ];
        assert_eq!(
            QueryPlan::new(spec(stages.clone(), vec![0]), &schema)
                .unwrap_err()
                .code()
                .as_str(),
            code
        );
        if name == "score" {
            stages.push(Stage::Attach {
                items: vec![AttachItem::Has {
                    optional: false,
                    owner: id(0),
                    field: field(name),
                    operand: literal(2),
                }],
            });
            QueryPlan::new(spec(stages, vec![0]), &schema).unwrap();
        }
    }
}

#[test]
fn update_uses_effective_cardinality_of_each_possible_owner_type() {
    let original = schema();
    let source = original
        .source(&original.facts().next().unwrap().id())
        .unwrap()
        .clone();
    let child =
        OwnsFactId::new(entity("child_account"), AttributeId::new("score").unwrap()).unwrap();
    let mut facts: Vec<_> = original.facts().cloned().collect();
    facts.push(SchemaFact::Owns(OwnsFact::new(child.clone())));
    for (field, cardinality) in [
        (field("score"), Cardinality::new(0, None).unwrap()),
        (child, Cardinality::new(0, Some(1)).unwrap()),
    ] {
        facts.push(SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(AnnotationSubjectId::Owns(field), AnnotationKindId::Card),
                SchemaAnnotationValue::Cardinality(cardinality),
            )
            .unwrap(),
        ));
    }
    let schema = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts
            .into_iter()
            .map(|fact| SourcedSchemaFact::new(fact, source.clone())),
    )
    .unwrap();
    for (owner, include_subtypes, accepted) in [
        (entity("account"), false, false),
        (entity("account"), true, false),
        (entity("child_account"), false, true),
    ] {
        let result = QueryPlan::new(
            spec(
                vec![
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Isa {
                                binding: id(0),
                                type_id: owner,
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
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }
}

#[test]
fn selective_key_ownership_deletion_is_also_refused_before_dispatch() {
    use type_bridge_contract::query_plan::DeleteItem;
    let error = QueryPlan::new(
        spec(
            vec![
                Stage::Match {
                    patterns: vec![
                        isa(0, entity("account")).into(),
                        QueryPattern::Has {
                            owner: id(0),
                            attribute: id(1),
                            attribute_id: AttributeId::new("id").unwrap(),
                        }
                        .into(),
                    ],
                },
                Stage::Delete {
                    items: vec![DeleteItem::HasValue {
                        owner: id(0),
                        field: field("id"),
                        value: id(1),
                    }],
                },
            ],
            vec![0],
        ),
        &schema(),
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_plan_key_delete");
}
