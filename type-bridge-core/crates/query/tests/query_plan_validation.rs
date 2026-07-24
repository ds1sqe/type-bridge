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
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueSet,
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact,
    SourceSpan, SourcedSchemaFact, SubFact, SubFactId, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::temporal::{CanonicalDateTimeTz, CanonicalDuration, TimeZoneDesignator};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema, managed_schema_state, resolve};

const TYPEQL_USER_FUNCTION_COLLISIONS: [&str; 8] = [
    "abs", "ceil", "floor", "label", "len", "max", "min", "round",
];

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
    schema_fixture_with_unique_name(true)
}

fn schema_fixture_with_unique_name(unique_name: bool) -> SchemaFixture {
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
            OwnsFactId::new(person.clone(), name.clone()).expect("owns id"),
        )),
    ];
    // Windowed fixture plans sort by name; the unique ownership proves
    // the sort tuple total for the visible person column. The
    // annotation-free variant exists to prove windows reject without it.
    let mut facts = facts;
    if unique_name {
        facts.push(SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(OwnsFactId::new(person, name).expect("owns id")),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("unique annotation"),
        ));
    }
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

fn duration_schema_fixture() -> SchemaFixture {
    let person = type_id(TypeKind::Entity, "person");
    let elapsed = AttributeId::new("elapsed").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("person type")),
        SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, "elapsed")).expect("elapsed type"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(elapsed.clone()),
            ValueTypeTag::Duration,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, elapsed).expect("owns"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-duration-comparison").expect("document"),
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
        ManagedScopeId::new("query-plan-duration-comparison").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    SchemaFixture {
        managed: managed_schema_state(&declared, &context).expect("managed state"),
        resolved: resolve(&declared, &profile).expect("resolved schema"),
    }
}

fn window_dependency_schema_fixture() -> SchemaFixture {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, TypeReference,
    };

    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let age = AttributeId::new("age").expect("attribute");
    let name_owns = OwnsFactId::new(person.clone(), name.clone()).expect("name owns");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("person type")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("name type")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("age type")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(age.clone()),
            ValueTypeTag::Long,
        )),
        SchemaFact::Owns(OwnsFact::new(name_owns.clone())),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, age).expect("age owns"),
        )),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(name_owns),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("unique name"),
        ),
        SchemaFact::Function(FunctionFact::new(
            FunctionId::new("schema_name_count").expect("function id"),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("subject").expect("parameter"),
                    TypeReference::Schema(Label::new("person").expect("type")),
                )],
                FunctionReturnMode::scalar(FunctionReturnElement::new(
                    TypeReference::Value(ValueTypeTag::Long),
                    false,
                )),
            )
            .expect("signature"),
            FunctionBody::new("match $subject has name $name; return count($name);").expect("body"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-window-dependencies").expect("document"),
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
        ManagedScopeId::new("query-plan-window-dependencies").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    SchemaFixture {
        managed: managed_schema_state(&declared, &context).expect("managed state"),
        resolved: resolve(&declared, &profile).expect("resolved schema"),
    }
}

