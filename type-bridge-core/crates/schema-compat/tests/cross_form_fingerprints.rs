use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, semantic_schema_fingerprint};
use type_bridge_schema_compat::{toml_to_declared, typeql_to_declared};

const SCHEMA_YAML: &str = include_str!("fixtures/cross_form/schema.yaml");
const SCHEMA_TYPEQL: &str = include_str!("fixtures/cross_form/schema.tql");
const LEGACY_YAML: &str = include_str!("fixtures/cross_form/legacy.yaml");
const LEGACY_TYPEQL: &str = include_str!("fixtures/cross_form/legacy.tql");
const LEGACY_TOML: &str = include_str!("fixtures/cross_form/legacy.toml");

fn document_id(path: &str) -> DocumentId {
    DocumentId::new(path).expect("fixture document identifier is valid")
}

fn yaml_to_declared(path: &str, source: &str) -> DeclaredSchema {
    let documents = SchemaDocumentSet::parse([(document_id(path), source)])
        .expect("cross-form YAML fixture parses");
    normalize_documents(&documents).expect("cross-form YAML fixture normalizes")
}

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture semantic profile is valid")
}

fn assert_equal_fingerprints(context: &str, left: &DeclaredSchema, right: &DeclaredSchema) {
    assert_eq!(
        left.declared_identity_fingerprint(),
        right.declared_identity_fingerprint(),
        "{context}: declared identity differs",
    );
    assert_eq!(
        semantic_schema_fingerprint(left, &profile()).expect("left semantic fingerprint computes"),
        semantic_schema_fingerprint(right, &profile())
            .expect("right semantic fingerprint computes"),
        "{context}: semantic fingerprint differs",
    );
}

#[test]
fn yaml_and_typeql_share_fingerprints_across_the_supported_schema_overlap() {
    let yaml = yaml_to_declared("schema/cross-form.yaml", SCHEMA_YAML);
    let typeql = typeql_to_declared(document_id("schema/cross-form.tql"), SCHEMA_TYPEQL)
        .expect("cross-form TypeQL fixture adapts");

    assert_equal_fingerprints("YAML versus TypeQL", &yaml, &typeql);
}

#[test]
fn yaml_typeql_and_legacy_toml_share_both_fingerprint_domains() {
    let yaml = yaml_to_declared("schema/legacy-overlap.yaml", LEGACY_YAML);
    let typeql = typeql_to_declared(document_id("schema/legacy-overlap.tql"), LEGACY_TYPEQL)
        .expect("legacy-overlap TypeQL fixture adapts");
    let toml = toml_to_declared(document_id("generated/legacy-overlap.tql"), LEGACY_TOML)
        .expect("legacy TOML fixture adapts");

    assert_equal_fingerprints("YAML versus TypeQL", &yaml, &typeql);
    assert_equal_fingerprints("YAML versus converted TOML", &yaml, &toml);
}
