use std::collections::BTreeSet;
use std::str;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{FunctionId, RoleId, StructId, TypeId};
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId, PlaysFactId};
use type_bridge_schema::{
    SchemaDocumentSet, VerifiedSchemaAuthority, encode_schema_authority, normalize_documents,
    project, resolve,
};
use type_bridge_schema_codegen::{GeneratedPackage, PythonEmitter, RustEmitter, TypeScriptEmitter};

mod support;

#[derive(Debug, Eq, PartialEq)]
struct CanonicalProjectionIds {
    models: BTreeSet<TypeId>,
    owns: BTreeSet<OwnsFactId>,
    roles: BTreeSet<(TypeId, RoleId)>,
    structs: BTreeSet<StructId>,
    functions: BTreeSet<FunctionId>,
    plays: BTreeSet<PlaysFactId>,
}

fn canonical_ids(projection: &RuntimeProjection) -> CanonicalProjectionIds {
    CanonicalProjectionIds {
        models: projection.models().keys().cloned().collect(),
        owns: projection
            .models()
            .values()
            .flat_map(|model| model.query_tokens().fields().keys().cloned())
            .collect(),
        roles: projection
            .models()
            .iter()
            .flat_map(|(owner, model)| {
                model
                    .query_tokens()
                    .roles()
                    .keys()
                    .map(|role| (owner.clone(), role.clone()))
            })
            .collect(),
        structs: projection.structs().keys().cloned().collect(),
        functions: projection.functions().keys().cloned().collect(),
        plays: projection.playing_facts().keys().cloned().collect(),
    }
}

fn missing_resource(resources: &[CodeResourceDigest]) -> Vec<CodeResourceDigest> {
    assert!(
        !resources.is_empty(),
        "emitter must declare fixed resource evidence"
    );
    resources.iter().skip(1).cloned().collect()
}

fn mutated_resource(resources: &[CodeResourceDigest]) -> Vec<CodeResourceDigest> {
    assert!(
        !resources.is_empty(),
        "emitter must declare fixed resource evidence"
    );
    let mut mutated = resources.to_vec();
    let id = mutated[0].id().as_str().to_owned();
    mutated[0] = CodeResourceDigest::from_bytes(id, b"mutated cross-binding evidence").unwrap();
    mutated.sort_by(|left, right| left.id().cmp(right.id()));
    mutated
}

fn assert_embeds_fingerprints(
    package: &GeneratedPackage,
    schema_path: &str,
    projection: &RuntimeProjection,
) {
    let source = str::from_utf8(
        package
            .get(schema_path)
            .expect("schema evidence file is emitted"),
    )
    .expect("generated schema evidence is UTF-8");
    let semantic_digest = projection
        .semantic_fingerprint()
        .as_fingerprint()
        .digest()
        .to_hex();
    let projection_digest = projection
        .projection_fingerprint()
        .as_fingerprint()
        .digest()
        .to_hex();

    assert!(source.contains("SEMANTIC_SCHEMA_FINGERPRINT_JSON"));
    assert!(source.contains("PROJECTION_FINGERPRINT_JSON"));
    assert!(source.contains("RUNTIME_PROJECTION_JSON"));
    assert!(source.contains(&semantic_digest));
    assert!(source.contains(&projection_digest));
}

fn assert_embeds_authority(
    package: &GeneratedPackage,
    authority_path: &str,
    authority: &VerifiedSchemaAuthority,
) {
    let source = str::from_utf8(
        package
            .get(authority_path)
            .expect("private authority source is emitted"),
    )
    .expect("private authority source is UTF-8");
    let envelope = String::from_utf8(encode_schema_authority(authority)).unwrap();
    let escaped = String::from_utf8(to_canonical_json(&envelope).unwrap()).unwrap();
    assert!(
        source.contains(&escaped),
        "{authority_path} did not embed exact authority bytes"
    );
}

