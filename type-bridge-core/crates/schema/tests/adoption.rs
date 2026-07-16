use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, ManagedScopeId,
    OwnsFact, OwnsFactId, SchemaAnnotationValue, SchemaFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::semantic_profile::SemanticProfile;
use type_bridge_contract::value::{Cardinality, ValueTypeTag};
use type_bridge_schema::{
    CanonicalObservedSchema, ObservedFactProvenance, ObservedFactScope, ObservedSchema,
    ObservedSchemaFact, adopt_observed_schema, canonicalize_observed_schema,
};

fn profile_id(value: &str) -> SemanticProfileId {
    SemanticProfileId::new(value).expect("test profile identifier")
}

fn profile(value: &str) -> SemanticProfile {
    SemanticProfile::resolve(&profile_id(value)).expect("supported test profile")
}

fn type_fact(kind: TypeKind, label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(kind, label).expect("test type")).expect("test type fact"),
    )
}

fn value_fact(label: &str) -> SchemaFact {
    SchemaFact::Value(ValueFact::new(
        ValueFactId::new(AttributeId::new(label).expect("test attribute")),
        ValueTypeTag::String,
    ))
}

fn owns_id(owner: &str, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(
        TypeId::new(TypeKind::Entity, owner).expect("test owner"),
        AttributeId::new(attribute).expect("test attribute"),
    )
    .expect("test owns")
}

fn captured(
    fact: SchemaFact,
    provenance: ObservedFactProvenance,
    scope: ObservedFactScope,
) -> ObservedSchemaFact {
    ObservedSchemaFact::new(fact, provenance, scope)
}

fn managed(fact: SchemaFact, provenance: ObservedFactProvenance) -> ObservedSchemaFact {
    captured(fact, provenance, ObservedFactScope::Managed)
}

fn canonical(facts: Vec<ObservedSchemaFact>) -> CanonicalObservedSchema {
    canonical_with_capabilities(facts, CapabilitySet::new())
}

fn canonical_with_capabilities(
    facts: Vec<ObservedSchemaFact>,
    capabilities: CapabilitySet,
) -> CanonicalObservedSchema {
    let observed = ObservedSchema::new(FormatVersion::V1, capabilities, facts);
    canonicalize_observed_schema(&observed, &profile("typedb-3.12.1/v1"))
        .expect("test observation canonicalizes")
}

fn adopt(canonical: &CanonicalObservedSchema) -> type_bridge_schema::AdoptionBaseline {
    adopt_observed_schema(
        canonical,
        ManagedScopeId::new("adoption-test").expect("test scope"),
        &profile_id("typedb-3.12.1/v1"),
        &CapabilitySet::new(),
    )
    .expect("test observation adopts")
}

#[test]
fn direct_observation_becomes_an_exact_operation_free_baseline() {
    let person = type_fact(TypeKind::Entity, "person");
    let person_id = person.id();
    let canonical = canonical(vec![managed(person, ObservedFactProvenance::Direct)]);
    let baseline = adopt(&canonical);

    assert!(baseline.declared_schema().fact(&person_id).is_some());
    assert_eq!(baseline.declared_schema().facts().len(), 1);
    assert_eq!(baseline.managed_state().selection().len(), 1);
    assert!(baseline.operations().is_empty());
    assert_eq!(
        baseline.managed_declared_identity(),
        baseline.managed_state().managed_declared_identity()
    );
    assert_eq!(
        baseline.managed_semantic_schema(),
        baseline.managed_state().managed_semantic_schema()
    );
    let _ = baseline.resolved_schema();
}

#[test]
fn inherited_projection_is_not_adopted_as_a_direct_fact() {
    let person = type_fact(TypeKind::Entity, "person");
    let person_id = person.id();
    let employee = type_fact(TypeKind::Entity, "employee");
    let employee_id = employee.id();
    let canonical = canonical(vec![
        managed(person, ObservedFactProvenance::Direct),
        managed(
            employee,
            ObservedFactProvenance::Inherited {
                declared_fact: person_id,
            },
        ),
    ]);
    let baseline = adopt(&canonical);

    assert!(baseline.declared_schema().fact(&employee_id).is_none());
    assert_eq!(baseline.declared_schema().facts().len(), 1);
}

