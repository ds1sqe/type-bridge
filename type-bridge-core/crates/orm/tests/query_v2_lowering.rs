use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_given_rows_capability;
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, OrderDirection, OrderTerm, QueryInvocation, QueryOperand,
    QueryOperation, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SourceSpan,
    SourcedSchemaFact, SubFact, SubFactId, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::temporal::{CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_orm::GivenValue;
use type_bridge_orm::query_v2::lower_validated_query;
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};
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
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).expect("owns id"),
        )),
        // The windowed fixture plan sorts by name; the unique ownership
        // proves the sort tuple total for the visible person column.
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(OwnsFactId::new(person, name).expect("owns id")),
                    AnnotationKindId::Unique,
                ),
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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
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
                            left: QueryOperand::Binding {
                                binding: binding_id(1),
                            },
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    (validated, plan)
}

fn string_row(value: &str) -> InputRow {
    InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new(value).expect("canonical string"),
    ))])
}

fn validated_transport_query(
    columns: impl IntoIterator<Item = (&'static str, ValueTypeTag, bool)>,
) -> (ValidatedQuery, QueryPlan) {
    let person = type_id(TypeKind::Entity, "person");
    let declared = DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        [SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SourceSpan::new(
                DocumentId::new("query-v2-given-transport-fixture").expect("document"),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .expect("span"),
        )],
    )
    .expect("declared schema");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-v2-given-transport-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let inputs = columns
        .into_iter()
        .enumerate()
        .map(|(index, (name, value_type, optional))| {
            InputColumn::new(
                InputColumnId::new(u16::try_from(index).expect("input ordinal")),
                QueryVariable::new(name).expect("input name"),
                value_type,
                optional,
            )
        })
        .collect();
    let plan = QueryPlan::new(
        vec![binding(0, "person")],
        inputs,
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: person,
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("transport query plan");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated transport query");
    (validated, plan)
}

#[test]
fn single_row_inline_lowering_is_deterministic_golden_text() {
    let (validated, plan) = validated_person_query();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
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
    assert_eq!(
        lowered
            .output_schema()
            .rows()
            .expect("row plan")
            .columns()
            .len(),
        2,
    );

    let repeat = lower_validated_query(&validated, &invocation).expect("repeat lowering");
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
    // Multi-row batches lower onto the driver-transported given stage:
    // a typed header in the query text, values outside it.
    let lowered = lower_validated_query(&validated, &multi).expect("given lowering");
    assert!(
        lowered
            .typeql()
            .starts_with("given $wanted_name: string;\nmatch\n"),
        "{}",
        lowered.typeql(),
    );
    assert!(!lowered.typeql().contains("\"ada\""));
    let spec = lowered.given_rows().expect("given rows");
    assert_eq!(spec.variables, vec!["wanted_name".to_owned()]);
    assert_eq!(spec.rows.len(), 2);

    // A foreign invocation never lowers against this plan.
    let foreign_plan = QueryPlan::new(
        plan.bindings().to_vec(),
        plan.inputs().to_vec(),
        vec![plan.pipeline()[0].clone(), ReadStage::Distinct],
        plan.output().clone(),
        plan.managed_semantics().clone(),
    )
    .expect("foreign plan");
    let foreign = QueryInvocation::new(&foreign_plan, QueryOperation::Rows, vec![string_row("x")])
        .expect("foreign invocation");
    let error =
        lower_validated_query(&validated, &foreign).expect_err("invocations bind exactly one plan");
    assert_eq!(error.code().as_str(), "query_v2_invocation_plan_mismatch");
}

#[test]
fn given_lowering_carries_temporal_decimal_and_optional_absence_without_inline_text() {
    let columns = [
        ("flag", ValueTypeTag::Boolean, false),
        ("number", ValueTypeTag::Long, false),
        ("ratio", ValueTypeTag::Double, false),
        ("text", ValueTypeTag::String, false),
        ("day", ValueTypeTag::Date, false),
        ("moment", ValueTypeTag::DateTime, false),
        ("zoned", ValueTypeTag::DateTimeTz, false),
        ("amount", ValueTypeTag::Decimal, false),
        ("maybe", ValueTypeTag::String, true),
    ];
    let (validated, plan) = validated_transport_query(columns);
    let local = "2026-07-13T10:30:00"
        .parse::<CanonicalDateTime>()
        .expect("datetime");
    let row = |suffix: &str, maybe| {
        InputRow::new(vec![
            Some(CanonicalValue::Boolean(true)),
            Some(CanonicalValue::Long(42)),
            Some(CanonicalValue::Double(
                CanonicalDouble::new(1.5).expect("double"),
            )),
            Some(CanonicalValue::String(
                CanonicalString::new(format!("value-{suffix}")).expect("string"),
            )),
            Some(CanonicalValue::Date("2026-07-13".parse().expect("date"))),
            Some(CanonicalValue::DateTime(local)),
            Some(CanonicalValue::DateTimeTz(
                CanonicalDateTimeTz::new_named_resolved(local, "Europe/Amsterdam", 7_200)
                    .expect("resolved named datetime-tz"),
            )),
            Some(CanonicalValue::Decimal(
                DecimalValue::new("12.30dec").expect("decimal"),
            )),
            maybe,
        ])
    };
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![
            row("one", None),
            row(
                "two",
                Some(CanonicalValue::String(
                    CanonicalString::new("present").expect("string"),
                )),
            ),
        ],
    )
    .expect("temporal given invocation");
    assert!(
        invocation
            .transport_capabilities()
            .contains(&query_given_rows_capability())
    );

    let lowered = lower_validated_query(&validated, &invocation).expect("given lowering");
    assert!(lowered.typeql().starts_with(
        "given $flag: boolean, $number: integer, $ratio: double, $text: string, \
         $day: date, $moment: datetime, $zoned: datetime-tz, $amount: decimal, \
         $maybe: string;\nmatch\n"
    ));
    assert!(!lowered.typeql().contains("value-one"));
    let rows = &lowered.given_rows().expect("given rows").rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][4], GivenValue::Date("2026-07-13".into()));
    assert_eq!(
        rows[0][5],
        GivenValue::Datetime("2026-07-13T10:30:00".into())
    );
    assert_eq!(
        rows[0][6],
        GivenValue::DatetimeTzExact {
            local: "2026-07-13T10:30:00".into(),
            named_zone: Some("Europe/Amsterdam".into()),
            effective_offset_seconds: 7_200,
        }
    );
    assert_eq!(rows[0][7], GivenValue::Decimal("12.3".into()));
    assert_eq!(rows[0][8], GivenValue::Empty);
}