fn validate_scalar_sort_plan(
    value_type: ValueTypeTag,
    windowed: bool,
) -> Result<type_bridge_query::ValidatedQuery, type_bridge_contract::diagnostic::Diagnostic> {
    let scalar_type = type_id(TypeKind::Attribute, "sortable-value");
    let attribute = AttributeId::new("sortable-value").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(scalar_type.clone()).expect("attribute type")),
        SchemaFact::Value(ValueFact::new(ValueFactId::new(attribute), value_type)),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-scalar-sort").expect("document"),
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
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-plan-scalar-sort").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let scalar = binding_id(0);
    let mut pipeline = vec![
        ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: scalar,
                include_subtypes: false,
                type_id: scalar_type,
            }],
        },
        ReadStage::Select {
            bindings: vec![scalar],
        },
        ReadStage::Sort {
            terms: vec![OrderTerm::new(scalar, OrderDirection::Ascending)],
        },
    ];
    if windowed {
        pipeline.push(ReadStage::Limit { rows: 1 });
    }
    let plan = QueryPlan::new(
        vec![binding(0, "sortable")],
        Vec::new(),
        pipeline,
        QueryOutput::Rows {
            columns: vec![scalar],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid scalar sort plan");
    validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
}

fn validate_polymorphic_attribute_window(
    value_type: ValueTypeTag,
    left_values: Option<Vec<CanonicalValue>>,
    right_values: Option<Vec<CanonicalValue>>,
) -> Result<type_bridge_query::ValidatedQuery, type_bridge_contract::diagnostic::Diagnostic> {
    let identifier = type_id(TypeKind::Attribute, "identifier");
    let employee_id = type_id(TypeKind::Attribute, "employee-id");
    let asset_id = type_id(TypeKind::Attribute, "asset-id");
    let identifier_attribute = AttributeId::new("identifier").expect("identifier attribute");
    let employee_attribute = AttributeId::new("employee-id").expect("employee-id attribute");
    let asset_attribute = AttributeId::new("asset-id").expect("asset-id attribute");
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(identifier.clone()).expect("identifier type")),
        SchemaFact::Type(TypeFact::new(employee_id.clone()).expect("employee-id type")),
        SchemaFact::Type(TypeFact::new(asset_id.clone()).expect("asset-id type")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(identifier_attribute),
            value_type,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(employee_attribute.clone()),
            value_type,
        )),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(asset_attribute.clone()),
            value_type,
        )),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee_id, identifier.clone()).expect("employee-id subtype"),
        )),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(asset_id, identifier.clone()).expect("asset-id subtype"),
        )),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Type(identifier.clone()),
                    AnnotationKindId::Abstract,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("abstract identifier"),
        ),
    ];
    for (attribute, values) in [
        (employee_attribute, left_values),
        (asset_attribute, right_values),
    ] {
        if let Some(values) = values {
            facts.push(SchemaFact::Annotation(
                AnnotationFact::new(
                    AnnotationFactId::new(
                        AnnotationSubjectId::Value(ValueFactId::new(attribute)),
                        AnnotationKindId::Values,
                    ),
                    SchemaAnnotationValue::Values(
                        CanonicalValueSet::new(values).expect("finite values"),
                    ),
                )
                .expect("values annotation"),
            ));
        }
    }
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-finite-polymorphic-sort").expect("document"),
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
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-plan-finite-polymorphic-sort").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let attribute = binding_id(0);
    let plan = QueryPlan::new(
        vec![binding(0, "identifier")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![QueryPattern::Isa {
                    binding: attribute,
                    include_subtypes: true,
                    type_id: identifier,
                }],
            },
            ReadStage::Select {
                bindings: vec![attribute],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(attribute, OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 1 },
        ],
        QueryOutput::Rows {
            columns: vec![attribute],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid polymorphic attribute plan");

    validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
}

fn validate_unique_owner_sort_plan(
    value_type: ValueTypeTag,
) -> Result<type_bridge_query::ValidatedQuery, type_bridge_contract::diagnostic::Diagnostic> {
    let owner_type = type_id(TypeKind::Entity, "record");
    let attribute = AttributeId::new("sort-key").expect("attribute");
    let owns = OwnsFactId::new(owner_type.clone(), attribute.clone()).expect("owns");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(owner_type.clone()).expect("owner type")),
        SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, "sort-key")).expect("attribute type"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute.clone()),
            value_type,
        )),
        SchemaFact::Owns(OwnsFact::new(owns.clone())),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(AnnotationSubjectId::Owns(owns), AnnotationKindId::Unique),
                SchemaAnnotationValue::Presence,
            )
            .expect("unique annotation"),
        ),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-unique-owner-sort").expect("document"),
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
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-plan-unique-owner-sort").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let owner = binding_id(0);
    let sort_key = binding_id(1);
    let plan = QueryPlan::new(
        vec![binding(0, "record"), binding(1, "sort_key")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: owner,
                        include_subtypes: false,
                        type_id: owner_type,
                    },
                    QueryPattern::Has {
                        attribute: sort_key,
                        attribute_id: attribute,
                        owner,
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![owner, sort_key],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(sort_key, OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 1 },
        ],
        QueryOutput::Rows {
            columns: vec![owner],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid unique-owner sort plan");
    validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
}

fn validate_temporal_literal_plan(
    value_type: ValueTypeTag,
    literal: CanonicalValue,
) -> Result<type_bridge_query::ValidatedQuery, type_bridge_contract::diagnostic::Diagnostic> {
    let person = type_id(TypeKind::Entity, "person");
    let observed = AttributeId::new("observed").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, "observed")).expect("type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(observed.clone()),
            value_type,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), observed.clone()).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("temporal-literal-plan").expect("document"),
                byte,
                byte + 1,
                u32::try_from(index + 1).expect("line"),
                1,
                u32::try_from(index + 1).expect("line"),
                2,
            )
            .expect("span"),
        )
    });
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("declared schema");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("temporal-literal-plan").expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "observed")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: observed,
                    owner: binding_id(0),
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding {
                        binding: binding_id(1),
                    },
                    right: QueryOperand::Literal { value: literal },
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("query plan");
    validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
}

