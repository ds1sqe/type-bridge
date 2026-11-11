use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{InsertThing, Operation, PlanSpec, Stage, WriteReturns};
use type_bridge_contract::query_plan::{QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::schema::*;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};

pub fn id(n: u16) -> BindingId {
    BindingId::new(n).unwrap()
}
pub fn entity(name: &str) -> TypeId {
    TypeId::new(TypeKind::Entity, name).unwrap()
}
pub fn relation() -> TypeId {
    TypeId::new(TypeKind::Relation, "membership").unwrap()
}
pub fn field(name: &str) -> OwnsFactId {
    OwnsFactId::new(entity("account"), AttributeId::new(name).unwrap()).unwrap()
}
pub fn role() -> RoleId {
    RoleId::new("membership", "member").unwrap()
}
pub fn literal(n: i64) -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::Long(n),
    }
}
pub fn string(s: &str) -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::String(CanonicalString::new(s).unwrap()),
    }
}
pub fn binding(n: u16, name: &str) -> AssertionBinding {
    AssertionBinding::new(id(n), QueryVariable::new(name).unwrap())
}
pub fn isa(n: u16, type_id: TypeId) -> QueryPattern {
    QueryPattern::Isa {
        binding: id(n),
        include_subtypes: false,
        type_id,
    }
}
pub fn semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        b"plan-v3-fixture-authority",
    )
    .unwrap()
}
pub fn schema() -> DeclaredSchema {
    declare(schema_facts())
}

#[allow(dead_code)]
pub fn schema_with_ordered_role() -> DeclaredSchema {
    declare(
        schema_facts()
            .into_iter()
            .map(|fact| match fact {
                SchemaFact::Relates(role) => {
                    SchemaFact::Relates(role.with_collection_mode(CollectionMode::OrderedList))
                }
                fact => fact,
            })
            .collect(),
    )
}

#[allow(dead_code)]
pub fn schema_with_ordered_collections() -> DeclaredSchema {
    declare(
        schema_facts()
            .into_iter()
            .map(|fact| match fact {
                SchemaFact::Owns(field) => {
                    SchemaFact::Owns(field.with_collection_mode(CollectionMode::OrderedList))
                }
                SchemaFact::Relates(role) => {
                    SchemaFact::Relates(role.with_collection_mode(CollectionMode::OrderedList))
                }
                fact => fact,
            })
            .collect(),
    )
}

#[allow(dead_code)]
pub fn schema_with_ordered_and_unordered_roles() -> DeclaredSchema {
    let mut facts = schema_facts()
        .into_iter()
        .map(|fact| match fact {
            SchemaFact::Relates(role) => {
                SchemaFact::Relates(role.with_collection_mode(CollectionMode::OrderedList))
            }
            fact => fact,
        })
        .collect::<Vec<_>>();
    // This ordinary role sorts after the ordered role, testing the full completeness walk.
    let tail = RoleId::new("membership", "z-tail").unwrap();
    facts.push(SchemaFact::Relates(
        RelatesFact::new(RelatesFactId::new(relation(), tail.clone()).unwrap(), None).unwrap(),
    ));
    facts.push(SchemaFact::Plays(PlaysFact::new(
        PlaysFactId::new(entity("account"), tail).unwrap(),
    )));
    declare(facts)
}

#[allow(dead_code)]
pub fn schema_with_subtype_role() -> DeclaredSchema {
    let mut facts = schema_facts();
    let child = TypeId::new(TypeKind::Relation, "child-membership").unwrap();
    let extra = RoleId::new("child-membership", "extra").unwrap();
    facts.push(SchemaFact::Type(TypeFact::new(child.clone()).unwrap()));
    facts.push(SchemaFact::Sub(SubFact::new(
        SubFactId::new(child.clone(), relation()).unwrap(),
    )));
    facts.push(SchemaFact::Relates(
        RelatesFact::new(RelatesFactId::new(child, extra.clone()).unwrap(), None).unwrap(),
    ));
    facts.push(SchemaFact::Plays(PlaysFact::new(
        PlaysFactId::new(entity("account"), extra).unwrap(),
    )));
    declare(facts)
}