#[test]
fn single_datetime_tz_inputs_use_one_exact_given_lowering() {
    use type_bridge_contract::temporal::TimeZoneDesignator;

    let (validated, plan) = validated_transport_query([("zoned", ValueTypeTag::DateTimeTz, false)]);
    let fixed_local = "1900-01-01T12:00:00"
        .parse::<CanonicalDateTime>()
        .expect("datetime");
    let values = [
        (
            CanonicalDateTimeTz::new_named_resolved(
                "2024-07-01T12:00:00".parse().expect("named local"),
                "Europe/Amsterdam",
                7_200,
            )
            .expect("ordinary named value"),
            Some("Europe/Amsterdam"),
            "2024-07-01T12:00:00",
            7_200,
        ),
        (
            CanonicalDateTimeTz::new_named_resolved(
                "2024-07-01T12:00:00".parse().expect("named local"),
                "europe/amsterdam",
                7_200,
            )
            .expect("case-insensitive named value"),
            Some("europe/amsterdam"),
            "2024-07-01T12:00:00",
            7_200,
        ),
        (
            CanonicalDateTimeTz::new_fixed(fixed_local, TimeZoneDesignator::OffsetSeconds(1_172))
                .expect("second-resolution fixed value"),
            None,
            "1900-01-01T12:00:00",
            1_172,
        ),
        (
            CanonicalDateTimeTz::new_named_resolved(
                "2024-10-27T01:30:00".parse().expect("overlap local"),
                "Europe/London",
                3_600,
            )
            .expect("explicit earlier overlap side"),
            Some("Europe/London"),
            "2024-10-27T01:30:00",
            3_600,
        ),
    ];

    for (value, named_zone, expected_local, effective_offset_seconds) in values {
        let invocation = QueryInvocation::new(
            &plan,
            QueryOperation::Rows,
            vec![InputRow::new(vec![Some(CanonicalValue::DateTimeTz(value))])],
        )
        .expect("single datetime-tz invocation");
        assert!(
            invocation
                .transport_capabilities()
                .contains(&query_given_rows_capability())
        );
        let lowered = lower_validated_query(&validated, &invocation)
            .expect("datetime-tz uses exact driver transport");
        assert!(lowered.typeql().starts_with("given $zoned: datetime-tz;\n"));
        assert_eq!(
            lowered.given_rows().expect("given rows").rows,
            [vec![GivenValue::DatetimeTzExact {
                local: expected_local.into(),
                named_zone: named_zone.map(str::to_owned),
                effective_offset_seconds,
            }]]
        );
        assert!(!lowered.typeql().contains("Europe/Amsterdam"));
        assert!(!lowered.typeql().contains("+00:19"));
    }
}

