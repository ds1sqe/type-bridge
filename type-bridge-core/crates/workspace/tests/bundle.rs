use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::managed_scope::{ManagedScopeBinding, ManagedScopeId};
use type_bridge_contract::migration::MigrationAppLabel;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, CodeResourceDigest, ProjectionConfig, ProjectionHandler,
};
use type_bridge_workspace::{
    BundleProjectionContext, BundleVerificationContext, ExtensionRegistryService,
    ExtensionRequirement, MAX_SCHEMA_BUNDLE_BYTES, MigrationV2Directory, OutputDirectory,
    SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION, SCHEMA_BUNDLE_FINGERPRINT_DOMAIN,
    SchemaBundleErrorCode, SchemaSetPath, SecretReference, SecretReferenceService,
    TYPEBRIDGE_SCHEMA_BUNDLE_V1, TypeBridgeConfig, TypeBridgeConfigServices, TypeBridgeRuntime,
    TypeBridgeWorkspace, TypeBridgeWorkspaceServices, WorkspaceDirectoryAuthority, WorkspaceRoot,
    WorkspaceServiceError, build_verified_schema_bundle, c_symbol_prefix_for_app_label,
    decode_verified_schema_bundle, encode_verified_schema_bundle,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-schema-bundle-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("schema-set.yaml"),
            "# source-only manifest\nformat: typebridge.schema-set/v1\nsources: [schema.yaml]\n",
        )
        .unwrap();
        fs::write(
            path.join("schema.yaml"),
            "# source-only comment\nformat: typebridge.schema/v2\nentities:\n  person: {}\n  employee:\n    sub: person\n",
        )
        .unwrap();
        Self(path)
    }

    fn root(&self) -> WorkspaceRoot {
        WorkspaceRoot::new(fs::canonicalize(&self.0).unwrap()).unwrap()
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Secrets;

impl SecretReferenceService for Secrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

struct Extensions;

impl ExtensionRegistryService for Extensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

fn capabilities() -> CapabilitySet {
    ["schema.annotations", "schema.doc-meta", "schema.roles"]
        .into_iter()
        .map(|value| CapabilityId::new(value).unwrap())
        .collect()
}

fn extension() -> ExtensionRequirement {
    ExtensionRequirement::new("example.bundle", "1").unwrap()
}

fn workspace(directory: &TempDirectory) -> TypeBridgeWorkspace {
    let source = WorkspaceDirectoryAuthority::open(directory.root()).unwrap();
    let secrets = Secrets;
    let extensions = Extensions;
    let available = capabilities();
    let config = TypeBridgeConfig::builder(directory.root())
        .schema_set(SchemaSetPath::new("schema-set.yaml").unwrap())
        .app_label(MigrationAppLabel::new("example").unwrap())
        .exclusive_managed_scope(ManagedScopeId::new("example-schema").unwrap())
        .semantic_profile(SemanticProfileId::new("typedb-3.12.1/v1").unwrap())
        .migration_v2_directory(MigrationV2Directory::new("migrations/v2").unwrap())
        .require_capability(CapabilityId::new("schema.doc-meta").unwrap())
        .require_extension(extension())
        .output(
            BindingTarget::Python,
            OutputDirectory::new("generated/python").unwrap(),
        )
        .output(
            BindingTarget::TypeScript,
            OutputDirectory::new("generated/typescript").unwrap(),
        )
        .output(
            BindingTarget::Rust,
            OutputDirectory::new("generated/rust").unwrap(),
        )
        .output(
            BindingTarget::C,
            OutputDirectory::new("generated/c").unwrap(),
        )
        .build(&TypeBridgeConfigServices::new(
            &source,
            &secrets,
            &extensions,
        ))
        .unwrap();
    TypeBridgeWorkspace::from_config(
        config,
        &TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available),
    )
    .unwrap()
}