/// The shared schema plus a duration attribute `shift` on `account`, for
/// tests of the TypeDB 3.14 rules; kept out of the shared fixture schema.
pub fn schema_with_duration() -> DeclaredSchema {
    let mut facts = schema_facts();
    let shift = AttributeId::new("shift").unwrap();
    facts.push(SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Attribute, "shift").unwrap()).unwrap(),
    ));
    facts.push(SchemaFact::Value(ValueFact::new(
        ValueFactId::new(shift.clone()),
        ValueTypeTag::Duration,
    )));
    facts.push(SchemaFact::Owns(OwnsFact::new(
        OwnsFactId::new(entity("account"), shift).unwrap(),
    )));
    declare(facts)
}

fn schema_facts() -> Vec<SchemaFact> {
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(entity("account")).unwrap()),
        SchemaFact::Type(TypeFact::new(entity("abstract_account")).unwrap()),
        SchemaFact::Type(TypeFact::new(entity("other")).unwrap()),
        SchemaFact::Type(TypeFact::new(relation()).unwrap()),
        SchemaFact::Relates(
            RelatesFact::new(RelatesFactId::new(relation(), role()).unwrap(), None).unwrap(),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(entity("account"), role()).unwrap(),
        )),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Type(entity("abstract_account")),
                    AnnotationKindId::Abstract,
                ),
                SchemaAnnotationValue::Presence,
            )
            .unwrap(),
        ),
    ];
    facts.push(SchemaFact::Type(
        TypeFact::new(entity("child_account")).unwrap(),
    ));
    facts.push(SchemaFact::Sub(SubFact::new(
        SubFactId::new(entity("child_account"), entity("account")).unwrap(),
    )));
    for (name, value_type) in [("id", ValueTypeTag::String), ("score", ValueTypeTag::Long)] {
        facts.push(SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, name).unwrap()).unwrap(),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(AttributeId::new(name).unwrap()),
            value_type,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(field(name))));
    }
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(field("id")),
                AnnotationKindId::Key,
            ),
            SchemaAnnotationValue::Presence,
        )
        .unwrap(),
    ));
    for (owner, attributes) in [
        (
            entity("item"),
            vec![
                ("item-id", ValueTypeTag::String, true),
                ("stock", ValueTypeTag::Long, false),
                ("price", ValueTypeTag::Long, false),
            ],
        ),
        (
            TypeId::new(TypeKind::Relation, "purchase").unwrap(),
            vec![("purchase-id", ValueTypeTag::String, true)],
        ),
        (
            entity("doctor"),
            vec![
                ("name", ValueTypeTag::String, true),
                ("on-call", ValueTypeTag::Boolean, false),
            ],
        ),
    ] {
        facts.push(SchemaFact::Type(TypeFact::new(owner.clone()).unwrap()));
        for (name, domain, key) in attributes {
            let attribute = AttributeId::new(name).unwrap();
            facts.push(SchemaFact::Type(
                TypeFact::new(TypeId::new(TypeKind::Attribute, name).unwrap()).unwrap(),
            ));
            facts.push(SchemaFact::Value(ValueFact::new(
                ValueFactId::new(attribute.clone()),
                domain,
            )));
            let owns = OwnsFactId::new(owner.clone(), attribute).unwrap();
            facts.push(SchemaFact::Owns(OwnsFact::new(owns.clone())));
            if key {
                facts.push(SchemaFact::Annotation(
                    AnnotationFact::new(
                        AnnotationFactId::new(
                            AnnotationSubjectId::Owns(owns),
                            AnnotationKindId::Key,
                        ),
                        SchemaAnnotationValue::Presence,
                    )
                    .unwrap(),
                ));
            }
        }
    }
    let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
    for (name, player) in [("buyer", entity("account")), ("item", entity("item"))] {
        let role = RoleId::new("purchase", name).unwrap();
        facts.push(SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(purchase.clone(), role.clone()).unwrap(),
                None,
            )
            .unwrap(),
        ));
        facts.push(SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(player, role).unwrap(),
        )));
    }
    let balance = AttributeId::new("balance").unwrap();
    facts.push(SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Attribute, "balance").unwrap()).unwrap(),
    ));
    facts.push(SchemaFact::Value(ValueFact::new(
        ValueFactId::new(balance.clone()),
        ValueTypeTag::Long,
    )));
    facts.push(SchemaFact::Owns(OwnsFact::new(
        OwnsFactId::new(entity("account"), balance).unwrap(),
    )));
    facts
}

