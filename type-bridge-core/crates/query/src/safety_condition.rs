//! Lower verifier-derived safety conditions into validated assertion plans.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, BindingId, MigrationAssertionPlan,
    QueryVariable, ValueComparator, ValueOperand,
};
use type_bridge_schema::{RequiredSafetyCondition, SafetyCondition, ScalarSafetySubject};

use crate::{
    MigrationAssertionValidationContext, ValidatedMigrationAssertionPlan,
    validate_migration_assertion_plan,
};

/// Lower one trusted verifier-derived condition and validate it against source state.
pub fn lower_condition_to_plan(
    condition: &RequiredSafetyCondition,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<ValidatedMigrationAssertionPlan, Diagnostic> {
    validate_condition_source(condition, context)?;
    let plan = match condition.condition() {
        SafetyCondition::NoInstances {
            type_id,
            include_subtypes,
        } => no_instances_plan(type_id, *include_subtypes, context)?,
        SafetyCondition::OwnsMinimum { owns, minimum } => {
            owns_minimum_plan(owns, *minimum, context)?
        }
        SafetyCondition::OwnsMaximum { owns, maximum } => {
            owns_maximum_plan(owns, *maximum, context, limits)?
        }
        SafetyCondition::RangeLower { subject, lower } => scalar_condition_plan(
            subject,
            AssertionPattern::Value {
                comparator: ValueComparator::Less,
                left: ValueOperand::binding(scalar_value_binding(subject)),
                right: ValueOperand::literal(lower.clone()),
            },
            context,
        )?,
        SafetyCondition::RangeUpper { subject, upper } => scalar_condition_plan(
            subject,
            AssertionPattern::Value {
                comparator: ValueComparator::Greater,
                left: ValueOperand::binding(scalar_value_binding(subject)),
                right: ValueOperand::literal(upper.clone()),
            },
            context,
        )?,
        SafetyCondition::ValuesNarrowed { subject, allowed } => {
            let value = scalar_value_binding(subject);
            let comparisons = allowed
                .iter()
                .cloned()
                .map(|literal| AssertionPattern::Value {
                    comparator: ValueComparator::NotEqual,
                    left: ValueOperand::binding(value),
                    right: ValueOperand::literal(literal),
                });
            scalar_conditions_plan(subject, comparisons, context)?
        }
        SafetyCondition::NoOrphanAttributes { attribute } => {
            no_orphan_attributes_plan(attribute, context)?
        }
        SafetyCondition::Unresolvable { reason, unlock } => {
            return Err(failure(
                DiagnosticCategory::UnsupportedCapability,
                "safety_condition_unresolvable",
                "safety condition cannot be represented by the current assertion algebra",
            )
            .with_detail("reason", reason.as_str())
            .with_detail("unlock", unlock.as_str()));
        }
    };
    validate_migration_assertion_plan(&plan, context, limits)
}

/// Compatibility spelling for lowering verifier-derived safety conditions.
pub fn safety_condition_to_assertion_plan(
    condition: &RequiredSafetyCondition,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<ValidatedMigrationAssertionPlan, Diagnostic> {
    lower_condition_to_plan(condition, context, limits)
}

fn validate_condition_source(
    condition: &RequiredSafetyCondition,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<(), Diagnostic> {
    if condition.source_declared_identity()
        != context.resolved_schema().declared_identity_fingerprint()
        || condition.source_declared_identity() != context.managed_state().declared_identity()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "safety_condition_source_identity_mismatch",
            "safety condition source identity does not match validation state",
        ));
    }
    Ok(())
}

fn no_instances_plan(
    type_id: &TypeId,
    include_subtypes: bool,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    let instance = binding_id(0)?;
    plan(
        vec![binding(0, "instance")?],
        vec![AssertionPattern::Isa {
            binding: instance,
            include_subtypes,
            type_id: type_id.clone(),
        }],
        vec![instance],
        Vec::new(),
        context,
    )
}

fn owns_minimum_plan(
    owns: &type_bridge_contract::schema::OwnsFactId,
    minimum: u64,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    if minimum != 1 {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "safety_condition_invalid_owns_minimum",
            "the lexical-not lowerer supports only an owns minimum of one",
        ));
    }
    let owner = binding_id(0)?;
    let attribute = binding_id(1)?;
    plan(
        vec![binding(0, "owner")?, binding(1, "attribute")?],
        vec![
            AssertionPattern::Isa {
                binding: owner,
                include_subtypes: true,
                type_id: owns.owner().clone(),
            },
            AssertionPattern::Not {
                patterns: vec![AssertionPattern::Has {
                    attribute,
                    attribute_id: owns.attribute().clone(),
                    owner,
                }],
            },
        ],
        vec![owner],
        vec![attribute],
        context,
    )
}