#[test]
fn temporal_query_literals_validate_before_typeql_lowering() {
    let local = "2024-07-01T12:00:00".parse().expect("local datetime");
    let named = CanonicalDateTimeTz::new_named_resolved(local, "Europe/Amsterdam", 7_200)
        .expect("ordinary named value");
    validate_temporal_literal_plan(ValueTypeTag::DateTimeTz, CanonicalValue::DateTimeTz(named))
        .expect("unambiguous named literal is provider-valid");

    let fixed = CanonicalDateTimeTz::new_fixed(
        "1900-01-01T12:00:00".parse().expect("fixed local datetime"),
        TimeZoneDesignator::OffsetSeconds(1_172),
    )
    .expect("second-resolution fixed value");
    assert_eq!(
        validate_temporal_literal_plan(
            ValueTypeTag::DateTimeTz,
            CanonicalValue::DateTimeTz(fixed),
        )
        .expect_err("fixed seconds cannot enter TypeQL")
        .code()
        .as_str(),
        "provider_datetime_tz_literal_offset_precision"
    );

    let overlap = CanonicalDateTimeTz::new_named_resolved(
        "2024-10-27T01:30:00".parse().expect("overlap local"),
        "Europe/London",
        3_600,
    )
    .expect("explicit overlap side");
    assert_eq!(
        validate_temporal_literal_plan(
            ValueTypeTag::DateTimeTz,
            CanonicalValue::DateTimeTz(overlap),
        )
        .expect_err("named TypeQL cannot spell an overlap side")
        .code()
        .as_str(),
        "ambiguous_named_timezone_local_datetime"
    );

    for duration in [
        CanonicalDuration::new(true, 0, 1, 0, 0).unwrap(),
        CanonicalDuration::new(false, u64::from(u32::MAX) + 1, 0, 0, 0).unwrap(),
    ] {
        assert_eq!(
            validate_temporal_literal_plan(
                ValueTypeTag::Duration,
                CanonicalValue::Duration(duration),
            )
            .expect_err("provider-invalid duration literal")
            .code()
            .as_str(),
            "provider_duration_out_of_range"
        );
    }
}