fn declare(facts: Vec<SchemaFact>) -> DeclaredSchema {
    DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        facts.into_iter().enumerate().map(|(n, fact)| {
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("plan-v3.yaml").unwrap(),
                    n as u64,
                    n as u64 + 1,
                    n as u32 + 1,
                    1,
                    n as u32 + 1,
                    2,
                )
                .unwrap(),
            )
        }),
    )
    .unwrap()
}
pub fn thing(n: u16) -> InsertThing {
    InsertThing {
        binding: id(n),
        type_id: entity("account"),
        fields: vec![
            (field("id"), string("account-b")),
            (field("score"), literal(1)),
        ],
        players: vec![],
    }
}
pub fn spec(pipeline: Vec<Stage>, output: Vec<u16>) -> PlanSpec {
    PlanSpec {
        bindings: vec![
            binding(0, "account"),
            binding(1, "peer"),
            binding(2, "total"),
        ],
        inputs: vec![],
        given_rows: None,
        operation: if pipeline.iter().any(Stage::is_write) {
            Operation::Write {
                returns: if output.is_empty() {
                    WriteReturns::Matched
                } else {
                    WriteReturns::Rows
                },
            }
        } else {
            Operation::Rows
        },
        pipeline,
        output: QueryOutput::Rows {
            columns: output.into_iter().map(id).collect(),
        },
        managed_semantics: semantics(),
        functions: vec![],
    }
}
pub fn matched() -> Stage {
    Stage::Match {
        patterns: vec![isa(0, entity("account")).into()],
    }
}

pub fn pat(n: u16, type_id: TypeId) -> type_bridge_contract::query_plan::Pattern {
    isa(n, type_id).into()
}

#[allow(dead_code)]
pub fn schema_with_attribute_subtype() -> DeclaredSchema {
    let mut facts = schema_facts();
    let child = TypeId::new(TypeKind::Attribute, "special_score").unwrap();
    let parent = TypeId::new(TypeKind::Attribute, "score").unwrap();
    facts.push(SchemaFact::Type(TypeFact::new(child.clone()).unwrap()));
    facts.push(SchemaFact::Sub(SubFact::new(
        SubFactId::new(child, parent).unwrap(),
    )));
    declare(facts)
}

#[allow(dead_code)]
pub fn schema_with_ownership_modes(abstract_attribute: bool) -> DeclaredSchema {
    let mut facts = schema_facts();
    let parent = TypeId::new(TypeKind::Attribute, "score").unwrap();
    let child = TypeId::new(TypeKind::Attribute, "special_score").unwrap();
    facts.push(SchemaFact::Type(TypeFact::new(child.clone()).unwrap()));
    facts.push(SchemaFact::Sub(SubFact::new(
        SubFactId::new(child, parent.clone()).unwrap(),
    )));
    facts.push(SchemaFact::Owns(OwnsFact::new(
        OwnsFactId::new(
            entity("child_account"),
            AttributeId::new("special_score").unwrap(),
        )
        .unwrap(),
    )));
    if abstract_attribute {
        facts.push(SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Type(parent),
                    AnnotationKindId::Abstract,
                ),
                SchemaAnnotationValue::Presence,
            )
            .unwrap(),
        ));
    }
    declare(facts)
}
