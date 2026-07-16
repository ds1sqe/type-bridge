use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, OrderDirection, OrderTerm, QueryOperand,
    QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_query::{
    MigrationAssertionValidationContext, validate_query_plan,
};
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve,
};

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type")
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

struct SchemaFixture {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

fn schema_fixture() -> SchemaFixture {
    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-fixture").expect("document"),
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
        ManagedScopeId::new("query-plan-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    SchemaFixture {
        managed: managed_schema_state(&declared, &context).expect("managed state"),
        resolved: resolve(&declared, &profile).expect("resolved schema"),
    }
}

fn person_name_plan(
    fixture: &SchemaFixture,
    pipeline_tail: Vec<ReadStage>,
    output: QueryOutput,
) -> Result<
    type_bridge_query::ValidatedQuery,
    type_bridge_contract::diagnostic::Diagnostic,
> {
    let mut pipeline = vec![ReadStage::Match {
        patterns: vec![
            QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner: binding_id(0),
            },
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding { binding: binding_id(1) },
                right: QueryOperand::Input { column: InputColumnId::new(0) },
            },
        ],
    }];
    pipeline.extend(pipeline_tail);
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        pipeline,
        output,
        fixture.managed.managed_semantic_schema().clone(),
    )?;
    let context =
        MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
}

#[test]
fn a_full_pipeline_validates_to_typed_output_columns() {
    let fixture = schema_fixture();
    let validated = person_name_plan(
        &fixture,
        vec![
            ReadStage::Select { bindings: vec![binding_id(0), binding_id(1)] },
            ReadStage::Distinct,
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Offset { rows: 0 },
            ReadStage::Limit { rows: 10 },
        ],
        QueryOutput::Rows { columns: vec![binding_id(0), binding_id(1)] },
    )
    .expect("validated query");

    let columns = validated.row_schema().columns();
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].variable().as_str(), "person");
    assert_eq!(columns[1].variable().as_str(), "name");
    assert_eq!(columns[1].domain().value_type(), Some(ValueTypeTag::String));
    assert!(columns[0].domain().value_type().is_none());
    assert_eq!(
        validated
            .binding_domain(&binding_id(0))
            .expect("person domain")
            .type_ids()
            .len(),
        1,
    );
    assert_eq!(validated.source_state(), &fixture.managed);
}

#[test]
fn sort_keys_require_a_uniform_scalar_domain() {
    let fixture = schema_fixture();
    let error = person_name_plan(
        &fixture,
        vec![ReadStage::Sort {
            terms: vec![OrderTerm::new(binding_id(0), OrderDirection::Ascending)],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
    )
    .expect_err("entity bindings carry no scalar order");
    assert_eq!(error.code().as_str(), "query_plan_sort_not_scalar");
}

#[test]
fn input_operands_type_check_against_their_declarations() {
    let fixture = schema_fixture();
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::Long,
            false,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: false,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: binding_id(1) },
                    right: QueryOperand::Input { column: InputColumnId::new(0) },
                },
            ],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid plan");
    let context =
        MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let error = validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
        .expect_err("a long input cannot compare with a string attribute");
    assert_eq!(error.code().as_str(), "query_plan_value_domain_mismatch");
}

#[test]
fn stale_semantics_and_unknown_types_fail_closed() {
    let fixture = schema_fixture();
    let foreign = QueryPlan::new(
        vec![binding(0, "person")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            }],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
        type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            b"foreign-semantics",
        )
        .expect("foreign fingerprint"),
    )
    .expect("foreign plan");
    let context =
        MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let stale = validate_query_plan(&foreign, &context, StructuralLimits::CANONICAL)
        .expect_err("semantic fingerprints must match the validation state");
    assert_eq!(stale.code().as_str(), "query_plan_managed_semantic_mismatch");

    let unknown = QueryPlan::new(
        vec![binding(0, "ghost")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "ghost"),
            }],
        }],
        QueryOutput::Rows { columns: vec![binding_id(0)] },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid unknown-type plan");
    let error = validate_query_plan(&unknown, &context, StructuralLimits::CANONICAL)
        .expect_err("unknown schema types fail validation");
    assert_eq!(error.code().as_str(), "query_plan_unknown_type");
}