#[test]
fn forged_or_unresolvable_named_datetime_tz_inputs_fail_before_lowering() {
    let (validated, plan) = validated_transport_query([("zoned", ValueTypeTag::DateTimeTz, false)]);
    let cases = [
        (
            "2024-07-01T12:00:00",
            "Europe/Amsterdam",
            1_172,
            "provider_datetime_tz_offset_mismatch",
        ),
        (
            "2024-07-01T12:00:00",
            "Not/A_Real_Zone",
            0,
            "unknown_named_timezone",
        ),
        (
            "2024-03-31T01:30:00",
            "Europe/London",
            3_600,
            "nonexistent_named_timezone_local_datetime",
        ),
    ];

    for (local, zone, offset, expected_code) in cases {
        let value = CanonicalDateTimeTz::new_named_resolved(
            local.parse().expect("local datetime"),
            zone,
            offset,
        )
        .expect("structurally valid carried value");
        let invocation = QueryInvocation::new(
            &plan,
            QueryOperation::Rows,
            vec![InputRow::new(vec![Some(CanonicalValue::DateTimeTz(value))])],
        )
        .expect("portable invocation contract");
        assert_eq!(
            lower_validated_query(&validated, &invocation)
                .expect_err("provider-invalid named input must fail preflight")
                .code()
                .as_str(),
            expected_code
        );
    }
}

#[test]
fn fixed_datetime_tz_utc_overflow_fails_before_given_runtime_construction() {
    use type_bridge_contract::temporal::TimeZoneDesignator;

    let (validated, plan) = validated_transport_query([("zoned", ValueTypeTag::DateTimeTz, false)]);
    for (local, offset) in [
        ("-262143-01-01T00:00:00", 86_399),
        ("+262142-12-31T23:59:59.999999999", -86_399),
    ] {
        let value = CanonicalDateTimeTz::new_fixed(
            local.parse().expect("canonical provider edge"),
            TimeZoneDesignator::OffsetSeconds(offset),
        )
        .expect("structurally valid fixed datetime-tz");
        let invocation = QueryInvocation::new(
            &plan,
            QueryOperation::Rows,
            vec![InputRow::new(vec![Some(CanonicalValue::DateTimeTz(value))])],
        )
        .expect("portable invocation contract");
        assert_eq!(
            lower_validated_query(&validated, &invocation)
                .expect_err("UTC overflow must fail before GivenRows reaches the runtime")
                .code()
                .as_str(),
            "provider_datetime_tz_out_of_range",
        );
    }
}

