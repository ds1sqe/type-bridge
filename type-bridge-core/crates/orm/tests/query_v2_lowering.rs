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
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm,
    QueryInvocation, QueryOperand, QueryOperation, QueryOutput, QueryPattern,
    QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_orm::query_v2::lower_validated_query;
use type_bridge_query::{
    MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan,
};
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

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

fn validated_person_query() -> (ValidatedQuery, QueryPlan) {
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
                DocumentId::new("query-v2-lowering-fixture").expect("document"),
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
        ManagedScopeId::new("query-v2-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: type_id(TypeKind::Entity, "person"),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    },
                    QueryPattern::Not {
                        patterns: vec![QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding { binding: binding_id(1) },
                            right: QueryOperand::Input {
                                column: InputColumnId::new(0),
                            },
                        }],
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Distinct,
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Offset { rows: 2 },
            ReadStage::Limit { rows: 7 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("query plan");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .expect("validated query");
    (validated, plan)
}

fn string_row(value: &str) -> InputRow {
    InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new(value).expect("canonical string"),
    ))])
}

#[test]
fn single_row_inline_lowering_is_deterministic_golden_text() {
    let (validated, plan) = validated_person_query();
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
            .expect("invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         $person has name $name;\n\
         $name isa! name;\n\
         not {\n\
         \x20   $name == \"ada\";\n\
         };\n\
         select $person, $name;\n\
         distinct;\n\
         sort $name asc;\n\
         offset 2;\n\
         limit 7;\n",
    );
    assert_eq!(lowered.operation(), QueryOperation::Rows);
    assert_eq!(lowered.row_schema().columns().len(), 2);

    let repeat =
        lower_validated_query(&validated, &invocation).expect("repeat lowering");
    assert_eq!(repeat, lowered);
}

#[test]
fn multi_row_and_absent_values_reject_before_data_io() {
    let (validated, plan) = validated_person_query();
    let multi = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![string_row("ada"), string_row("grace")],
    )
    .expect("rectangular batch");
    let error = lower_validated_query(&validated, &multi)
        .expect_err("multi-row transport is capability-gated");
    assert_eq!(
        error.code().as_str(),
        "query_v2_multi_row_given_unsupported"
    );

    // A foreign invocation never lowers against this plan.
    let foreign_plan = QueryPlan::new(
        plan.bindings().to_vec(),
        plan.inputs().to_vec(),
        vec![
            plan.pipeline()[0].clone(),
            ReadStage::Distinct,
        ],
        plan.output().clone(),
        plan.managed_semantics().clone(),
    )
    .expect("foreign plan");
    let foreign =
        QueryInvocation::new(&foreign_plan, QueryOperation::Rows, vec![string_row("x")])
            .expect("foreign invocation");
    let error = lower_validated_query(&validated, &foreign)
        .expect_err("invocations bind exactly one plan");
    assert_eq!(error.code().as_str(), "query_v2_invocation_plan_mismatch");
}

#[test]
fn scalar_function_calls_lower_to_deterministic_let_assignments() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
        FunctionReturnMode, FunctionSignature, TypeReference,
    };

    let person = type_id(TypeKind::Entity, "person");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Function(FunctionFact::new(
            FunctionId::new("person_name_length").expect("function id"),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("subject").expect("parameter"),
                    TypeReference::Schema(Label::new("person").expect("label")),
                )],
                FunctionReturnMode::scalar(FunctionReturnElement::new(
                    TypeReference::Value(ValueTypeTag::Long),
                    false,
                )),
            )
            .expect("signature"),
            FunctionBody::new(
                "match $subject has name $n; let $l = length($n); return first $l;",
            )
            .expect("body"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-fn-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-fn-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name_length")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: type_id(TypeKind::Entity, "person"),
                    },
                    QueryPattern::FunctionCall {
                        arguments: vec![QueryOperand::Binding {
                            binding: binding_id(0),
                        }],
                        assigned: binding_id(1),
                        function: FunctionId::new("person_name_length")
                            .expect("function id"),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("query plan");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .expect("validated query");
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new())
        .expect("invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         let $name_length = person_name_length($person);\n\
         sort $name_length asc;\n",
    );

    let repeat =
        lower_validated_query(&validated, &invocation).expect("repeat lowering");
    assert_eq!(repeat, lowered);
}

#[test]
fn reduce_stages_lower_to_deterministic_grouped_reducers() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

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
                DocumentId::new("query-v2-reduce-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-reduce-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed);

    let plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "name_count"),
        ],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: type_id(TypeKind::Entity, "person"),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    },
                ],
            },
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(2),
                    Reducer::Count,
                    Some(binding_id(1)),
                )],
                groups: vec![binding_id(0)],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(2), OrderDirection::Descending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("reduce plan");
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .expect("validated query");
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new())
        .expect("invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         $person has name $name;\n\
         $name isa! name;\n\
         reduce $name_count = count($name) groupby $person;\n\
         sort $name_count desc;\n",
    );

    // A global bare count reduces the whole stream to one row.
    let global = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "total")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                }],
            },
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(
                    binding_id(1),
                    Reducer::Count,
                    None,
                )],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows { columns: vec![binding_id(1)] },
        managed.managed_semantic_schema().clone(),
    )
    .expect("global count plan");
    let validated =
        validate_query_plan(&global, &validation_context, StructuralLimits::CANONICAL)
            .expect("validated global count");
    let invocation = QueryInvocation::new(&global, QueryOperation::Rows, Vec::new())
        .expect("invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         reduce $total = count;\n",
    );
}

#[test]
fn try_blocks_lower_to_indented_optional_bodies() {
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
                DocumentId::new("query-v2-try-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-try-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed_try = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                QueryPattern::Try {
                    patterns: vec![QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    }],
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_try.managed_semantic_schema().clone(),
    )
    .expect("try plan");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &managed_try);
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
            .expect("validated query");
    assert!(validated.row_schema().columns()[1].optional());
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new())
        .expect("invocation");
    let lowered =
        lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         try {\n\
         \x20   $person has name $name;\n\
         \x20   $name isa! name;\n\
         };\n",
    );
}
