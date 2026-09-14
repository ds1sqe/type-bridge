use std::env;
use std::fs;
use std::path::PathBuf;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::TypeScriptEmitter;

fn main() {
    let mut arguments = env::args_os().skip(1);
    let schema_path = PathBuf::from(arguments.next().expect("schema path is required"));
    let output_path = PathBuf::from(arguments.next().expect("output path is required"));
    assert!(
        arguments.next().is_none(),
        "only schema and output paths are accepted"
    );

    let source = fs::read_to_string(&schema_path).expect("acceptance schema is readable");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("typescript-acceptance.yaml").expect("document ID is valid"),
        source,
    )])
    .expect("acceptance schema parses");
    let declared = normalize_documents(&documents).expect("acceptance schema normalizes");
    let profile_name = env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
        .unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    let profile = SemanticProfileId::new(&profile_name).expect("semantic profile is valid");
    let resolved = resolve(&declared, &profile).expect("acceptance schema resolves");
    let emitter = TypeScriptEmitter::new();
    let handlers = emitter.generator_handlers_for(&resolved);
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("emitter resources hash");
    let mut config = ProjectionConfig::typescript();
    if let Ok(overrides) = env::var("TYPE_BRIDGE_ACCEPTANCE_TYPE_NAMES") {
        let overrides: Vec<(type_bridge_contract::id::TypeId, String)> =
            serde_json::from_str(&overrides).expect("acceptance type-name overrides decode");
        for (type_id, name) in overrides {
            config = config
                .with_type_name_override(type_id, name)
                .expect("valid override");
        }
    }
    let projection = project(
        &resolved,
        BindingTarget::TypeScript,
        &config,
        &handlers,
        &resources,
    )
    .expect("acceptance schema projects");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("node-generated-acceptance").expect("acceptance scope"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("acceptance authority builds");
    let package = emitter
        .emit(&projection, &authority)
        .expect("TypeScript package emits");

    fs::create_dir_all(&output_path).expect("output directory is created");
    for (relative, bytes) in package.files() {
        let path = output_path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("generated parent directory is created");
        }
        fs::write(path, bytes).expect("generated file is written");
    }
}
