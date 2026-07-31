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
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("rust-emitter.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &RustEmitter::new().generator_handlers(),
        resources,
    )
    .unwrap()
}

#[test]
fn emits_exact_deterministic_single_dependency_crate() {
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(include_str!("acceptance/schema.yaml"), &resources);
    let first = emitter.emit(&projection).unwrap();
    let second = emitter.emit(&projection).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first
            .files()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
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
    let declarations =
        String::from_utf8(first.get("src/declaration.rs").unwrap().to_vec()).unwrap();
    for import in [
        "HydratedRow",
        "HydrationCapability",
        "ValidationError",
        "MaterializeModel",
    ] {
        assert!(declarations.contains(import));
    }
    assert!(declarations.contains("fn __tb_dispatch_subtype"));
    assert!(!declarations.contains("fn dispatch_subtype"));
    let person_start = declarations
        .find("impl SubtypeRootModel for Person")
        .unwrap();
    let person_end = person_start + declarations[person_start..].find(" } }\n").unwrap() + 4;
    let person_impl = &declarations[person_start..person_end];
    let identity = person_impl
        .find("__tb_row.type_id_json() == Person::TYPE_ID_JSON")
        .unwrap();
    let materialize = person_impl.find("Person::materialize").unwrap();
    assert!(identity < materialize);
    assert!(
        person_impl.contains("ValidationError::new(\"type_id\", \"wrong_concrete_model_type\")")
    );
    let membership_start = declarations
        .find("impl SubtypeRootModel for Membership")
        .unwrap();
    let membership_end =
        membership_start + declarations[membership_start..].find(" } }\n").unwrap() + 4;
    let membership_impl = &declarations[membership_start..membership_end];
    assert!(membership_impl.contains("Membership::TYPE_ID_JSON => Membership::materialize(__tb_row, __tb_cap).map(MembershipFamily::Membership)"));
    assert!(membership_impl.contains("Employment::TYPE_ID_JSON => Employment::materialize(__tb_row, __tb_cap).map(MembershipFamily::Employment)"));
    assert!(
        membership_impl
            .contains("_ => Err(ValidationError::new(\"type_id\", \"wrong_concrete_model_type\"))")
    );
    assert!(
        declarations.find("impl Model for Membership").unwrap()
            < declarations.find("impl Model for Employment").unwrap()
    );
    assert!(declarations.contains("RoleUpcast<EmploymentEmployeePlayer, MembershipMemberPlayer>"));
    assert!(declarations.contains(
        "impl RoleTokenCompatible<Employment, EmploymentEmployeePlayer> for Employment {}"
    ));
    assert!(!declarations.contains(
        "impl RoleTokenCompatible<Membership, MembershipMemberPlayer> for Employment {}"
    ));
    let create = String::from_utf8(first.get("src/create.rs").unwrap().to_vec()).unwrap();
    assert!(create.contains("pub struct ContainerCreate"));
    assert!(create.contains("pub fn try_new"));
    let read = String::from_utf8(first.get("src/read.rs").unwrap().to_vec()).unwrap();
    assert!(read.contains("impl QueryValued for Identifier { type Domain = String; }"));
    let tokens = String::from_utf8(first.get("src/tokens.rs").unwrap().to_vec()).unwrap();
    assert!(tokens.contains("pub struct ContainerType"));
    assert!(tokens.contains("pub enum ContainerItemPlayer"));
    assert!(tokens.contains("impl RolePlayer<Person> for EmploymentEmployeePlayer {}"));
    assert!(tokens.contains("pub const plays_event_container_item"));
    let manifest = String::from_utf8(first.get("Cargo.toml").unwrap().to_vec()).unwrap();
    assert!(manifest.contains("[dependencies]"));
    assert!(manifest.contains("type-bridge = { version = \"=2.0.0\", default-features = false }"));
    assert!(manifest.contains("doctest = false"));
    assert!(manifest.contains("rust-version = \"1.88\""));
}

#[test]
fn emits_safely_line_prefixed_type_and_direct_sub_documentation() {
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        r#"format: typebridge.schema/v2
entities:
  actor: {}
  person:
    doc: |-
      Type "doc".
      closing */ kept
      ```rust
      compile_error!("schema documentation must never execute");
      ```
          compile_error!("indented schema documentation must never execute");
    sub:
      type: actor
      doc: |-
        Edge 'doc' \ path
        closes */ safely
"#,
        &resources,
    );
    let package = emitter.emit(&projection).unwrap();
    let read = std::str::from_utf8(package.get("src/read.rs").unwrap()).unwrap();
    let reference = std::str::from_utf8(package.get("src/reference.rs").unwrap()).unwrap();
    let manifest = std::str::from_utf8(package.get("Cargo.toml").unwrap()).unwrap();
    let documentation = "/// Type \"doc\".\n/// closing */ kept\n/// ```rust\n/// compile_error!(\"schema documentation must never execute\");\n/// ```\n///     compile_error!(\"indented schema documentation must never execute\");\n/// \n/// Direct subtype of `actor`:\n/// Edge 'doc' \\ path\n/// closes */ safely\n";

    assert!(manifest.contains("doctest = false"));
    assert!(read.contains(&format!(
        "{documentation}#[derive(Clone, Debug, PartialEq)]\npub struct Person"
    )));
    assert!(reference.contains(&format!(
        "{documentation}#[derive(Clone, Debug, PartialEq)]\npub struct PersonRef"
    )));
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

#[test]
fn rejects_schema_name_colliding_with_injected_schema_export() {
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        "format: typebridge.schema/v2\nentities:\n  app_schema: {}\n",
        &resources,
    );
    let error = emitter.emit(&projection).unwrap_err();
    assert!(error.to_string().contains("rust_emitter_name_collision"));
    assert!(error.to_string().contains("AppSchema"));
}