fn owns_maximum_plan(
    owns: &type_bridge_contract::schema::OwnsFactId,
    maximum: u64,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    let violating_attributes = maximum
        .checked_add(1)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "safety_condition_binding_limit",
                "owns maximum cannot fit the canonical binding range",
            )
        })?;
    let binding_count = violating_attributes.checked_add(1).ok_or_else(|| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "safety_condition_binding_limit",
            "owns maximum cannot fit the canonical binding range",
        )
    })?;
    if !limits.allows_bindings(binding_count)
        || !StructuralLimits::CANONICAL.allows_bindings(binding_count)
    {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "safety_condition_binding_limit",
            "owns maximum assertion exceeds the binding ceiling",
        ));
    }

    let owner = binding_id(0)?;
    let mut bindings = vec![binding(0, "owner")?];
    let mut attributes = Vec::with_capacity(violating_attributes);
    let mut patterns = vec![AssertionPattern::Isa {
        binding: owner,
        include_subtypes: true,
        type_id: owns.owner().clone(),
    }];
    for index in 0..violating_attributes {
        let ordinal = index + 1;
        let attribute = binding_id(ordinal)?;
        bindings.push(binding(ordinal, &format!("attribute_{index}"))?);
        patterns.push(AssertionPattern::Has {
            attribute,
            attribute_id: owns.attribute().clone(),
            owner,
        });
        attributes.push(attribute);
    }
    for left in 0..attributes.len() {
        for right in (left + 1)..attributes.len() {
            patterns.push(AssertionPattern::Value {
                comparator: ValueComparator::NotEqual,
                left: ValueOperand::binding(attributes[left]),
                right: ValueOperand::binding(attributes[right]),
            });
        }
    }
    plan(bindings, patterns, vec![owner], attributes, context)
}

fn scalar_value_binding(subject: &ScalarSafetySubject) -> BindingId {
    match subject {
        ScalarSafetySubject::Value(_) => {
            BindingId::new(0).expect("the fixed scalar binding is canonical")
        }
        ScalarSafetySubject::Owns(_) => {
            BindingId::new(1).expect("the fixed ownership value binding is canonical")
        }
    }
}

fn scalar_condition_plan(
    subject: &ScalarSafetySubject,
    comparison: AssertionPattern,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    scalar_conditions_plan(subject, [comparison], context)
}

fn scalar_conditions_plan(
    subject: &ScalarSafetySubject,
    comparisons: impl IntoIterator<Item = AssertionPattern>,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    match subject {
        ScalarSafetySubject::Value(value) => {
            let attribute = binding_id(0)?;
            let mut patterns = vec![AssertionPattern::Isa {
                binding: attribute,
                include_subtypes: true,
                type_id: TypeId::new(TypeKind::Attribute, value.attribute().label().as_str())?,
            }];
            patterns.extend(comparisons);
            plan(
                vec![binding(0, "attribute")?],
                patterns,
                vec![attribute],
                Vec::new(),
                context,
            )
        }
        ScalarSafetySubject::Owns(owns) => {
            let owner = binding_id(0)?;
            let attribute = binding_id(1)?;
            let mut patterns = vec![
                AssertionPattern::Isa {
                    binding: owner,
                    include_subtypes: true,
                    type_id: owns.owner().clone(),
                },
                AssertionPattern::Has {
                    attribute,
                    attribute_id: owns.attribute().clone(),
                    owner,
                },
            ];
            patterns.extend(comparisons);
            plan(
                vec![binding(0, "owner")?, binding(1, "attribute")?],
                patterns,
                vec![owner],
                vec![attribute],
                context,
            )
        }
    }
}

fn no_orphan_attributes_plan(
    attribute_id: &type_bridge_contract::id::AttributeId,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    let attribute = binding_id(0)?;
    let owner = binding_id(1)?;
    plan(
        vec![binding(0, "attribute")?, binding(1, "owner")?],
        vec![
            AssertionPattern::Isa {
                binding: attribute,
                include_subtypes: true,
                type_id: TypeId::new(TypeKind::Attribute, attribute_id.label().as_str())?,
            },
            AssertionPattern::Not {
                patterns: vec![AssertionPattern::Has {
                    attribute,
                    attribute_id: attribute_id.clone(),
                    owner,
                }],
            },
        ],
        vec![attribute],
        vec![owner],
        context,
    )
}

