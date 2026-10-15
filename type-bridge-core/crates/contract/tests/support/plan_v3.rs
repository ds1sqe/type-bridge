use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::v3::{InsertThing, PlanSpec, Stage};
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
        pipeline,
        output: QueryOutput::Rows {
            columns: output.into_iter().map(id).collect(),
        },
        managed_semantics: semantics(),
        compatibility: None,
        functions: vec![],
    }
}
pub fn matched() -> Stage {
    Stage::Match {
        patterns: vec![isa(0, entity("account"))],
    }
}
