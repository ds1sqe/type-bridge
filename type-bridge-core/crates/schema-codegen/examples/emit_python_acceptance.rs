use std::env;
use std::fs;
use std::path::PathBuf;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::PythonEmitter;

fn main() {
    let mut arguments = env::args_os().skip(1);
    let schema_path = PathBuf::from(arguments.next().expect("schema path is required"));
    let output_path = PathBuf::from(arguments.next().expect("output path is required"));
    assert!(arguments.next().is_none(), "only schema and output paths are accepted");

    let source = fs::read_to_string(&schema_path).expect("acceptance schema is readable");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("python-acceptance.yaml").expect("document ID is valid"),
        source,
    )])
    .expect("acceptance schema parses");
    let declared = normalize_documents(&documents).expect("acceptance schema normalizes");
    let profile =
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile is valid");
    let resolved = resolve(&declared, &profile).expect("acceptance schema resolves");
    let emitter = PythonEmitter::new();
    let handlers = emitter.generator_handlers();
    let resources = emitter.code_resources().expect("emitter resources hash");
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &handlers,
        &resources,
    )
    .expect("acceptance schema projects");
    let package = emitter.emit(&projection).expect("Python package emits");

    fs::create_dir_all(&output_path).expect("output directory is created");
    for (relative, bytes) in package.files() {
        fs::write(output_path.join(relative), bytes).expect("generated file is written");
    }
}