fn plan(
    bindings: Vec<AssertionBinding>,
    patterns: Vec<AssertionPattern>,
    outputs: Vec<BindingId>,
    witnesses: Vec<BindingId>,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<MigrationAssertionPlan, Diagnostic> {
    MigrationAssertionPlan::new(
        bindings,
        patterns,
        outputs,
        witnesses,
        context.managed_state().managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
}

fn binding(index: usize, variable: &str) -> Result<AssertionBinding, Diagnostic> {
    Ok(AssertionBinding::new(
        binding_id(index)?,
        QueryVariable::new(variable)?,
    ))
}

fn binding_id(index: usize) -> Result<BindingId, Diagnostic> {
    let index = u16::try_from(index).map_err(|_| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "safety_condition_binding_limit",
            "condition binding ordinal exceeds the canonical range",
        )
    })?;
    BindingId::new(index)
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code)
            .expect("static safety-condition lowering diagnostic code is canonical"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::id::{AttributeId, TypeKind};
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::managed_scope::SemanticProfileBinding;
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, AssertionExpectation, AssertionPattern, MigrationAssertionPlan,
        QueryVariable,
    };
    use type_bridge_contract::schema::{
        AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
        DocumentId, OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, SchemaOperation,
        SourceSpan, SourcedSchemaFact, TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::schema_lowering::SchemaLoweringProfileBinding;
    use type_bridge_contract::value::{Cardinality, ValueTypeTag};
    use type_bridge_schema::{
        ManagedDeltaContext, SafetyDerivationProfile, derive_safety_conditions,
        managed_schema_state, resolve,
    };

    use super::*;

    fn test_safety_profile() -> SafetyDerivationProfile {
        SafetyDerivationProfile::new(
            SemanticProfileBinding::typedb_3_12_1().expect("semantic profile"),
            SchemaLoweringProfileBinding::from_canonical_profile_bytes(
                br#"{"id":"typedb-3.12.1-schema-lowering/v1","test_fixture":"safety-condition"}"#,
            )
            .expect("test lowering profile"),
        )
        .expect("test safety profile")
    }

    fn type_id(kind: TypeKind, label: &str) -> TypeId {
        TypeId::new(kind, label).expect("fixture type")
    }

    fn declared(facts: Vec<SchemaFact>) -> DeclaredSchema {
        let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
            let offset = u64::try_from(index).expect("offset");
            let line = u32::try_from(index + 1).expect("line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("condition-lowering-test").expect("document"),
                    offset,
                    offset + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("span"),
            )
        });
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared fixture")
    }

    fn annotation(
        subject: AnnotationSubjectId,
        kind: AnnotationKindId,
        value: SchemaAnnotationValue,
    ) -> SchemaFact {
        SchemaFact::Annotation(
            AnnotationFact::new(AnnotationFactId::new(subject, kind), value).expect("annotation"),
        )
    }

    fn validation_fixture(
        source: &DeclaredSchema,
    ) -> (
        type_bridge_schema::ResolvedSchema,
        type_bridge_contract::schema_delta::ManagedSchemaState,
    ) {
        let profile = type_bridge_contract::fingerprint::SemanticProfileId::new("typedb-3.12.1/v1")
            .expect("profile");
        let resolved = resolve(source, &profile).expect("resolved source");
        let managed = managed_schema_state(
            source,
            &ManagedDeltaContext::new(
                ManagedScopeId::new("condition-lowering-test").expect("scope"),
                profile,
                CapabilitySet::new(),
            ),
        )
        .expect("managed source");
        (resolved, managed)
    }

    fn owns_base() -> (Vec<SchemaFact>, OwnsFactId) {
        let person = type_id(TypeKind::Entity, "person");
        let age = AttributeId::new("age").expect("attribute");
        let owns = OwnsFactId::new(person.clone(), age.clone()).expect("owns");
        (
            vec![
                SchemaFact::Type(TypeFact::new(person).expect("type")),
                SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type")),
                SchemaFact::Value(ValueFact::new(ValueFactId::new(age), ValueTypeTag::Long)),
                SchemaFact::Owns(OwnsFact::new(owns.clone())),
            ],
            owns,
        )
    }

    #[test]
    fn abstract_plan_has_exact_golden_shape_and_deterministic_fingerprint() {
        let person = type_id(TypeKind::Entity, "person");
        let type_fact = SchemaFact::Type(TypeFact::new(person.clone()).expect("type"));
        let abstract_fact = annotation(
            AnnotationSubjectId::Type(person.clone()),
            AnnotationKindId::Abstract,
            SchemaAnnotationValue::Presence,
        );
        let source = declared(vec![type_fact.clone()]);
        let target = declared(vec![type_fact, abstract_fact.clone()]);
        let derived = derive_safety_conditions(
            0,
            &SchemaOperation::define(vec![abstract_fact]).expect("operation"),
            &source,
            &target,
            &test_safety_profile(),
        )
        .expect("derived condition");
        let (resolved, managed) = validation_fixture(&source);
        let context = MigrationAssertionValidationContext::new(&resolved, &managed);
        let first = lower_condition_to_plan(
            &derived.conditions()[0],
            &context,
            StructuralLimits::CANONICAL,
        )
        .expect("lowered condition");
        let second = lower_condition_to_plan(
            &derived.conditions()[0],
            &context,
            StructuralLimits::CANONICAL,
        )
        .expect("lowered condition");

        let instance = BindingId::new(0).expect("binding");
        let expected = MigrationAssertionPlan::new(
            vec![AssertionBinding::new(
                instance,
                QueryVariable::new("instance").expect("variable"),
            )],
            vec![AssertionPattern::Isa {
                binding: instance,
                include_subtypes: false,
                type_id: person,
            }],
            vec![instance],
            Vec::new(),
            managed.managed_semantic_schema().clone(),
            AssertionExpectation::NoRows,
        )
        .expect("golden plan");
        assert_eq!(
            first.plan().canonical_bytes().expect("plan bytes"),
            expected.canonical_bytes().expect("golden bytes")
        );
        assert_eq!(
            first.plan().canonical_bytes().expect("plan bytes"),
            second.plan().canonical_bytes().expect("plan bytes")
        );
        assert_eq!(
            first.plan().fingerprint().expect("fingerprint"),
            second.plan().fingerprint().expect("fingerprint")
        );
    }

    #[test]
    fn owns_minimum_maximum_validate_and_respect_caller_limits() {
        let (base, owns) = owns_base();
        let subject = AnnotationSubjectId::Owns(owns);
        let old = annotation(
            subject.clone(),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(2)).expect("old card")),
        );
        let new = annotation(
            subject,
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(Cardinality::new(1, Some(1)).expect("new card")),
        );
        let source = declared(base.iter().cloned().chain([old.clone()]).collect());
        let target = declared(base.into_iter().chain([new.clone()]).collect());
        let derived = derive_safety_conditions(
            0,
            &SchemaOperation::redefine(old, new).expect("operation"),
            &source,
            &target,
            &test_safety_profile(),
        )
        .expect("conditions");
        let (resolved, managed) = validation_fixture(&source);
        let context = MigrationAssertionValidationContext::new(&resolved, &managed);
        let minimum = lower_condition_to_plan(
            &derived.conditions()[0],
            &context,
            StructuralLimits::CANONICAL,
        )
        .expect("minimum validates");
        assert!(matches!(
            minimum.plan().patterns()[1],
            AssertionPattern::Not { .. }
        ));
        let maximum = lower_condition_to_plan(
            &derived.conditions()[1],
            &context,
            StructuralLimits::CANONICAL,
        )
        .expect("maximum validates");
        assert_eq!(maximum.plan().bindings().len(), 3);
        let tight = StructuralLimits {
            bindings: 2,
            ..StructuralLimits::CANONICAL
        };
        assert_eq!(
            lower_condition_to_plan(&derived.conditions()[1], &context, tight)
                .expect_err("caller limit")
                .code()
                .as_str(),
            "safety_condition_binding_limit"
        );
    }

    #[test]
    fn unresolvable_and_source_tamper_fail_closed() {
        let (base, owns) = owns_base();
        let key = annotation(
            AnnotationSubjectId::Owns(owns),
            AnnotationKindId::Key,
            SchemaAnnotationValue::Presence,
        );
        let source = declared(base.clone());
        let target = declared(base.into_iter().chain([key.clone()]).collect());
        let derived = derive_safety_conditions(
            0,
            &SchemaOperation::define(vec![key]).expect("operation"),
            &source,
            &target,
            &test_safety_profile(),
        )
        .expect("condition");
        let (resolved, managed) = validation_fixture(&source);
        let context = MigrationAssertionValidationContext::new(&resolved, &managed);
        assert_eq!(
            lower_condition_to_plan(
                &derived.conditions()[0],
                &context,
                StructuralLimits::CANONICAL,
            )
            .expect_err("gated condition")
            .code()
            .as_str(),
            "safety_condition_unresolvable"
        );

        let other = declared(vec![SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Entity, "other")).expect("type"),
        )]);
        let (other_resolved, other_managed) = validation_fixture(&other);
        let other_context =
            MigrationAssertionValidationContext::new(&other_resolved, &other_managed);
        assert_eq!(
            lower_condition_to_plan(
                &derived.conditions()[0],
                &other_context,
                StructuralLimits::CANONICAL,
            )
            .expect_err("source tamper")
            .code()
            .as_str(),
            "safety_condition_source_identity_mismatch"
        );
    }
}
