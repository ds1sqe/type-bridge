use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{PythonEmitter, RustEmitter, TypeScriptEmitter};

const DOCUMENTED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  display-name: {value: string}
entities:
  actor: {}
  person:
    doc: Type */ documentation.
    sub: {type: actor, doc: Sub */ documentation.}
    owns:
      display-name: {doc: Owns */ documentation.}
relations:
  employment:
    relates:
      employee: {doc: Relates */ documentation.}
plays:
  person:
    employment:
      employee: {doc: Plays */ documentation.}
functions:
  documented:
    doc: Function */ documentation.
    parameters:
      - {name: person, type: person}
    returns:
      stream: [person]
    body:
      typeql: |-
        match
          $person isa person;
        return { $person };
"#;

fn projected(
    target: BindingTarget,
    config: ProjectionConfig,
    handlers: &[ProjectionHandler],
    resources: &[CodeResourceDigest],
) -> RuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("documentation.yaml").unwrap(),
        DOCUMENTED_SCHEMA,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    project(&resolved, target, &config, handlers, resources).unwrap()
}

#[test]
fn every_annotatable_doc_subject_reaches_safe_cross_target_documentation() {
    let python_emitter = PythonEmitter::new();
    let python_resources = python_emitter.code_resources().unwrap();
    let python_projection = projected(
        BindingTarget::Python,
        ProjectionConfig::python(),
        &python_emitter.generator_handlers(),
        &python_resources,
    );
    let python = python_emitter.emit(&python_projection).unwrap();
    let python_models = std::str::from_utf8(python.get("_models.pyi").unwrap()).unwrap();
    let python_schema = std::str::from_utf8(python.get("_schema.py").unwrap()).unwrap();
    assert!(python_models.contains(
        "\"Type */ documentation.\\n\\nDirect subtype of `actor`:\\nSub */ documentation.\""
    ));
    assert!(
        python_models
            .contains("    #: Owns */ documentation.\n    display_name: _FieldDescriptor["),
        "{python_models}"
    );
    assert!(
        python_models.contains("    #: Relates */ documentation.\n    employee: _RoleDescriptor[")
    );
    assert!(
        python_models.contains("#: Function */ documentation.\ndocumented: Final[FunctionRef[")
    );
    assert!(python_schema.contains("    #: Plays */ documentation.\n"));

    let typescript_emitter = TypeScriptEmitter::new();
    let typescript_resources = typescript_emitter.code_resources().unwrap();
    let typescript_projection = projected(
        BindingTarget::TypeScript,
        ProjectionConfig::typescript(),
        &typescript_emitter.generator_handlers(),
        &typescript_resources,
    );
    let typescript = typescript_emitter.emit(&typescript_projection).unwrap();
    let typescript_models = std::str::from_utf8(typescript.get("src/models.ts").unwrap()).unwrap();
    let typescript_schema = std::str::from_utf8(typescript.get("src/schema.ts").unwrap()).unwrap();
    let typescript_functions =
        std::str::from_utf8(typescript.get("src/functions.ts").unwrap()).unwrap();
    assert!(typescript_models.contains(
        " * Type *\\/ documentation.\n * \n * Direct subtype of `actor`:\n * Sub *\\/ documentation."
    ));
    assert!(
        typescript_models
            .contains("  /**\n   * Owns *\\/ documentation.\n   */\n  readonly displayName:")
    );
    assert!(
        typescript_models
            .contains("  /**\n   * Relates *\\/ documentation.\n   */\n  readonly employee:")
    );
    assert!(typescript_schema.contains(
        "/**\n * Plays *\\/ documentation.\n */\nexport const playsPersonEmploymentEmployee"
    ));
    assert!(
        typescript_functions
            .contains("/**\n * Function *\\/ documentation.\n */\nexport const documented:")
    );
    let rust_emitter = RustEmitter::new();
    let rust_resources = rust_emitter.code_resources().unwrap();
    let rust_projection = projected(
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        &rust_emitter.generator_handlers(),
        &rust_resources,
    );
    let rust = rust_emitter.emit(&rust_projection).unwrap();
    let rust_read = std::str::from_utf8(rust.get("src/read.rs").unwrap()).unwrap();
    let rust_tokens = std::str::from_utf8(rust.get("src/tokens.rs").unwrap()).unwrap();
    let rust_functions = std::str::from_utf8(rust.get("src/functions.rs").unwrap()).unwrap();
    let rust_manifest = std::str::from_utf8(rust.get("Cargo.toml").unwrap()).unwrap();
    assert!(rust_read.contains(
        "/// Type */ documentation.\n/// \n/// Direct subtype of `actor`:\n/// Sub */ documentation."
    ));
    assert!(
        rust_tokens.contains("  /// Owns */ documentation.\n  pub const display_name: FieldToken<")
    );
    assert!(
        rust_tokens.contains("  /// Relates */ documentation.\n  pub const employee: RoleToken<")
    );
    assert!(rust_tokens.contains(
        "/// Plays */ documentation.\n#[allow(non_upper_case_globals)]\npub const plays_person_employment_employee:"
    ));
    assert!(rust_functions.contains(
        "/// Function */ documentation.\n#[allow(non_upper_case_globals)]\npub const documented:"
    ));
    assert!(rust_manifest.contains("doctest = false"));
}
