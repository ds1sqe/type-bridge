use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CompleteReadProjection, CreateProjection, DeclarationProjection, EmissionPlan,
    ModelProjection, ProjectedMultiplicity, ProjectionConfig, ProjectionHandler,
    QueryTokenProjection, ReadRoleProjection, ReferenceReadProjection, RuntimeProjection,
    TargetIdentifier,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;
use type_bridge_contract::value::Cardinality;

fn fixture() -> RuntimeProjection {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let model = ModelProjection::new(
        person.clone(),
        TargetIdentifier::python("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        CreateProjection::new(true, vec![], BTreeMap::new()).unwrap(),
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(Some(TargetIdentifier::python("PersonRef").unwrap()), vec![])
            .unwrap(),
        QueryTokenProjection::new(person.clone(), BTreeMap::new(), BTreeMap::new()).unwrap(),
    )
    .unwrap();
    RuntimeProjection::try_new(
        BindingTarget::Python,
        ProjectionConfig::python(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"schema",
        )
        .unwrap(),
        &[ProjectionHandler::python_v1()],
        &[],
        BTreeMap::from([(person.clone(), model)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![person.clone()],
            vec![BTreeSet::from([person])],
            vec![],
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

fn rust_fixture(
    explicit_names: bool,
) -> Result<RuntimeProjection, type_bridge_contract::diagnostic::Diagnostic> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let mut create = CreateProjection::new(true, vec![], BTreeMap::new()).unwrap();
    let mut query =
        QueryTokenProjection::new(person.clone(), BTreeMap::new(), BTreeMap::new()).unwrap();
    if explicit_names {
        create = create.with_target_name(TargetIdentifier::rust("PersonCreate").unwrap());
        query = query.with_target_name(TargetIdentifier::rust("PersonType").unwrap());
    }
    let model = ModelProjection::new(
        person.clone(),
        TargetIdentifier::rust("Person").unwrap(),
        DeclarationProjection::new(
            None,
            None,
            false,
            true,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap(),
        create,
        CompleteReadProjection::new(vec![], BTreeMap::new(), vec![]).unwrap(),
        ReferenceReadProjection::new(Some(TargetIdentifier::rust("PersonRef").unwrap()), vec![])
            .unwrap(),
        query,
    )
    .unwrap();
    RuntimeProjection::try_new(
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        SemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            b"rust-schema",
        )
        .unwrap(),
        &[ProjectionHandler::rust_v1()],
        &[],
        BTreeMap::from([(person.clone(), model)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            vec![person.clone()],
            vec![BTreeSet::from([person])],
            vec![],
            vec![],
        )
        .unwrap(),
    )
}

#[test]
fn canonical_projection_rebuilds_and_verifies_detached_fingerprints() {
    let runtime = fixture();
    let decoded = decode_runtime_projection_verified(
        &to_canonical_json(&runtime).unwrap(),
        &to_canonical_json(runtime.semantic_fingerprint()).unwrap(),
        &to_canonical_json(runtime.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, runtime);
}

#[test]
fn rust_projection_rebuilds_with_explicit_surface_names() {
    let runtime = rust_fixture(true).unwrap();
    let decoded = decode_runtime_projection_verified(
        &to_canonical_json(&runtime).unwrap(),
        &to_canonical_json(runtime.semantic_fingerprint()).unwrap(),
        &to_canonical_json(runtime.projection_fingerprint()).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, runtime);
}

#[test]
fn rust_projection_rejects_missing_required_surface_names() {
    assert_eq!(
        rust_fixture(false).unwrap_err().code().as_str(),
        "missing_rust_projection_identifier",
    );
}

#[test]
fn noncanonical_and_tampered_projection_bytes_fail_closed() {
    let runtime = fixture();
    let bytes = to_canonical_json(&runtime).unwrap();
    let semantic = to_canonical_json(runtime.semantic_fingerprint()).unwrap();
    let binding = to_canonical_json(runtime.projection_fingerprint()).unwrap();

    let mut noncanonical = vec![b' '];
    noncanonical.extend_from_slice(&bytes);
    assert_eq!(
        decode_runtime_projection_verified(&noncanonical, &semantic, &binding)
            .unwrap_err()
            .code()
            .as_str(),
        "non_canonical_json",
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["models"][0]["target_name"] = serde_json::Value::String("ChangedPerson".into());
    let tampered = to_canonical_json(&value).unwrap();
    assert_eq!(
        decode_runtime_projection_verified(&tampered, &semantic, &binding)
            .unwrap_err()
            .code()
            .as_str(),
        "runtime_projection_fingerprint_mismatch",
    );
}

#[test]
fn role_upcast_wire_carries_the_active_role_identity() {
    let child = RoleId::new("employment", "employee").unwrap();
    let parent = RoleId::new("membership", "member").unwrap();
    let role = ReadRoleProjection::new(
        child.clone(),
        BTreeSet::new(),
        ProjectedMultiplicity::from_cardinality(Cardinality::new(1, Some(1)).unwrap()),
    )
    .unwrap();
    let read = CompleteReadProjection::new(vec![], BTreeMap::from([(child.clone(), role)]), vec![])
        .unwrap()
        .with_role_upcasts(BTreeMap::from([(child.clone(), vec![parent])]))
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&to_canonical_json(&read).unwrap()).unwrap();
    assert_eq!(
        value["role_upcasts"][0]["role"],
        serde_json::to_value(child).unwrap()
    );
}