fn ordered_workspace(directory: &TempDirectory) -> TypeBridgeWorkspace {
    fs::write(
        directory.0.join("schema.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\n\
         entities:\n  person:\n    owns:\n      tag: { card: { min: 0, max: 3 }, ordered: true, distinct: true }\n",
    )
    .unwrap();
    workspace(directory)
}

fn context() -> BundleVerificationContext {
    context_with_c_prefix(c_symbol_prefix_for_app_label(
        &MigrationAppLabel::new("example").unwrap(),
    ))
}

fn context_with_c_prefix(c_symbol_prefix: CSymbolPrefix) -> BundleVerificationContext {
    BundleVerificationContext::new(
        SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        ManagedScopeBinding::exclusive(ManagedScopeId::new("example-schema").unwrap()).unwrap(),
        capabilities(),
        [extension()],
        [
            BundleProjectionContext::new(
                ProjectionConfig::python(),
                vec![ProjectionHandler::python_v1()],
            )
            .unwrap(),
            BundleProjectionContext::new(
                ProjectionConfig::typescript(),
                vec![ProjectionHandler::typescript_v1()],
            )
            .unwrap(),
            BundleProjectionContext::new(
                ProjectionConfig::rust(),
                vec![ProjectionHandler::rust_v1()],
            )
            .unwrap(),
            BundleProjectionContext::new(
                ProjectionConfig::c(c_symbol_prefix),
                vec![ProjectionHandler::c_v2()],
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

fn resources(target: BindingTarget) -> Vec<CodeResourceDigest> {
    let ids: &[&str] = match target {
        BindingTarget::Python => &[
            "typebridge.generator.python.py-typed",
            "typebridge.generator.python.query-source",
            "typebridge.generator.python.query-stub",
            "typebridge.generator.python.runtime-source",
            "typebridge.generator.python.runtime-stub",
        ],
        BindingTarget::TypeScript => &[
            "typebridge.generator.typescript.package-json",
            "typebridge.generator.typescript.runtime-source",
            "typebridge.generator.typescript.tsconfig-json",
        ],
        BindingTarget::Rust => &[
            "typebridge.generator.rust.cargo-toml",
            "typebridge.generator.rust.runtime-source",
        ],
        BindingTarget::C => &[
            "typebridge.generator.c.cmake-package-config-template",
            "typebridge.generator.c.cmake-template",
            "typebridge.generator.c.pkg-config-template",
        ],
        _ => panic!("test resource inventory does not support this binding target"),
    };
    ids.iter()
        .map(|id| {
            CodeResourceDigest::from_bytes(*id, format!("successor:{id}").as_bytes()).unwrap()
        })
        .collect()
}

fn ordered_context() -> BundleVerificationContext {
    BundleVerificationContext::new(
        SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        ManagedScopeBinding::exclusive(ManagedScopeId::new("example-schema").unwrap()).unwrap(),
        capabilities(),
        [extension()],
        [
            BundleProjectionContext::new_with_evidence(
                ProjectionConfig::python(),
                vec![ProjectionHandler::python_v2()],
                resources(BindingTarget::Python),
            )
            .unwrap(),
            BundleProjectionContext::new_with_evidence(
                ProjectionConfig::typescript(),
                vec![ProjectionHandler::typescript_v2()],
                resources(BindingTarget::TypeScript),
            )
            .unwrap(),
            BundleProjectionContext::new_with_evidence(
                ProjectionConfig::rust(),
                vec![ProjectionHandler::rust_v2()],
                resources(BindingTarget::Rust),
            )
            .unwrap(),
            BundleProjectionContext::new_with_evidence(
                ProjectionConfig::c(c_symbol_prefix_for_app_label(
                    &MigrationAppLabel::new("example").unwrap(),
                )),
                vec![ProjectionHandler::c_v3()],
                resources(BindingTarget::C),
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn bundle_creation_rejects_a_c_prefix_not_derived_from_the_workspace_app_label() {
    let directory = TempDirectory::new();
    let workspace = workspace(&directory);
    let context = context_with_c_prefix(CSymbolPrefix::new("foreign").unwrap());
    let error = build_verified_schema_bundle(&workspace, &context)
        .expect_err("a foreign C link namespace must not enter the workspace bundle");
    assert_eq!(error.code(), SchemaBundleErrorCode::ContextMismatch);
}

fn bundle_value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn rehash_declared(declared: &mut Value) {
    let identity = json!({
        "facts": declared["facts"].clone(),
        "format_version": declared["format_version"].clone(),
        "required_capabilities": declared["required_capabilities"].clone(),
    });
    let fingerprint = Fingerprint::compute(
        FingerprintDomain::new("typebridge.schema.declared-identity").unwrap(),
        CanonicalizationVersion::new("typebridge.schema-canonical-json/v1").unwrap(),
        None,
        &to_canonical_json(&identity).unwrap(),
    );
    declared["declared_identity"] = serde_json::to_value(fingerprint).unwrap();
}

fn rehash_bundle(bundle: &mut Value) {
    let fingerprint = Fingerprint::compute(
        FingerprintDomain::new(SCHEMA_BUNDLE_FINGERPRINT_DOMAIN).unwrap(),
        CanonicalizationVersion::new(SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION).unwrap(),
        None,
        &to_canonical_json(&bundle["content"]).unwrap(),
    );
    bundle["bundle_fingerprint"] = serde_json::to_value(fingerprint).unwrap();
}

fn python_projection(bundle: &mut Value) -> &mut Value {
    bundle["content"]["projections"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["target"] == "python")
        .unwrap()
}

#[test]
fn build_twice_decode_roundtrip_and_source_free_runtime_are_exact() {
    let directory = TempDirectory::new();
    let workspace = workspace(&directory);
    let context = context();
    let first = build_verified_schema_bundle(&workspace, &context).unwrap();
    let second = build_verified_schema_bundle(&workspace, &context).unwrap();
    let bytes = encode_verified_schema_bundle(&first);
    assert_eq!(bytes, encode_verified_schema_bundle(&second));
    let decoded = decode_verified_schema_bundle(&bytes, &context).unwrap();
    assert_eq!(encode_verified_schema_bundle(&decoded), bytes);
    assert_eq!(decoded.projections().len(), 4);

    let runtime = TypeBridgeRuntime::from_bundle_bytes(&bytes, &context).unwrap();
    assert_eq!(runtime.bundle_fingerprint(), first.bundle_fingerprint());
    assert_eq!(
        runtime.resolved_schema().semantic_fingerprint(),
        workspace.resolved_schema().semantic_fingerprint(),
    );
    assert_eq!(runtime.managed_state(), workspace.managed_state());
    assert_eq!(
        runtime.required_extensions(),
        &BTreeSet::from([extension()])
    );
    for target in [
        BindingTarget::Python,
        BindingTarget::TypeScript,
        BindingTarget::Rust,
        BindingTarget::C,
    ] {
        assert!(runtime.projection(target).is_some());
    }

    let value = bundle_value(&bytes);
    let c = value["content"]["projections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["target"] == "c")
        .expect("bundle wire retains the C projection target");
    assert_eq!(c["config"]["binding"], "c");
    assert_eq!(c["config"]["symbol_prefix"], "tb_example");

    let text = String::from_utf8(bytes).unwrap();
    assert!(
        !text.contains("resource_evidence"),
        "the legacy unordered bundle wire gained successor resource evidence",
    );
    assert!(!text.contains(directory.0.to_string_lossy().as_ref()));
    assert!(!text.contains("schema.yaml"));
    assert!(!text.contains("source-only"));
    assert!(!text.contains("resolved_cache"));
    let first_fact = runtime.declared_schema().facts().next().unwrap();
    let source =
        serde_json::to_value(runtime.declared_schema().source(&first_fact.id()).unwrap()).unwrap();
    assert_eq!(
        source["document"],
        "__typebridge_compiled__/declared-schema-v1"
    );
}

#[test]
fn ordered_bundle_binds_successor_handlers_and_resources_exactly() {
    let directory = TempDirectory::new();
    let ordered = ordered_workspace(&directory);
    let ordered_context = ordered_context();
    let bundle = build_verified_schema_bundle(&ordered, &ordered_context).unwrap();
    let bytes = encode_verified_schema_bundle(&bundle);
    let decoded = decode_verified_schema_bundle(&bytes, &ordered_context).unwrap();
    assert_eq!(encode_verified_schema_bundle(&decoded), bytes);

    let value = bundle_value(&bytes);
    for projection in value["content"]["projections"].as_array().unwrap() {
        let expected_version = if projection["target"] == "c" { 3 } else { 2 };
        assert_eq!(
            projection["handler_evidence"][0]["version"],
            expected_version
        );
        assert!(
            !projection["resource_evidence"]
                .as_array()
                .unwrap()
                .is_empty(),
            "ordered projection omitted its successor resource ledger",
        );
    }

    assert_eq!(
        build_verified_schema_bundle(&ordered, &context())
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::UnsupportedProjectionEvidence,
    );
    let unordered = TempDirectory::new();
    assert_eq!(
        build_verified_schema_bundle(&workspace(&unordered), &ordered_context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::UnsupportedProjectionEvidence,
    );

    let mut tampered = value;
    python_projection(&mut tampered)["resource_evidence"][0]["digest"] =
        Value::String("0".repeat(64));
    rehash_bundle(&mut tampered);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&tampered).unwrap(), &ordered_context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::ContextMismatch,
    );
}

#[test]
fn correct_outer_hash_does_not_trust_invalid_rehashed_declared_facts() {
    let directory = TempDirectory::new();
    let workspace = workspace(&directory);
    let context = context();
    let bundle = build_verified_schema_bundle(&workspace, &context).unwrap();
    let mut value = bundle_value(&encode_verified_schema_bundle(&bundle));
    let declared = &mut value["content"]["declared_schema"];
    let facts = declared["facts"].as_array_mut().unwrap();
    let person = facts
        .iter()
        .position(|fact| fact["kind"] == "type" && fact["value"]["id"]["label"] == "person")
        .unwrap();
    facts.remove(person);
    rehash_declared(declared);
    value["content"]["expected_declared_identity"] = declared["declared_identity"].clone();
    rehash_bundle(&mut value);

    let error =
        decode_verified_schema_bundle(&to_canonical_json(&value).unwrap(), &context).unwrap_err();
    assert_eq!(error.code(), SchemaBundleErrorCode::Contract);
    assert!(error.contract().is_some());
}

#[test]
fn versions_profile_capabilities_extensions_and_unknown_cache_fail_closed() {
    let directory = TempDirectory::new();
    let workspace = workspace(&directory);
    let context = context();
    let bytes =
        encode_verified_schema_bundle(&build_verified_schema_bundle(&workspace, &context).unwrap());

    let mut version = bundle_value(&bytes);
    version["content"]["bundle_version"] = Value::String("typebridge.schema-bundle/v2".to_owned());
    rehash_bundle(&mut version);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&version).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::UnsupportedVersion,
    );

    let mut profile = bundle_value(&bytes);
    profile["content"]["semantic_profile"]["id"] = Value::String("typedb-9.9.9/v1".to_owned());
    rehash_bundle(&mut profile);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&profile).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::IntegrityMismatch,
    );

    let mut capability = bundle_value(&bytes);
    capability["content"]["required_capabilities"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("schema.unknown".to_owned()));
    rehash_bundle(&mut capability);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&capability).unwrap(), &context,)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::Contract,
    );

    let mut extension = bundle_value(&bytes);
    extension["content"]["required_extensions"][0]["handler_id"] =
        Value::String("unknown.extension".to_owned());
    rehash_bundle(&mut extension);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&extension).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::ExtensionUnavailable,
    );

    let mut cache = bundle_value(&bytes);
    cache["content"]
        .as_object_mut()
        .unwrap()
        .insert("resolved_cache".to_owned(), json!({"trusted": true}));
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&cache).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::Contract,
    );
}