#[test]
fn single_row_optional_absence_uses_given_while_present_value_stays_inline() {
    let (validated, plan) = validated_transport_query([("maybe", ValueTypeTag::String, true)]);
    let absent = QueryInvocation::new(&plan, QueryOperation::Rows, vec![InputRow::new(vec![None])])
        .expect("absent optional input");
    let lowered = lower_validated_query(&validated, &absent).expect("one-row given lowering");
    assert!(lowered.typeql().starts_with("given $maybe: string;\n"));
    assert_eq!(
        lowered.given_rows().expect("given rows").rows,
        [vec![GivenValue::Empty]]
    );

    let present = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("inline")])
        .expect("present optional input");
    let lowered = lower_validated_query(&validated, &present).expect("inline lowering");
    assert!(lowered.given_rows().is_none());
    assert!(!lowered.typeql().starts_with("given "));
}

#[test]
fn optional_duration_absence_uses_an_empty_given_cell() {
    let (validated, plan) =
        validated_transport_query([("maybe_elapsed", ValueTypeTag::Duration, true)]);
    let absent = QueryInvocation::new(&plan, QueryOperation::Rows, vec![InputRow::new(vec![None])])
        .expect("absent optional duration input");

    let lowered = lower_validated_query(&validated, &absent)
        .expect("absence does not require a duration value conversion");
    assert!(
        lowered
            .typeql()
            .starts_with("given $maybe_elapsed: duration;\n")
    );
    assert_eq!(
        lowered.given_rows().expect("given rows").rows,
        [vec![GivenValue::Empty]]
    );
}

#[test]
fn duration_batches_use_lossless_given_components_through_the_driver_boundary() {
    let (validated, plan) = validated_transport_query([("elapsed", ValueTypeTag::Duration, false)]);
    let value = CanonicalValue::Duration(
        "P1DT2S"
            .parse::<CanonicalDuration>()
            .expect("canonical duration"),
    );
    let boundary = CanonicalDuration::new(
        false,
        u64::from(u32::MAX),
        u64::from(u32::MAX),
        u64::MAX / 1_000_000_000,
        u32::try_from(u64::MAX % 1_000_000_000).unwrap(),
    )
    .unwrap();
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![
            InputRow::new(vec![Some(value)]),
            InputRow::new(vec![Some(CanonicalValue::Duration(boundary))]),
        ],
    )
    .expect("duration batch is a valid portable invocation");
    let lowered = lower_validated_query(&validated, &invocation)
        .expect("provider-domain duration batches use exact components");
    assert!(lowered.typeql().starts_with("given $elapsed: duration;\n"));
    assert_eq!(
        lowered.given_rows().expect("given rows").rows,
        [
            vec![GivenValue::Duration {
                months: 0,
                days: 1,
                nanos: 2_000_000_000,
            }],
            vec![GivenValue::Duration {
                months: u32::MAX,
                days: u32::MAX,
                nanos: u64::MAX,
            }],
        ]
    );
}

#[test]
fn single_duration_inputs_enforce_the_exact_driver_domain_before_provider_use() {
    let (validated, plan) = validated_transport_query([("elapsed", ValueTypeTag::Duration, false)]);
    let invalid = [
        CanonicalDuration::new(true, 0, 1, 0, 0).unwrap(),
        CanonicalDuration::new(false, u64::from(u32::MAX) + 1, 0, 0, 0).unwrap(),
        CanonicalDuration::new(false, 0, u64::from(u32::MAX) + 1, 0, 0).unwrap(),
        CanonicalDuration::new(false, 0, 0, u64::MAX / 1_000_000_000 + 1, 0).unwrap(),
    ];
    for value in invalid {
        let invocation = QueryInvocation::new(
            &plan,
            QueryOperation::Rows,
            vec![InputRow::new(vec![Some(CanonicalValue::Duration(value))])],
        )
        .expect("portable invocation contract");
        assert_eq!(
            lower_validated_query(&validated, &invocation)
                .expect_err("provider-invalid duration must fail preflight")
                .code()
                .as_str(),
            "provider_duration_out_of_range"
        );
    }

    let boundary = CanonicalDuration::new(
        false,
        u64::from(u32::MAX),
        u64::from(u32::MAX),
        u64::MAX / 1_000_000_000,
        u32::try_from(u64::MAX % 1_000_000_000).unwrap(),
    )
    .unwrap();
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::Duration(
            boundary,
        ))])],
    )
    .expect("boundary duration invocation");
    let lowered = lower_validated_query(&validated, &invocation)
        .expect("boundary duration passes single-row preflight");
    assert!(lowered.given_rows().is_none());
}

