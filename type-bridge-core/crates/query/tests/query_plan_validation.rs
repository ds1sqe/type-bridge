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
    InputColumn, InputColumnId, OrderDirection, OrderTerm, QueryOperand, QueryOutput, QueryPattern,
    QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type")
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn person_isa(binding: u16) -> QueryPattern {
    QueryPattern::Isa {
        binding: binding_id(binding),
        include_subtypes: false,
        type_id: type_id(TypeKind::Entity, "person"),
    }
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
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
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
) -> Result<type_bridge_query::ValidatedQuery, type_bridge_contract::diagnostic::Diagnostic> {
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
                left: QueryOperand::Binding {
                    binding: binding_id(1),
                },
                right: QueryOperand::Input {
                    column: InputColumnId::new(0),
                },
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
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
}

#[test]
fn a_full_pipeline_validates_to_typed_output_columns() {
    let fixture = schema_fixture();
    let validated = person_name_plan(
        &fixture,
        vec![
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Distinct,
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Offset { rows: 0 },
            ReadStage::Limit { rows: 10 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
    )
    .expect("validated query");

    let columns = validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns();
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
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
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
                    left: QueryOperand::Binding {
                        binding: binding_id(1),
                    },
                    right: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid plan");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
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
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            b"foreign-semantics",
        )
        .expect("foreign fingerprint"),
    )
    .expect("foreign plan");
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let stale = validate_query_plan(&foreign, &context, StructuralLimits::CANONICAL)
        .expect_err("semantic fingerprints must match the validation state");
    assert_eq!(
        stale.code().as_str(),
        "query_plan_managed_semantic_mismatch"
    );

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
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid unknown-type plan");
    let error = validate_query_plan(&unknown, &context, StructuralLimits::CANONICAL)
        .expect_err("unknown schema types fail validation");
    assert_eq!(error.code().as_str(), "query_plan_unknown_type");
}