#[test]
fn stale_digest_fingerprints_projection_and_handler_evidence_are_rejected() {
    let directory = TempDirectory::new();
    let workspace = workspace(&directory);
    let context = context();
    let bytes =
        encode_verified_schema_bundle(&build_verified_schema_bundle(&workspace, &context).unwrap());

    let mut digest = bundle_value(&bytes);
    digest["bundle_fingerprint"]["digest"] = Value::String("0".repeat(64));
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&digest).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::IntegrityMismatch,
    );

    let mut fingerprint = bundle_value(&bytes);
    fingerprint["content"]["expected_semantic_schema"]["digest"] = Value::String("0".repeat(64));
    rehash_bundle(&mut fingerprint);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&fingerprint).unwrap(), &context,)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::IntegrityMismatch,
    );

    let mut projection = bundle_value(&bytes);
    python_projection(&mut projection)["canonical_projection"]["projection_fingerprint"]["digest"] =
        Value::String("0".repeat(64));
    rehash_bundle(&mut projection);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&projection).unwrap(), &context,)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::ProjectionMismatch,
    );

    let mut evidence = bundle_value(&bytes);
    python_projection(&mut evidence)["handler_evidence"][0]["version"] = Value::from(2);
    rehash_bundle(&mut evidence);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&evidence).unwrap(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::ContextMismatch,
    );

    let mut missing_target = bundle_value(&bytes);
    missing_target["content"]["projections"]
        .as_array_mut()
        .unwrap()
        .pop();
    rehash_bundle(&mut missing_target);
    assert_eq!(
        decode_verified_schema_bundle(&to_canonical_json(&missing_target).unwrap(), &context,)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::ProjectionTargetMismatch,
    );

    let mut malformed_extension = bundle_value(&bytes);
    malformed_extension["content"]["required_extensions"][0]["handler_id"] =
        Value::String(String::new());
    rehash_bundle(&mut malformed_extension);
    let error =
        decode_verified_schema_bundle(&to_canonical_json(&malformed_extension).unwrap(), &context)
            .unwrap_err();
    assert_eq!(error.code(), SchemaBundleErrorCode::Contract);
    assert!(error.config().is_some());
}