#[test]
fn shared_schema_preserves_canonical_ids_and_target_specific_evidence() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("cross-binding.yaml").unwrap(),
        include_str!("acceptance/schema.yaml"),
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let authority = support::authority(include_str!("acceptance/schema.yaml"));

    let python_emitter = PythonEmitter::new();
    let python_handlers = python_emitter.generator_handlers();
    let python_resources = python_emitter.code_resources().unwrap();
    let python = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &python_handlers,
        &python_resources,
    )
    .unwrap();

    let typescript_emitter = TypeScriptEmitter::new();
    let typescript_handlers = typescript_emitter.generator_handlers();
    let typescript_resources = typescript_emitter.code_resources().unwrap();
    let typescript = project(
        &resolved,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &typescript_handlers,
        &typescript_resources,
    )
    .unwrap();

    let rust_emitter = RustEmitter::new();
    let rust_handlers = rust_emitter.generator_handlers();
    let rust_resources = rust_emitter.code_resources().unwrap();
    let rust = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &rust_handlers,
        &rust_resources,
    )
    .unwrap();

    let python_ids = canonical_ids(&python);
    assert_eq!(python_ids, canonical_ids(&typescript));
    assert_eq!(python_ids, canonical_ids(&rust));
    assert_eq!(
        python.semantic_fingerprint(),
        typescript.semantic_fingerprint()
    );
    assert_eq!(python.semantic_fingerprint(), rust.semantic_fingerprint());
    assert_ne!(
        python.projection_fingerprint(),
        typescript.projection_fingerprint()
    );
    assert_ne!(
        python.projection_fingerprint(),
        rust.projection_fingerprint()
    );
    assert_ne!(
        typescript.projection_fingerprint(),
        rust.projection_fingerprint()
    );

    let python_package = python_emitter.emit(&python, &authority).unwrap();
    let typescript_package = typescript_emitter.emit(&typescript, &authority).unwrap();
    let rust_package = rust_emitter.emit(&rust, &authority).unwrap();
    assert_embeds_fingerprints(&python_package, "_schema.py", &python);
    assert_embeds_fingerprints(&typescript_package, "src/schema.ts", &typescript);
    assert_embeds_fingerprints(&rust_package, "src/schema.rs", &rust);
    assert_embeds_authority(&python_package, "_authority.py", &authority);
    assert_embeds_authority(&typescript_package, "src/authority.ts", &authority);
    assert_embeds_authority(&rust_package, "src/schema.rs", &authority);

    let foreign_authority =
        support::authority("format: typebridge.schema/v2\nentities:\n  foreign-workspace: {}\n");
    for error in [
        python_emitter
            .emit(&python, &foreign_authority)
            .unwrap_err(),
        typescript_emitter
            .emit(&typescript, &foreign_authority)
            .unwrap_err(),
        rust_emitter.emit(&rust, &foreign_authority).unwrap_err(),
    ] {
        assert_eq!(error.code().as_str(), "schema_codegen_authority_mismatch");
    }

    for resources in [
        missing_resource(&python_resources),
        mutated_resource(&python_resources),
    ] {
        let invalid = project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &python_handlers,
            &resources,
        )
        .unwrap();
        assert_eq!(
            python_emitter
                .emit(&invalid, &authority)
                .unwrap_err()
                .code()
                .as_str(),
            "python_emitter_evidence_mismatch",
        );
    }

    for resources in [
        missing_resource(&typescript_resources),
        mutated_resource(&typescript_resources),
    ] {
        let invalid = project(
            &resolved,
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &typescript_handlers,
            &resources,
        )
        .unwrap();
        assert_eq!(
            typescript_emitter
                .emit(&invalid, &authority)
                .unwrap_err()
                .code()
                .as_str(),
            "typescript_emitter_evidence_mismatch",
        );
    }

    for resources in [
        missing_resource(&rust_resources),
        mutated_resource(&rust_resources),
    ] {
        let invalid = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &rust_handlers,
            &resources,
        )
        .unwrap();
        assert_eq!(
            rust_emitter
                .emit(&invalid, &authority)
                .unwrap_err()
                .code()
                .as_str(),
            "rust_emitter_evidence_mismatch",
        );
    }
}

#[test]
fn exact_split_yaml_docs_fixture_emits_every_binding_target() {
    let source = include_str!("../../../../docs/fixtures/split-yaml-v1/schema/fixture.yaml");
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("fixture.yaml").unwrap(), source)]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let authority = support::authority(source);

    let python_emitter = PythonEmitter::new();
    let python = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &python_emitter.generator_handlers(),
        &python_emitter.code_resources().unwrap(),
    )
    .unwrap();
    let python_package = python_emitter.emit(&python, &authority).unwrap();
    assert!(python_package.get("_models.py").is_some());

    let typescript_emitter = TypeScriptEmitter::new();
    let typescript = project(
        &resolved,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &typescript_emitter.generator_handlers(),
        &typescript_emitter.code_resources().unwrap(),
    )
    .unwrap();
    let typescript_package = typescript_emitter.emit(&typescript, &authority).unwrap();
    assert!(typescript_package.get("src/models.ts").is_some());

    let rust_emitter = RustEmitter::new();
    let rust = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &rust_emitter.generator_handlers(),
        &rust_emitter.code_resources().unwrap(),
    )
    .unwrap();
    let rust_package = rust_emitter.emit(&rust, &authority).unwrap();
    let rust_read = str::from_utf8(rust_package.get("src/read.rs").unwrap()).unwrap();
    assert!(rust_read.contains("pub enum IdentifierFamily"));
    assert!(rust_read.contains("pub fn value(&self) -> &String"));
}