#[test]
fn scalar_function_calls_validate_and_type_their_value_bindings() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, TypeReference,
    };

    // Extend the fixture with a scalar long-returning schema function.
    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let function = FunctionFact::new(
        FunctionId::new("person_name_length").expect("function id"),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("subject").expect("parameter name"),
                TypeReference::Schema(Label::new("person").expect("type label")),
            )],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("function signature"),
        FunctionBody::new("match $subject has name $n; let $l = length($n); return first $l;")
            .expect("function body"),
    );
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name).expect("owns id"),
        )),
        SchemaFact::Function(function),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-function-fixture").expect("document"),
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
        ManagedScopeId::new("query-plan-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed =
        type_bridge_schema::managed_schema_state(&declared, &context).expect("managed state");
    let resolved = type_bridge_schema::resolve(&declared, &profile).expect("resolved schema");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

    let call = |function: &str, arguments, assigned| QueryPattern::FunctionCall {
        arguments,
        assigned: binding_id(assigned),
        function: type_bridge_contract::id::FunctionId::new(function).expect("function id"),
    };
    let plan = |patterns| {
        QueryPlan::new(
            vec![binding(0, "person"), binding(1, "name_length")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            managed.managed_semantic_schema().clone(),
        )
        .expect("structurally valid plan")
    };

    let validated = validate_query_plan(
        &plan(vec![
            person_isa(0),
            call(
                "person_name_length",
                vec![QueryOperand::Binding {
                    binding: binding_id(0),
                }],
                1,
            ),
        ]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("scalar function call validates");
    let value_column = &validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns()[1];
    assert!(value_column.domain().type_ids().is_empty());
    assert_eq!(value_column.domain().value_type(), Some(ValueTypeTag::Long));

    let unknown = validate_query_plan(
        &plan(vec![
            person_isa(0),
            call(
                "missing_function",
                vec![QueryOperand::Binding {
                    binding: binding_id(0),
                }],
                1,
            ),
        ]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("unknown functions fail closed");
    assert_eq!(unknown.code().as_str(), "query_plan_unknown_function");

    let arity = validate_query_plan(
        &plan(vec![
            person_isa(0),
            call("person_name_length", Vec::new(), 1),
        ]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("arity mismatches fail closed");
    assert_eq!(arity.code().as_str(), "query_plan_function_arity_mismatch");

    let argument = validate_query_plan(
        &plan(vec![
            person_isa(0),
            call(
                "person_name_length",
                vec![QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                }],
                1,
            ),
        ]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("schema-typed parameters require thing bindings");
    assert_eq!(
        argument.code().as_str(),
        "query_plan_function_argument_type"
    );
}

#[test]
fn reduce_stages_type_grouped_and_global_results() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

    let person = type_id(TypeKind::Entity, "person");
    let age = AttributeId::new("age").expect("attribute");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).expect("owns id"),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name.clone()).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-reduce-fixture").expect("document"),
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
        ManagedScopeId::new("query-plan-reduce-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

    let grouped = |input: &AttributeId,
                   bindings: Vec<AssertionBinding>,
                   assignments: Vec<ReduceAssignment>,
                   columns: Vec<BindingId>| {
        let plan = QueryPlan::new(
            bindings,
            Vec::new(),
            vec![
                ReadStage::Match {
                    patterns: vec![
                        person_isa(0),
                        QueryPattern::Has {
                            attribute: binding_id(1),
                            attribute_id: input.clone(),
                            owner: binding_id(0),
                        },
                    ],
                },
                ReadStage::Reduce {
                    assignments,
                    groups: vec![binding_id(0)],
                },
                ReadStage::Sort {
                    terms: vec![OrderTerm::new(binding_id(2), OrderDirection::Ascending)],
                },
            ],
            QueryOutput::Rows { columns },
            managed.managed_semantic_schema().clone(),
        )
        .expect("reduce plan");
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
    };

    // Grouped sum keeps the input scalar; mean always widens to double.
    let validated = grouped(
        &age,
        vec![
            binding(0, "person"),
            binding(1, "measure"),
            binding(2, "first_result"),
            binding(3, "second_result"),
        ],
        vec![
            ReduceAssignment::new(binding_id(2), Reducer::Sum, Some(binding_id(1))),
            ReduceAssignment::new(binding_id(3), Reducer::Mean, Some(binding_id(1))),
        ],
        vec![binding_id(0), binding_id(2), binding_id(3)],
    )
    .expect("grouped numeric reduce");
    let columns = validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns();
    assert!(!columns[0].domain().type_ids().is_empty());
    assert!(columns[1].domain().type_ids().is_empty());
    assert_eq!(columns[1].domain().value_type(), Some(ValueTypeTag::Long));
    assert!(columns[2].domain().type_ids().is_empty());
    assert_eq!(columns[2].domain().value_type(), Some(ValueTypeTag::Double));

    // Counting a thing binding needs no scalar and yields a long.
    let validated = grouped(
        &name,
        vec![
            binding(0, "person"),
            binding(1, "measure"),
            binding(2, "first_result"),
        ],
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Count,
            Some(binding_id(1)),
        )],
        vec![binding_id(0), binding_id(2)],
    )
    .expect("grouped count over strings");
    assert_eq!(
        validated
            .output_schema()
            .rows()
            .expect("row plan")
            .columns()[1]
            .domain()
            .value_type(),
        Some(ValueTypeTag::Long),
    );

    // Numeric reducers reject non-numeric scalar inputs.
    let error = grouped(
        &name,
        vec![
            binding(0, "person"),
            binding(1, "measure"),
            binding(2, "first_result"),
        ],
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Sum,
            Some(binding_id(1)),
        )],
        vec![binding_id(0), binding_id(2)],
    )
    .expect_err("sum over strings");
    assert_eq!(error.code().as_str(), "query_plan_reduce_input_domain");

    // Numeric reducers reject thing bindings without a scalar domain.
    let error = grouped(
        &age,
        vec![
            binding(0, "person"),
            binding(1, "measure"),
            binding(2, "first_result"),
        ],
        vec![ReduceAssignment::new(
            binding_id(2),
            Reducer::Max,
            Some(binding_id(0)),
        )],
        vec![binding_id(0), binding_id(2)],
    )
    .expect_err("max over entities");
    assert_eq!(error.code().as_str(), "query_plan_reduce_input_domain");
}

#[test]
fn try_blocks_type_optional_columns_and_fail_closed() {
    let person = type_id(TypeKind::Entity, "person");
    let age = AttributeId::new("age").expect("attribute");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), age.clone()).expect("owns id"),
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name.clone()).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-try-fixture").expect("document"),
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
        ManagedScopeId::new("query-plan-try-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

    let has_age = QueryPattern::Has {
        attribute: binding_id(1),
        attribute_id: age.clone(),
        owner: binding_id(0),
    };
    let build = |try_body: Vec<QueryPattern>| {
        let plan = QueryPlan::new(
            vec![binding(0, "person"), binding(1, "age")],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![person_isa(0), QueryPattern::Try { patterns: try_body }],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            managed.managed_semantic_schema().clone(),
        )
        .expect("try plan");
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
    };

    // The optional column is typed from its refined body domain.
    let validated = build(vec![has_age.clone()]).expect("optional projection");
    let columns = validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns();
    assert!(!columns[0].optional());
    assert!(columns[1].optional());
    assert_eq!(columns[1].domain().value_type(), Some(ValueTypeTag::Long));

    // A try body must correlate with the mandatory row.
    let error = build(vec![QueryPattern::Isa {
        binding: binding_id(1),
        include_subtypes: false,
        type_id: type_id(TypeKind::Attribute, "age"),
    }])
    .expect_err("uncorrelated try body");
    assert_eq!(error.code().as_str(), "query_plan_try_not_correlated");

    // A body pinning the optional binding to two attribute types is
    // impossible under the schema.
    let error = build(vec![
        has_age.clone(),
        QueryPattern::Isa {
            binding: binding_id(1),
            include_subtypes: false,
            type_id: type_id(TypeKind::Attribute, "name"),
        },
    ])
    .expect_err("impossible try domain");
    assert_eq!(error.code().as_str(), "query_plan_empty_try_domain");

    // Every body reference is established in the body or the root.
    let error = build(vec![QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Binding {
            binding: binding_id(1),
        },
        right: QueryOperand::Literal {
            value: CanonicalValue::Long(1),
        },
    }])
    .expect_err("unbound try reference");
    assert_eq!(error.code().as_str(), "query_plan_try_unbound_binding");
}