#[test]
fn projection_context_requires_the_exact_shipped_handler_set() {
    let error = BundleProjectionContext::new(
        ProjectionConfig::python(),
        vec![
            ProjectionHandler::python_v1(),
            ProjectionHandler::typescript_v1(),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code(), SchemaBundleErrorCode::ContextMismatch);

    let legacy_with_resources = BundleProjectionContext::new_with_evidence(
        ProjectionConfig::rust(),
        vec![ProjectionHandler::rust_v1()],
        resources(BindingTarget::Rust),
    )
    .unwrap_err();
    assert_eq!(
        legacy_with_resources.code(),
        SchemaBundleErrorCode::ContextMismatch
    );

    let successor_without_resources = BundleProjectionContext::new_with_evidence(
        ProjectionConfig::rust(),
        vec![ProjectionHandler::rust_v2()],
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(
        successor_without_resources.code(),
        SchemaBundleErrorCode::ContextMismatch
    );
}

#[test]
fn canonical_limits_and_frozen_v1_contract_are_enforced() {
    assert_eq!(TYPEBRIDGE_SCHEMA_BUNDLE_V1, "typebridge.schema-bundle/v1");
    assert_eq!(SCHEMA_BUNDLE_FINGERPRINT_DOMAIN, "typebridge.schema.bundle");
    assert_eq!(
        SCHEMA_BUNDLE_FINGERPRINT_CANONICALIZATION,
        "typebridge.schema-bundle/v1"
    );

    let context = context();
    let oversized = vec![b' '; MAX_SCHEMA_BUNDLE_BYTES + 1];
    assert_eq!(
        decode_verified_schema_bundle(&oversized, &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::Contract,
    );
    let mut deep = "[".repeat(65);
    deep.push('0');
    deep.push_str(&"]".repeat(65));
    assert_eq!(
        decode_verified_schema_bundle(deep.as_bytes(), &context)
            .unwrap_err()
            .code(),
        SchemaBundleErrorCode::Contract,
    );
}