#[test]
fn duration_ordered_comparisons_fail_in_every_pattern_scope() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, Reducer};

    let fixture = duration_schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let elapsed = AttributeId::new("elapsed").expect("attribute");
    let comparison = |comparator| QueryPattern::Value {
        comparator,
        left: QueryOperand::Binding {
            binding: binding_id(1),
        },
        right: QueryOperand::Literal {
            value: CanonicalValue::Duration(
                CanonicalDuration::new(false, 0, 1, 0, 0).expect("duration"),
            ),
        },
    };
    let has_elapsed = || QueryPattern::Has {
        attribute: binding_id(1),
        attribute_id: elapsed.clone(),
        owner: binding_id(0),
    };
    let validate_root = |patterns| {
        let plan = QueryPlan::new(
            vec![binding(0, "person"), binding(1, "elapsed")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            fixture.managed.managed_semantic_schema().clone(),
        )
        .expect("duration comparison plan");
        validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
    };

    for comparator in [ValueComparator::Equal, ValueComparator::NotEqual] {
        validate_root(vec![person_isa(0), has_elapsed(), comparison(comparator)])
            .expect("duration equality remains provider-defined");
    }
    for comparator in [
        ValueComparator::Less,
        ValueComparator::LessOrEqual,
        ValueComparator::Greater,
        ValueComparator::GreaterOrEqual,
    ] {
        let error = validate_root(vec![person_isa(0), has_elapsed(), comparison(comparator)])
            .expect_err("ordered duration comparison");
        assert_eq!(
            error.code().as_str(),
            "query_plan_value_comparator_unsupported"
        );
    }

    let nested = validate_root(vec![
        person_isa(0),
        has_elapsed(),
        QueryPattern::Not {
            patterns: vec![comparison(ValueComparator::Less)],
        },
    ])
    .expect_err("nested ordered duration comparison");
    assert_eq!(
        nested.code().as_str(),
        "query_plan_value_comparator_unsupported"
    );

    let optional = validate_root(vec![
        person_isa(0),
        QueryPattern::Try {
            patterns: vec![has_elapsed(), comparison(ValueComparator::Less)],
        },
    ])
    .expect_err("optional ordered duration comparison");
    assert_eq!(
        optional.code().as_str(),
        "query_plan_value_comparator_unsupported"
    );

    let function_id = FunctionId::new("elapsed_count").expect("function id");
    let local = LocalFunction::new(
        function_id.clone(),
        vec![binding(0, "subject"), binding(1, "elapsed")],
        vec![Label::new("person").expect("label")],
        vec![has_elapsed(), comparison(ValueComparator::Less)],
        LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
    );
    let plan = QueryPlan::new_with_functions(
        vec![binding(0, "person"), binding(1, "elapsed_count")],
        vec![local],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                QueryPattern::FunctionCall {
                    arguments: vec![QueryOperand::Binding {
                        binding: binding_id(0),
                    }],
                    assigned: binding_id(1),
                    function: function_id,
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("local duration comparison plan");
    let local_error = validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
        .expect_err("local ordered duration comparison");
    assert_eq!(
        local_error.code().as_str(),
        "query_plan_value_comparator_unsupported"
    );
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
    let long_source = FunctionFact::new(
        FunctionId::new("long_source").expect("function id"),
        FunctionSignature::new(
            Vec::new(),
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("function signature"),
        FunctionBody::new("match let $value = 1; return first $value;").expect("function body"),
    );
    let long_identity = FunctionFact::new(
        FunctionId::new("long_identity").expect("function id"),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("value").expect("parameter name"),
                TypeReference::Value(ValueTypeTag::Long),
            )],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("function signature"),
        FunctionBody::new("match let $result = $value; return first $result;")
            .expect("function body"),
    );
    let builtin_functions = TYPEQL_USER_FUNCTION_COLLISIONS.map(|name| {
        FunctionFact::new(
            FunctionId::new(name).expect("contextual function id"),
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
            FunctionBody::new("match $subject isa person; return count($subject);")
                .expect("function body"),
        )
    });
    let mut facts = vec![
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
        SchemaFact::Function(long_source),
        SchemaFact::Function(long_identity),
    ];
    facts.extend(builtin_functions.into_iter().map(SchemaFact::Function));
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
    let plan_with_columns = |patterns, columns| {
        QueryPlan::new(
            vec![binding(0, "person"), binding(1, "name_length")],
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            QueryOutput::Rows { columns },
            managed.managed_semantic_schema().clone(),
        )
        .expect("structurally valid plan")
    };
    let plan = |patterns| plan_with_columns(patterns, vec![binding_id(0), binding_id(1)]);

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

    for builtin in TYPEQL_USER_FUNCTION_COLLISIONS {
        let collision = validate_query_plan(
            &plan(vec![
                person_isa(0),
                call(
                    builtin,
                    vec![QueryOperand::Binding {
                        binding: binding_id(0),
                    }],
                    1,
                ),
            ]),
            &validation_context,
            StructuralLimits::CANONICAL,
        )
        .expect_err("TypeQL built-ins cannot resolve as schema function calls");
        assert_eq!(
            collision.category(),
            type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
        );
        assert_eq!(
            collision.code().as_str(),
            "query_plan_builtin_function_collision",
        );
        assert_eq!(
            collision.message(),
            "TypeQL 3.12 built-in function names cannot identify schema calls or plan-local functions",
        );
    }

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

    validate_query_plan(
        &plan(vec![person_isa(0), call("long_source", Vec::new(), 1)]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("a zero-argument scalar call is a singleton beside a graph match");

    validate_query_plan(
        &plan(vec![
            person_isa(0),
            call(
                "long_identity",
                vec![QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                }],
                1,
            ),
        ]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("a literal-only scalar call is a singleton beside a graph match");

    let input_singleton = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "input_identity")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("source_value").expect("input name"),
            ValueTypeTag::Long,
            false,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                call(
                    "long_identity",
                    vec![QueryOperand::Input {
                        column: InputColumnId::new(0),
                    }],
                    1,
                ),
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("input singleton plan");
    validate_query_plan(
        &input_singleton,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("an input-only scalar call is a singleton beside a graph match");

    let binding_argument_cross_product = QueryPlan::new(
        vec![
            binding(0, "left_person"),
            binding(1, "name_length"),
            binding(2, "right_person"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                call(
                    "person_name_length",
                    vec![QueryOperand::Binding {
                        binding: binding_id(0),
                    }],
                    1,
                ),
                person_isa(2),
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1), binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("binding-argument cross-product plan");
    let error = validate_query_plan(
        &binding_argument_cross_product,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("a binding argument does not connect an independent graph component");
    assert_eq!(error.code().as_str(), "query_plan_disconnected_topology");

    let graph_cross_product_with_singleton = QueryPlan::new(
        vec![
            binding(0, "left_person"),
            binding(1, "right_person"),
            binding(2, "source_value"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                person_isa(0),
                person_isa(1),
                call("long_source", Vec::new(), 2),
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1), binding_id(2)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("graph cross-product plan with singleton");
    let error = validate_query_plan(
        &graph_cross_product_with_singleton,
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("a singleton call does not connect independent graph components");
    assert_eq!(error.code().as_str(), "query_plan_disconnected_topology");

    let source = || call("long_source", Vec::new(), 0);
    let identity = |argument, assigned| {
        call(
            "long_identity",
            vec![QueryOperand::Binding {
                binding: binding_id(argument),
            }],
            assigned,
        )
    };

    // Match conjunction order is declarative: a consumer may precede the
    // call that establishes its input binding.
    let reversed = validate_query_plan(
        &plan(vec![identity(0, 1), source()]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("function dependencies are order independent");
    assert!(
        reversed
            .output_schema()
            .rows()
            .expect("row plan")
            .columns()
            .iter()
            .all(|column| column.domain().value_type() == Some(ValueTypeTag::Long)),
    );

    let unbound = validate_query_plan(
        &plan_with_columns(vec![identity(0, 1)], vec![binding_id(1)]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("a function argument is a reference, not a producer");
    assert_eq!(unbound.code().as_str(), "query_plan_binding_not_positive");

    let self_cycle = validate_query_plan(
        &plan(vec![person_isa(0), identity(1, 1)]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("self-dependent calls fail closed");
    assert_eq!(
        self_cycle.code().as_str(),
        "query_plan_function_dependency_cycle"
    );

    let cycle = validate_query_plan(
        &plan(vec![identity(1, 0), identity(0, 1)]),
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect_err("cyclic call dependencies fail closed");
    assert_eq!(
        cycle.code().as_str(),
        "query_plan_function_dependency_cycle"
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
    let build_with_columns = |try_body: Vec<QueryPattern>, columns: Vec<BindingId>| {
        let plan = QueryPlan::new(
            vec![binding(0, "person"), binding(1, "age")],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![person_isa(0), QueryPattern::Try { patterns: try_body }],
            }],
            QueryOutput::Rows { columns },
            managed.managed_semantic_schema().clone(),
        )
        .expect("try plan");
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
    };
    let build = |try_body| build_with_columns(try_body, vec![binding_id(0), binding_id(1)]);

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
    let error = build_with_columns(
        vec![QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left: QueryOperand::Binding {
                binding: binding_id(1),
            },
            right: QueryOperand::Literal {
                value: CanonicalValue::Long(1),
            },
        }],
        vec![binding_id(0)],
    )
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

    for builtin in TYPEQL_USER_FUNCTION_COLLISIONS {
        let plan = QueryPlan::new_with_functions(
            vec![binding(0, "person")],
            vec![LocalFunction::new(
                FunctionId::new(builtin).expect("contextual function id"),
                vec![binding(0, "subject"), binding(1, "value")],
                vec![Label::new("person").expect("label")],
                vec![QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                }],
                LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
            )],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![person_isa(0)],
            }],
            QueryOutput::Rows {
                columns: vec![binding_id(0)],
            },
            fixture.managed.managed_semantic_schema().clone(),
        )
        .expect("structurally valid contextual local function name");
        let collision = validate_query_plan(
            &plan,
            &MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed),
            StructuralLimits::CANONICAL,
        )
        .expect_err("TypeQL built-ins cannot identify plan-local functions");
        assert_eq!(
            collision.category(),
            type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
        );
        assert_eq!(
            collision.code().as_str(),
            "query_plan_builtin_function_collision",
        );
        assert_eq!(
            collision.message(),
            "TypeQL 3.12 built-in function names cannot identify schema calls or plan-local functions",
        );
    }

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
    let from = RoleId::new("edge", "origin").expect("role");
    let to = RoleId::new("edge", "destination").expect("role");
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

#[test]
fn windows_without_a_proven_total_order_are_rejected() {
    // Two persons may share a name when the ownership is not unique: the
    // sorted page order among them is provider-defined, so the window is
    // refused instead of paging nondeterministically.
    let fixture = schema_fixture_with_unique_name(false);
    let error = person_name_plan(
        &fixture,
        vec![
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
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
    .expect_err("duplicate sort keys must not admit a window");
    assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");

    // Dropping the tied thing column from the row environment restores
    // the proof: the remaining sorted attribute column is value-identified.
    person_name_plan(
        &fixture,
        vec![
            ReadStage::Select {
                bindings: vec![binding_id(1)],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Offset { rows: 0 },
            ReadStage::Limit { rows: 10 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
    )
    .expect("a fully sorted scalar environment is total");
}

#[test]
fn reduce_windows_accept_vacuous_global_and_complete_group_dependencies() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

    let fixture = window_dependency_schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);

    // A global reduce emits at most one row. Sorting either result therefore
    // gives a total order for both results before applying the window.
    let global = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "row_count"),
            binding(2, "person_count"),
        ],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![person_isa(0)],
            },
            ReadStage::Reduce {
                assignments: vec![
                    ReduceAssignment::new(binding_id(1), Reducer::Count, None),
                    ReduceAssignment::new(binding_id(2), Reducer::Count, Some(binding_id(0))),
                ],
                groups: Vec::new(),
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 1 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1), binding_id(2)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("global reduce plan");
    validate_query_plan(&global, &context, StructuralLimits::CANONICAL)
        .expect("a one-row global reduce has a vacuously total order");

    let unordered_global = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "row_count")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![person_isa(0)],
            },
            ReadStage::Reduce {
                assignments: vec![ReduceAssignment::new(binding_id(1), Reducer::Count, None)],
                groups: Vec::new(),
            },
            ReadStage::Limit { rows: 1 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect_err("even a global reduce keeps the explicit-sort wire contract");
    assert_eq!(
        unordered_global.code().as_str(),
        "query_plan_unordered_truncation"
    );

    let grouped = |complete_sort: bool| {
        let mut terms = vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)];
        if complete_sort {
            terms.push(OrderTerm::new(binding_id(2), OrderDirection::Ascending));
        }
        QueryPlan::new(
            vec![
                binding(0, "person"),
                binding(1, "name"),
                binding(2, "age"),
                binding(3, "person_count"),
            ],
            Vec::new(),
            vec![
                ReadStage::Match {
                    patterns: vec![
                        person_isa(0),
                        QueryPattern::Has {
                            attribute: binding_id(1),
                            attribute_id: AttributeId::new("name").expect("attribute"),
                            owner: binding_id(0),
                        },
                        QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: AttributeId::new("age").expect("attribute"),
                            owner: binding_id(0),
                        },
                    ],
                },
                ReadStage::Reduce {
                    assignments: vec![ReduceAssignment::new(
                        binding_id(3),
                        Reducer::Count,
                        Some(binding_id(0)),
                    )],
                    groups: vec![binding_id(1), binding_id(2)],
                },
                ReadStage::Sort { terms },
                ReadStage::Limit { rows: 10 },
            ],
            QueryOutput::Rows {
                columns: vec![binding_id(1), binding_id(2), binding_id(3)],
            },
            fixture.managed.managed_semantic_schema().clone(),
        )
        .expect("grouped reduce plan")
    };

    validate_query_plan(&grouped(true), &context, StructuralLimits::CANONICAL)
        .expect("the complete identity-total group tuple determines every reducer result");
    let incomplete = validate_query_plan(&grouped(false), &context, StructuralLimits::CANONICAL)
        .expect_err("an incomplete group sort can leave distinct aggregate rows tied");
    assert_eq!(
        incomplete.code().as_str(),
        "query_plan_window_order_not_total"
    );
}

#[test]
fn local_function_results_follow_determined_arguments_but_schema_calls_do_not() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, Reducer};

    let fixture = window_dependency_schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let local_function_id = FunctionId::new("local_name_count").expect("function id");
    let local_function = LocalFunction::new(
        local_function_id.clone(),
        vec![binding(0, "subject"), binding(1, "name")],
        vec![Label::new("person").expect("type")],
        vec![QueryPattern::Has {
            attribute: binding_id(1),
            attribute_id: AttributeId::new("name").expect("attribute"),
            owner: binding_id(0),
        }],
        LocalReturn::new(Reducer::Count, binding_id(1), ValueTypeTag::Long),
    );
    let patterns = |function| {
        vec![
            person_isa(0),
            QueryPattern::Has {
                attribute: binding_id(1),
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner: binding_id(0),
            },
            QueryPattern::FunctionCall {
                arguments: vec![QueryOperand::Binding {
                    binding: binding_id(0),
                }],
                assigned: binding_id(2),
                function,
            },
        ]
    };
    let pipeline = |patterns| {
        vec![
            ReadStage::Match { patterns },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 10 },
        ]
    };
    let output = || QueryOutput::Rows {
        columns: vec![binding_id(0), binding_id(1), binding_id(2)],
    };

    let local = QueryPlan::new_with_functions(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "name_count"),
        ],
        vec![local_function],
        Vec::new(),
        pipeline(patterns(local_function_id)),
        output(),
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("local function window plan");
    validate_query_plan(&local, &context, StructuralLimits::CANONICAL)
        .expect("a closed local aggregate result follows its determined argument tuple");

    let schema = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "name_count"),
        ],
        Vec::new(),
        pipeline(patterns(
            FunctionId::new("schema_name_count").expect("function id"),
        )),
        output(),
        fixture.managed.managed_semantic_schema().clone(),
    )
    .expect("schema function window plan");
    let error = validate_query_plan(&schema, &context, StructuralLimits::CANONICAL)
        .expect_err("schema signatures make no determinism claim");
    assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");
}

#[test]
fn sort_rejects_provider_unorderable_duration_values() {
    let error =
        validate_scalar_sort_plan(ValueTypeTag::Duration, false).expect_err("duration sort");
    assert_eq!(error.code().as_str(), "query_plan_sort_not_orderable");
}

#[test]
fn windows_reject_scalar_identities_that_can_tie_under_provider_order() {
    for value_type in [ValueTypeTag::Double, ValueTypeTag::DateTimeTz] {
        let error = validate_scalar_sort_plan(value_type, true)
            .expect_err("non-injective provider order must not admit a window");
        assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");
    }
    // TypeDB forbids @unique on doubles; datetime-tz is the admissible
    // non-injective domain that exercises the unique-owner proof.
    let owner_error = validate_unique_owner_sort_plan(ValueTypeTag::DateTimeTz)
        .expect_err("non-injective values must not prove unique-owner page order");
    assert_eq!(
        owner_error.code().as_str(),
        "query_plan_window_order_not_total"
    );

    validate_scalar_sort_plan(ValueTypeTag::Long, true)
        .expect("an injectively ordered scalar domain admits a window");
    validate_unique_owner_sort_plan(ValueTypeTag::Long)
        .expect("an injectively ordered unique attribute determines its owner");
}

#[test]
fn windows_accept_disjoint_exhaustive_polymorphic_value_domains() {
    let string = |value| {
        CanonicalValue::String(CanonicalString::new(value).expect("canonical string value"))
    };
    let validated = validate_polymorphic_attribute_window(
        ValueTypeTag::String,
        Some(vec![string("employee-a"), string("employee-b")]),
        Some(vec![string("asset-a"), string("asset-b")]),
    )
    .expect("disjoint exhaustive value domains prove a total polymorphic order");
    assert_eq!(
        validated
            .binding_domain(&binding_id(0))
            .expect("identifier domain")
            .type_ids()
            .len(),
        2,
    );
}

#[test]
fn windows_reject_open_or_overlapping_polymorphic_value_domains() {
    let string = |value| {
        CanonicalValue::String(CanonicalString::new(value).expect("canonical string value"))
    };
    for (left, right, reason) in [
        (
            Some(vec![string("employee"), string("shared")]),
            Some(vec![string("asset"), string("shared")]),
            "overlapping finite domains",
        ),
        (
            Some(vec![string("employee")]),
            None,
            "one open provider domain",
        ),
        (None, None, "two open provider domains"),
    ] {
        let error = validate_polymorphic_attribute_window(ValueTypeTag::String, left, right)
            .expect_err(reason);
        assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");
    }
}

#[test]
fn windows_reject_comparison_equivalent_polymorphic_value_representations() {
    let positive_zero = CanonicalValue::Double(CanonicalDouble::new(0.0).expect("positive zero"));
    let negative_zero = CanonicalValue::Double(CanonicalDouble::new(-0.0).expect("negative zero"));
    let error = validate_polymorphic_attribute_window(
        ValueTypeTag::Double,
        Some(vec![positive_zero]),
        Some(vec![negative_zero]),
    )
    .expect_err("signed zero identities compare equal at the provider");
    assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");

    let utc = CanonicalValue::DateTimeTz(
        CanonicalDateTimeTz::new_fixed(
            "2024-07-01T12:00:00".parse().expect("UTC local datetime"),
            TimeZoneDesignator::Utc,
        )
        .expect("UTC datetime"),
    );
    let offset = CanonicalValue::DateTimeTz(
        CanonicalDateTimeTz::new_fixed(
            "2024-07-01T13:00:00"
                .parse()
                .expect("offset local datetime"),
            TimeZoneDesignator::OffsetSeconds(3_600),
        )
        .expect("offset datetime"),
    );
    let error = validate_polymorphic_attribute_window(
        ValueTypeTag::DateTimeTz,
        Some(vec![utc]),
        Some(vec![offset]),
    )
    .expect_err("distinct timezone representations can denote the same instant");
    assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");
}

#[test]
fn independent_unique_owns_scopes_do_not_prove_a_total_union_order() {
    // TypeDB enforces each unique owns declaration within its declaring owner
    // hierarchy. An unrelated person and company may therefore own the same
    // name even when both direct owns facts are independently @unique.
    let person = type_id(TypeKind::Entity, "person");
    let company = type_id(TypeKind::Entity, "company");
    let name = AttributeId::new("name").expect("attribute");
    let person_owns = OwnsFactId::new(person.clone(), name.clone()).expect("person owns");
    let company_owns = OwnsFactId::new(company.clone(), name.clone()).expect("company owns");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person).expect("person type")),
        SchemaFact::Type(TypeFact::new(company).expect("company type")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("name type")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(person_owns.clone())),
        SchemaFact::Owns(OwnsFact::new(company_owns.clone())),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(person_owns),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("person unique"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(company_owns),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("company unique"),
        ),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-plan-independent-unique-fixture").expect("document"),
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
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-plan-independent-unique-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = QueryPlan::new(
        vec![binding(0, "owner"), binding(1, "name")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: name,
                    owner: binding_id(0),
                }],
            },
            ReadStage::Select {
                bindings: vec![binding_id(0), binding_id(1)],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
            ReadStage::Limit { rows: 10 },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("structurally valid union plan");

    let error = validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
    .expect_err("independent uniqueness scopes cannot order the owner union");
    assert_eq!(error.code().as_str(), "query_plan_window_order_not_total");
}
