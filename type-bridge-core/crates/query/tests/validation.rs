use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, AssertionRolePlayer, BindingId,
    MigrationAssertionPlan, QueryVariable, ValueComparator, ValueOperand,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact,
    RelatesFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_query::{MigrationAssertionValidationContext, validate_migration_assertion_plan};
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

struct SchemaFixture {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

fn schema_fixture_with_extra_type(extra_type: bool) -> SchemaFixture {
    let person = type_id(TypeKind::Entity, "person");
    let company = type_id(TypeKind::Entity, "company");
    let name = AttributeId::new("name").expect("attribute");
    let employment = type_id(TypeKind::Relation, "employment");
    let employee = RoleId::new("employment", "employee").expect("role");
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(company).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name).expect("owns id"),
        )),
        SchemaFact::Type(TypeFact::new(employment.clone()).expect("type fact")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(employment, employee.clone()).expect("relates id"),
                None,
            )
            .expect("relates fact"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person, employee).expect("plays id"),
        )),
    ];
    if extra_type {
        facts.push(SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Entity, "department")).expect("extra type fact"),
        ));
    }
    let sourced = facts
        .into_iter()
        .enumerate()
        .map(|(index, fact)| {
            let byte = u64::try_from(index).expect("byte");
            let line = u32::try_from(index + 1).expect("line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("query-fixture").expect("document"),
                    byte,
                    byte + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("span"),
            )
        })
        .collect::<Vec<_>>();
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("declared schema");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-fixture").expect("managed scope"),
            profile,
            CapabilitySet::new(),
        ),
    )
    .expect("managed schema state");
    SchemaFixture { managed, resolved }
}

fn schema_fixture() -> SchemaFixture {
    schema_fixture_with_extra_type(false)
}

fn valid_plan(managed_semantics: &ManagedSemanticSchemaFingerprint) -> MigrationAssertionPlan {
    let person = BindingId::new(0).expect("binding");
    let name = BindingId::new(1).expect("binding");
    let employment = BindingId::new(2).expect("binding");
    MigrationAssertionPlan::new(
        vec![
            binding(0, "person"),
            binding(1, "name"),
            binding(2, "employment"),
        ],
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Has {
                attribute: name,
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner: person,
            },
            AssertionPattern::Links {
                players: vec![AssertionRolePlayer::new(
                    RoleId::new("employment", "employee").expect("role"),
                    person,
                )],
                relation: employment,
                relation_id: type_id(TypeKind::Relation, "employment"),
            },
            AssertionPattern::Value {
                comparator: ValueComparator::Equal,
                left: ValueOperand::binding(name),
                right: ValueOperand::literal(CanonicalValue::String(
                    CanonicalString::new("Ada").expect("literal"),
                )),
            },
        ],
        vec![person],
        vec![name, employment],
        managed_semantics.clone(),
        AssertionExpectation::NoRows,
    )
    .expect("valid plan")
}

#[test]
fn resolved_ownership_roles_values_and_row_schema_validate() {
    let fixture = schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let validated = validate_migration_assertion_plan(
        &valid_plan(fixture.managed.managed_semantic_schema()),
        &context,
        StructuralLimits::CANONICAL,
    )
    .expect("validated assertion");
    assert_eq!(validated.row_schema().columns().len(), 1);
    assert_eq!(
        validated.row_schema().columns()[0].variable().as_str(),
        "person"
    );
    assert_eq!(validated.witnesses().len(), 2);
    assert_eq!(
        validated
            .binding_domain(&BindingId::new(1).expect("binding"))
            .expect("name domain")
            .value_type(),
        Some(ValueTypeTag::String)
    );

    let mismatched = valid_plan(
        &ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            b"different-managed-selection",
        )
        .expect("different managed fingerprint"),
    );
    assert_eq!(
        validate_migration_assertion_plan(&mismatched, &context, StructuralLimits::CANONICAL,)
            .expect_err("managed semantic mismatch")
            .code()
            .as_str(),
        "migration_assertion_managed_semantic_mismatch"
    );

    let different = schema_fixture_with_extra_type(true);
    let incoherent =
        MigrationAssertionValidationContext::new(&different.resolved, &fixture.managed);
    assert_eq!(
        validate_migration_assertion_plan(
            &valid_plan(fixture.managed.managed_semantic_schema()),
            &incoherent,
            StructuralLimits::CANONICAL,
        )
        .expect_err("declared identity mismatch")
        .code()
        .as_str(),
        "migration_assertion_declared_identity_mismatch"
    );
}