#[test]
fn document_outputs_derive_typed_field_schemas() {
    use type_bridge_contract::query_plan::{DocumentField, DocumentSource};
    use type_bridge_query::DocumentColumnShape;

    let fixture = schema_fixture();
    let key = |name: &str| QueryVariable::new(name).expect("document key");
    let build = |fields: Vec<DocumentField>| {
        let plan = QueryPlan::new(
            vec![binding(0, "person"), binding(1, "name")],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    },
                ],
            }],
            QueryOutput::Documents { fields },
            fixture.managed.managed_semantic_schema().clone(),
        )
        .expect("document plan");
        validate_query_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
            StructuralLimits::CANONICAL,
        )
    };

    // Scalar and list fields carry exact validated types.
    let validated = build(vec![
        DocumentField::new(
            key("name"),
            DocumentSource::Binding {
                binding: binding_id(1),
            },
        ),
        DocumentField::new(
            key("names"),
            DocumentSource::AttributeList {
                attribute: AttributeId::new("name").expect("attribute"),
                owner: binding_id(0),
            },
        ),
    ])
    .expect("typed document schema");
    let schema = validated
        .output_schema()
        .documents()
        .expect("document plan")
        .columns();
    assert_eq!(schema.len(), 2);
    assert_eq!(
        schema[0].shape(),
        &DocumentColumnShape::Scalar {
            value_type: ValueTypeTag::String,
            optional: false,
        },
    );
    assert!(matches!(
        schema[1].shape(),
        DocumentColumnShape::List {
            element_type: ValueTypeTag::String,
            ..
        },
    ));

    // A thing binding has no scalar to fetch.
    let error = build(vec![DocumentField::new(
        key("person"),
        DocumentSource::Binding {
            binding: binding_id(0),
        },
    )])
    .expect_err("entity binding as a scalar field");
    assert_eq!(
        error.code().as_str(),
        "query_plan_document_field_not_scalar"
    );

    // A list of an attribute no owner-domain type owns is unreachable.
    let error = build(vec![DocumentField::new(
        key("ages"),
        DocumentSource::AttributeList {
            attribute: AttributeId::new("age").expect("attribute"),
            owner: binding_id(0),
        },
    )])
    .expect_err("unowned listed attribute");
    assert_eq!(error.code().as_str(), "query_plan_unknown_attribute");
}