#[test]
fn scalar_function_calls_lower_to_deterministic_let_assignments() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, TypeReference,
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
            FunctionBody::new("match $subject has name $n; let $l = length($n); return first $l;")
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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
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
                        function: FunctionId::new("person_name_length").expect("function id"),
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         let $name_length = person_name_length($person);\n\
         sort $name_length asc;\n",
    );

    let repeat = lower_validated_query(&validated, &invocation).expect("repeat lowering");
    assert_eq!(repeat, lowered);
}

#[test]
fn reduce_stages_lower_to_deterministic_grouped_reducers() {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};

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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("declared schema");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("query-v2-reduce-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);

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
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
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
                assignments: vec![ReduceAssignment::new(binding_id(1), Reducer::Count, None)],
                groups: Vec::new(),
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("global count plan");
    let validated = validate_query_plan(&global, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated global count");
    let invocation =
        QueryInvocation::new(&global, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
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
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed_try);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    assert!(
        validated
            .output_schema()
            .rows()
            .expect("row plan")
            .columns()[1]
            .optional()
    );
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
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

#[test]
fn document_outputs_lower_to_deterministic_fetch_blocks() {
    use type_bridge_contract::query_plan::{DocumentField, DocumentSource};

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
                DocumentId::new("query-v2-fetch-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-fetch-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
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
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
            ],
        }],
        QueryOutput::Documents {
            fields: vec![
                DocumentField::new(
                    QueryVariable::new("name").expect("key"),
                    DocumentSource::Binding {
                        binding: binding_id(1),
                    },
                ),
                DocumentField::new(
                    QueryVariable::new("all_names").expect("key"),
                    DocumentSource::AttributeList {
                        attribute: AttributeId::new("name").expect("attribute"),
                        owner: binding_id(0),
                    },
                ),
            ],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("document plan");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         $person has name $name;\n\
         $name isa! name;\n\
         fetch {\n\
         \x20   \"name\": $name,\n\
         \x20   \"all_names\": [ $person.name ]\n\
         };\n",
    );
    assert!(lowered.output_schema().documents().is_some());
}

#[test]
fn local_functions_lower_to_with_fun_preambles() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::query_plan::{LocalFunction, LocalReturn, Reducer};

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
                DocumentId::new("query-v2-local-fn-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-local-fn-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let plan = QueryPlan::new_with_functions(
        vec![binding(0, "person"), binding(1, "name_count")],
        vec![LocalFunction::new(
            FunctionId::new("name_count_of").expect("function id"),
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
                        function: FunctionId::new("name_count_of").expect("function id"),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Descending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("local function plan");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "with fun name_count_of($subject: person) -> integer:\n\
         match\n\
         $subject has name $value;\n\
         $value isa! name;\n\
         return count($value);\n\
         match\n\
         $person isa person;\n\
         let $name_count = name_count_of($person);\n\
         sort $name_count desc;\n",
    );
}

#[test]
fn bounded_reachability_lowers_to_unrolled_disjunctions() {
    use type_bridge_contract::id::RoleId;
    use type_bridge_contract::schema::{PlaysFact, PlaysFactId, RelatesFact, RelatesFactId};

    let node = type_id(TypeKind::Entity, "node");
    let node_child = type_id(TypeKind::Entity, "node-child");
    let edge = type_id(TypeKind::Relation, "edge");
    let from = RoleId::new("edge", "origin").expect("role");
    let to = RoleId::new("edge", "destination").expect("role");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(node.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(node_child.clone()).expect("type fact")),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(node_child, node.clone()).expect("sub fact"),
        )),
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
                DocumentId::new("query-v2-reachable-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-reachable-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let plan = QueryPlan::new_v2(
        vec![binding(0, "start"), binding(1, "finish")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Reachable {
                min_depth: 0,
                max_depth: 3,
                relation: edge.clone(),
                role_from: from.clone(),
                role_to: to.clone(),
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
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated query");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         { $R0z0 isa! node; $start is $R0z0; $finish is $R0z0; } or \
         { $R0z1 isa! node-child; $start is $R0z1; $finish is $R0z1; } or \
         { (origin: $start, destination: $finish) isa! edge; } or \
         { (origin: $start, destination: $R0l2h1) isa! edge; \
         (origin: $R0l2h1, destination: $finish) isa! edge; } or \
         { (origin: $start, destination: $R0l3h1) isa! edge; \
         (origin: $R0l3h1, destination: $R0l3h2) isa! edge; \
         (origin: $R0l3h2, destination: $finish) isa! edge; };\n\
         reduce $RreachableProofCount = count groupby $start, $finish;\n\
         select $start, $finish;\n",
    );
}

#[test]
fn hidden_negation_witnesses_lower_with_an_exact_root_select() {
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
                DocumentId::new("query-v2-witness-fixture").expect("document"),
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
        ManagedScopeId::new("query-v2-scope").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    // The hidden witness is established only inside the negation and never
    // projected; the plan carries no explicit Select stage.
    let plan = QueryPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "hidden"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
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
                    patterns: vec![
                        QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: AttributeId::new("name").expect("attribute"),
                            owner: binding_id(0),
                        },
                        QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding {
                                binding: binding_id(2),
                            },
                            right: QueryOperand::Literal {
                                value: CanonicalValue::String(
                                    CanonicalString::new("zed").expect("literal"),
                                ),
                            },
                        },
                    ],
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("witness plan");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("validated witness query");

    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered query");
    assert_eq!(
        lowered.typeql(),
        "match\n\
         $person isa person;\n\
         $person has name $name;\n\
         $name isa! name;\n\
         not {\n\
         \x20   $person has name $hidden;\n\
         \x20   $hidden isa! name;\n\
         \x20   $hidden == \"zed\";\n\
         };\n\
         select $person, $name;\n",
    );
}