#[test]
fn negation_body_locals_support_completeness_and_do_not_escape() {
    let fixture = schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let person = BindingId::new(0).expect("binding");
    let name = BindingId::new(1).expect("binding");
    let has_name = AssertionPattern::Has {
        attribute: name,
        attribute_id: AttributeId::new("name").expect("attribute"),
        owner: person,
    };
    let completeness = MigrationAssertionPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Not {
                patterns: vec![
                    has_name.clone(),
                    AssertionPattern::Not {
                        patterns: vec![AssertionPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: ValueOperand::binding(name),
                            right: ValueOperand::literal(CanonicalValue::String(
                                CanonicalString::new("Ada").expect("literal"),
                            )),
                        }],
                    },
                ],
            },
        ],
        vec![person],
        vec![name],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("completeness assertion");
    let validated =
        validate_migration_assertion_plan(&completeness, &context, StructuralLimits::CANONICAL)
            .expect("negation-local binding validates");
    assert!(validated.binding_domain(&name).is_none());

    let root_escape = MigrationAssertionPlan::new(
        completeness.bindings().to_vec(),
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Not {
                patterns: vec![has_name.clone()],
            },
            AssertionPattern::Value {
                comparator: ValueComparator::Equal,
                left: ValueOperand::binding(name),
                right: ValueOperand::literal(CanonicalValue::String(
                    CanonicalString::new("Ada").expect("literal"),
                )),
            },
        ],
        vec![person],
        vec![name],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid root escape");
    assert_eq!(
        validate_migration_assertion_plan(&root_escape, &context, StructuralLimits::CANONICAL,)
            .expect_err("local binding escaped to root")
            .code()
            .as_str(),
        "migration_assertion_binding_not_positive"
    );

    let nested_escape = MigrationAssertionPlan::new(
        completeness.bindings().to_vec(),
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Not {
                patterns: vec![
                    AssertionPattern::Not {
                        patterns: vec![has_name],
                    },
                    AssertionPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: ValueOperand::binding(name),
                        right: ValueOperand::literal(CanonicalValue::String(
                            CanonicalString::new("Ada").expect("literal"),
                        )),
                    },
                ],
            },
        ],
        vec![person],
        vec![name],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid nested escape");
    assert_eq!(
        validate_migration_assertion_plan(&nested_escape, &context, StructuralLimits::CANONICAL,)
            .expect_err("nested local binding escaped")
            .code()
            .as_str(),
        "migration_assertion_negation_unbound_binding"
    );
}

#[test]
fn ownership_role_and_value_domain_mismatches_fail_closed() {
    let fixture = schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let owner = BindingId::new(0).expect("binding");
    let attribute = BindingId::new(1).expect("binding");
    let invalid_ownership = MigrationAssertionPlan::new(
        vec![binding(0, "company"), binding(1, "name")],
        vec![
            AssertionPattern::Isa {
                binding: owner,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "company"),
            },
            AssertionPattern::Has {
                attribute,
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner,
            },
        ],
        vec![owner],
        vec![attribute],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid ownership");
    assert!(
        validate_migration_assertion_plan(
            &invalid_ownership,
            &context,
            StructuralLimits::CANONICAL
        )
        .is_err()
    );

    let mut value_mismatch = valid_plan(fixture.managed.managed_semantic_schema());
    let bytes = value_mismatch.canonical_bytes().expect("bytes");
    let _ = bytes;
    value_mismatch = MigrationAssertionPlan::new(
        value_mismatch.bindings().to_vec(),
        vec![
            AssertionPattern::Isa {
                binding: owner,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Has {
                attribute,
                attribute_id: AttributeId::new("name").expect("attribute"),
                owner,
            },
            AssertionPattern::Value {
                comparator: ValueComparator::Equal,
                left: ValueOperand::binding(attribute),
                right: ValueOperand::literal(CanonicalValue::Long(1)),
            },
            AssertionPattern::Isa {
                binding: BindingId::new(2).expect("binding"),
                include_subtypes: false,
                type_id: type_id(TypeKind::Relation, "employment"),
            },
        ],
        vec![owner],
        vec![attribute, BindingId::new(2).expect("binding")],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid mismatch");
    assert!(
        validate_migration_assertion_plan(&value_mismatch, &context, StructuralLimits::CANONICAL)
            .is_err()
    );
}

#[test]
fn negation_witness_topology_and_caller_limits_fail_closed() {
    let fixture = schema_fixture();
    let context = MigrationAssertionValidationContext::new(&fixture.resolved, &fixture.managed);
    let person = BindingId::new(0).expect("binding");
    let hidden = BindingId::new(1).expect("binding");
    let negation_only = MigrationAssertionPlan::new(
        vec![binding(0, "person"), binding(1, "hidden")],
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Not {
                patterns: vec![AssertionPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: ValueOperand::binding(hidden),
                    right: ValueOperand::literal(CanonicalValue::String(
                        CanonicalString::new("unbound").expect("literal"),
                    )),
                }],
            },
        ],
        vec![person],
        vec![hidden],
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid negation");
    assert_eq!(
        validate_migration_assertion_plan(&negation_only, &context, StructuralLimits::CANONICAL,)
            .expect_err("negation-only witness")
            .code()
            .as_str(),
        "migration_assertion_negation_unbound_binding"
    );

    let disconnected = MigrationAssertionPlan::new(
        vec![binding(0, "person"), binding(1, "company")],
        vec![
            AssertionPattern::Isa {
                binding: person,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "person"),
            },
            AssertionPattern::Isa {
                binding: hidden,
                include_subtypes: false,
                type_id: type_id(TypeKind::Entity, "company"),
            },
        ],
        vec![person, hidden],
        Vec::new(),
        fixture.managed.managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("contract-valid disconnected plan");
    assert_eq!(
        validate_migration_assertion_plan(&disconnected, &context, StructuralLimits::CANONICAL,)
            .expect_err("disconnected topology")
            .code()
            .as_str(),
        "migration_assertion_disconnected_topology"
    );

    let limits = StructuralLimits {
        selected_slots: 0,
        ..StructuralLimits::CANONICAL
    };
    assert_eq!(
        validate_migration_assertion_plan(
            &valid_plan(fixture.managed.managed_semantic_schema()),
            &context,
            limits,
        )
        .expect_err("caller limit")
        .code()
        .as_str(),
        "migration_assertion_validation_limit"
    );
}