#[test]
fn server_default_is_filtered_before_adoption_without_changing_state() {
    let owns = owns_id("person", "name");
    let direct = vec![
        managed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::Direct,
        ),
        managed(
            type_fact(TypeKind::Attribute, "name"),
            ObservedFactProvenance::Direct,
        ),
        managed(value_fact("name"), ObservedFactProvenance::Direct),
        managed(
            SchemaFact::Owns(OwnsFact::new(owns.clone())),
            ObservedFactProvenance::Direct,
        ),
    ];
    let default = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(owns),
                AnnotationKindId::Card,
            ),
            SchemaAnnotationValue::Cardinality(
                Cardinality::new(0, Some(1)).expect("profile default"),
            ),
        )
        .expect("default annotation"),
    );
    let without_default = canonical(direct.clone());
    let mut with_default_capture = direct;
    with_default_capture.push(managed(default, ObservedFactProvenance::ServerDefault));
    let with_default = canonical(with_default_capture);

    assert_eq!(
        adopt(&without_default).managed_state(),
        adopt(&with_default).managed_state()
    );
}

#[test]
fn internal_direct_facts_are_evidence_but_not_adopted_schema() {
    let person = type_fact(TypeKind::Entity, "person");
    let internal = type_fact(TypeKind::Entity, "typebridge-migration-state");
    let internal_id = internal.id();
    let canonical = canonical(vec![
        managed(person, ObservedFactProvenance::Direct),
        captured(
            internal,
            ObservedFactProvenance::Direct,
            ObservedFactScope::TypeBridgeInternal,
        ),
    ]);
    assert_eq!(canonical.direct_schema().facts().len(), 2);

    let baseline = adopt(&canonical);
    assert!(baseline.declared_schema().fact(&internal_id).is_none());
    assert_eq!(baseline.declared_schema().facts().len(), 1);
    assert_eq!(baseline.managed_state().selection().len(), 1);
}

#[test]
fn exclusive_scope_is_complete_and_bound_to_requested_identity() {
    let canonical = canonical(vec![
        managed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::Direct,
        ),
        managed(
            type_fact(TypeKind::Entity, "company"),
            ObservedFactProvenance::Direct,
        ),
    ]);
    let requested = ManagedScopeId::new("exclusive-adoption").expect("test scope");
    let baseline = adopt_observed_schema(
        &canonical,
        requested.clone(),
        &profile_id("typedb-3.12.1/v1"),
        &CapabilitySet::new(),
    )
    .expect("exclusive adoption");

    assert_eq!(baseline.bound_scope().binding().id(), &requested);
    assert_eq!(
        baseline.bound_scope().selection().len(),
        baseline.declared_schema().facts().len()
    );
    assert_eq!(
        baseline.bound_scope().selection().iter().collect::<Vec<_>>(),
        baseline.managed_state().selection().iter().collect::<Vec<_>>()
    );
}

#[test]
fn canonicalization_profile_mismatch_is_rejected() {
    let canonical = canonical(vec![managed(
        type_fact(TypeKind::Entity, "person"),
        ObservedFactProvenance::Direct,
    )]);
    let error = adopt_observed_schema(
        &canonical,
        ManagedScopeId::new("adoption-test").expect("test scope"),
        &profile_id("typedb-3.11.5/v1"),
        &CapabilitySet::new(),
    )
    .expect_err("profile mismatch");
    assert_eq!(
        error
            .iter()
            .next()
            .expect("diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "adoption_semantic_profile_mismatch"
    );
}

#[test]
fn missing_available_capability_is_rejected() {
    let required = CapabilityId::new("schema.annotations").expect("test capability");
    let canonical = canonical_with_capabilities(
        vec![managed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::Direct,
        )],
        [required.clone()].into_iter().collect(),
    );
    assert!(
        adopt_observed_schema(
            &canonical,
            ManagedScopeId::new("adoption-test").expect("test scope"),
            &profile_id("typedb-3.12.1/v1"),
            &CapabilitySet::new(),
        )
        .is_err()
    );

    let available = [required].into_iter().collect();
    assert!(
        adopt_observed_schema(
            &canonical,
            ManagedScopeId::new("adoption-test").expect("test scope"),
            &profile_id("typedb-3.12.1/v1"),
            &available,
        )
        .is_ok()
    );
}

#[test]
fn ambiguous_provenance_is_rejected_before_adoption() {
    let observed = ObservedSchema::new(
        FormatVersion::V1,
        CapabilitySet::new(),
        [managed(
            type_fact(TypeKind::Entity, "person"),
            ObservedFactProvenance::Ambiguous,
        )],
    );
    let error = canonicalize_observed_schema(&observed, &profile("typedb-3.12.1/v1"))
        .expect_err("ambiguous observation");
    assert_eq!(
        error
            .iter()
            .next()
            .expect("diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "ambiguous_observed_provenance"
    );
}