#[test]
fn ordinary_disjunction_lowers_conjunction_branches_with_nested_negation() {
    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("person type")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("name type")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).expect("owns"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-disjunction-lowering").expect("document"),
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
        ManagedScopeId::new("query-v2-disjunction-lowering").expect("scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = QueryPlan::new_v2(
        vec![
            binding(0, "person"),
            binding(1, "left_name"),
            binding(2, "right_name"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Or {
                branches: vec![
                    vec![
                        QueryPattern::Isa {
                            binding: binding_id(0),
                            include_subtypes: false,
                            type_id: person.clone(),
                        },
                        QueryPattern::Has {
                            attribute: binding_id(1),
                            attribute_id: name.clone(),
                            owner: binding_id(0),
                        },
                    ],
                    vec![
                        QueryPattern::Isa {
                            binding: binding_id(0),
                            include_subtypes: false,
                            type_id: person,
                        },
                        QueryPattern::Has {
                            attribute: binding_id(2),
                            attribute_id: name,
                            owner: binding_id(0),
                        },
                        QueryPattern::Not {
                            patterns: vec![QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: QueryOperand::Binding {
                                    binding: binding_id(2),
                                },
                                right: QueryOperand::Literal {
                                    value: CanonicalValue::String(
                                        CanonicalString::new("blocked").expect("literal"),
                                    ),
                                },
                            }],
                        },
                    ],
                ],
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("disjunction plan");
    let validated = validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        StructuralLimits::CANONICAL,
    )
    .expect("validated disjunction");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("lowered disjunction");

    assert_eq!(
        lowered.typeql(),
        "match\n\
         {\n\
         \x20   $person isa! person;\n\
         \x20   $person has name $left_name;\n\
         \x20   $left_name isa! name;\n\
         }\n\
         or {\n\
         \x20   $person isa! person;\n\
         \x20   $person has name $right_name;\n\
         \x20   $right_name isa! name;\n\
         \x20   not {\n\
         \x20       $right_name == \"blocked\";\n\
         \x20   };\n\
         };\n\
         select $person;\n",
    );
}