#[test]
fn local_functions_type_their_calls_and_fail_closed() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, Reducer};

    let fixture = schema_fixture();
    let name_count = |body_owner_label: &str| {
        LocalFunction::new(
            FunctionId::new("name_count_of").expect("function id"),
            vec![binding(0, "subject"), binding(1, "value")],
            vec![Label::new(body_owner_label).expect("label")],
            vec![QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner: binding_id(0),
            }],
            LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
        )
    };
    let build = |functions: Vec<LocalFunction>| {
        let plan = QueryPlan::new_with_functions(
            vec![binding(0, "person"), binding(1, "name_count")],
            functions,
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![
                    person_isa(0),
                    QueryPattern::FunctionCall {
                        arguments: vec![QueryOperand::Binding {
                            binding: binding_id(0),
                        }],
                        assigned: binding_id(1),
                        function: FunctionId::new("name_count_of").expect("function id"),
                    },
                ],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            fixture.managed.managed_semantic_schema().clone(),
        )
        .expect("local function plan");
        validate_query_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
            StructuralLimits::CANONICAL,
        )
    };

    // The call assigns a typed value binding from the local signature.
    let validated = build(vec![name_count("person")]).expect("validated local call");
    let columns = validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns();
    assert!(columns[1].domain().type_ids().is_empty());
    assert_eq!(columns[1].domain().value_type(), Some(ValueTypeTag::Long));

    // An unknown parameter type label fails closed.
    let error = build(vec![name_count("city")]).expect_err("unknown parameter label");
    assert_eq!(error.code().as_str(), "query_plan_unknown_function");

    // A sum over a string body binding has no numeric domain.
    let error = build(vec![LocalFunction::new(
        FunctionId::new("name_count_of").expect("function id"),
        vec![binding(0, "subject"), binding(1, "value")],
        vec![Label::new("person").expect("label")],
        vec![QueryPattern::Has {
            attribute: binding_id(1),
            attribute_id: AttributeId::new("name").expect("attribute"),
            owner: binding_id(0),
        }],
        LocalReturn::new(Reducer::Sum, binding_id(1), ValueTypeTag::Long),
    )])
    .expect_err("sum over strings");
    assert_eq!(
        error.code().as_str(),
        "query_plan_local_function_return_domain",
    );

    // A body that never references its parameter is uncorrelated.
    let error = build(vec![LocalFunction::new(
        FunctionId::new("name_count_of").expect("function id"),
        vec![
            binding(0, "subject"),
            binding(1, "other"),
            binding(2, "value"),
        ],
        vec![Label::new("person").expect("label")],
        vec![
            QueryPattern::Isa {
                binding: binding_id(1),
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            QueryPattern::Has {
                attribute: binding_id(2),
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner: binding_id(1),
            },
        ],
        LocalReturn::new(Reducer::Count, binding_id(2), ValueTypeTag::Long),
    )])
    .expect_err("uncorrelated local body");
    assert_eq!(
        error.code().as_str(),
        "query_plan_local_function_uncorrelated",
    );
}

#[test]
fn bounded_reachability_narrows_endpoints_to_role_players() {
    use type_bridge_contract::id::RoleId;
    use type_bridge_contract::schema::{PlaysFact, PlaysFactId, RelatesFact, RelatesFactId};

    let node = type_id(TypeKind::Entity, "node");
    let island = type_id(TypeKind::Entity, "island");
    let edge = type_id(TypeKind::Relation, "edge");
    let from = RoleId::new("edge", "from").expect("role");
    let to = RoleId::new("edge", "to").expect("role");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(node.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(island.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(edge.clone()).expect("type fact")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), from.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), to.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), from.clone()).expect("plays id"),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(node.clone(), to.clone()).expect("plays id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-reachable-fixture").expect("document"),
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
        ManagedScopeId::new("query-plan-reachable-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

    let build = |role_to: RoleId| {
        let plan = QueryPlan::new(
            vec![binding(0, "start"), binding(1, "finish")],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![QueryPattern::Reachable {
                    max_depth: 2,
                    relation: edge.clone(),
                    role_from: from.clone(),
                    role_to,
                    source: binding_id(0),
                    target: binding_id(1),
                }],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0), binding_id(1)],
            },
            managed.managed_semantic_schema().clone(),
        )
        .expect("reachability plan");
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
    };

    // Both endpoints narrow to the node type; the island never plays.
    let validated = build(to.clone()).expect("validated reachability");
    let columns = validated
        .output_schema()
        .rows()
        .expect("row plan")
        .columns();
    assert_eq!(
        columns[0].domain().type_ids().iter().collect::<Vec<_>>(),
        vec![&node],
    );
    assert_eq!(
        columns[1].domain().type_ids().iter().collect::<Vec<_>>(),
        vec![&node],
    );

    // A role outside the relation fails closed.
    let error = build(RoleId::new("edge", "witness").expect("role")).expect_err("unknown role");
    assert_eq!(error.code().as_str(), "query_plan_unknown_role");
}
