use std::collections::BTreeSet;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, CodeResourceDigest, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

fn projected(
    source: &str,
    resources: &[CodeResourceDigest],
) -> type_bridge_contract::projection::RuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-emitter.yaml").unwrap(),
        source,
    )]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &RustEmitter::new().generator_handlers(),
        resources,
    ).unwrap()
}

#[test]
fn emits_exact_deterministic_dependency_free_crate() {
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(include_str!("acceptance/schema.yaml"), &resources);
    let first = emitter.emit(&projection).unwrap();
    let second = emitter.emit(&projection).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first.files().keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "Cargo.toml",
            "src/create.rs",
            "src/declaration.rs",
            "src/functions.rs",
            "src/lib.rs",
            "src/read.rs",
            "src/reference.rs",
            "src/runtime.rs",
            "src/schema.rs",
            "src/structs.rs",
            "src/tokens.rs",
        ]),
    );
    let declarations = String::from_utf8(first.get("src/declaration.rs").unwrap().to_vec()).unwrap();
    assert!(declarations.find("impl Model for Membership").unwrap()
        < declarations.find("impl Model for Employment").unwrap());
    assert!(declarations.contains("RoleUpcast<EmploymentEmployeePlayer, MembershipMemberPlayer>"));
    let create = String::from_utf8(first.get("src/create.rs").unwrap().to_vec()).unwrap();
    assert!(create.contains("pub struct ContainerCreate"));
    assert!(create.contains("pub fn try_new"));
    let tokens = String::from_utf8(first.get("src/tokens.rs").unwrap().to_vec()).unwrap();
    assert!(tokens.contains("pub struct ContainerType"));
    assert!(tokens.contains("pub enum ContainerItemPlayer"));
    assert!(tokens.contains("pub const plays_event_container_item"));
    let manifest = String::from_utf8(first.get("Cargo.toml").unwrap().to_vec()).unwrap();
    assert!(!manifest.contains("dependencies"));
}

#[test]
fn rejects_projection_without_exact_resource_evidence() {
    let projection = projected(include_str!("acceptance/schema.yaml"), &[]);
    let error = RustEmitter::new().emit(&projection).unwrap_err();
    assert!(error.to_string().contains("rust_emitter_evidence_mismatch"));
}

#[test]
fn rejects_schema_name_colliding_with_runtime_export() {
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        "format: typebridge.schema/v2\nentities:\n  cardinality: {}\n",
        &resources,
    );
    let error = emitter.emit(&projection).unwrap_err();
    assert!(error.to_string().contains("rust_emitter_name_collision"));
    assert!(error.to_string().contains("Cardinality"));
}
