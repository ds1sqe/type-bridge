use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::TypeKind;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::{_schema::SchemaInfo, InstalledRuntimeProjection};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  base:
    owns:
      tag:
        card: { min: 0, max: 3 }
        ordered: true
        distinct: true
  person:
    sub: base
    owns:
      tag:
        card: { min: 0, max: 3 }
        ordered: true
relations:
  collection:
    relates:
      member:
        card: { min: 0, max: 3 }
        ordered: true
        distinct: true
  curated-collection:
    sub: collection
    relates:
      curated-member:
        as: member
        card: { min: 0, max: 3 }
        ordered: true
        distinct: true
"#;

#[test]
fn canonical_ordered_projection_renders_exact_typeql_3_12_list_interfaces() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("ordered-typeql.yaml").unwrap(),
        ORDERED_SCHEMA,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v2()],
        &[],
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(projection).unwrap();
    let descriptors = installed
        .projection()
        .models()
        .keys()
        .filter(|id| matches!(id.kind(), TypeKind::Entity | TypeKind::Relation))
        .map(|id| installed.descriptor(id).unwrap().clone())
        .collect::<Vec<_>>();

    let emitted = SchemaInfo::from_descriptors(&descriptors)
        .to_typeql()
        .unwrap();
    assert!(emitted.contains("owns tag[] @distinct @card(0..3)"));
    assert!(emitted.contains("entity person sub base,\n    owns tag[] @card(0..3);"));
    assert!(emitted.contains("relates member[] @distinct @card(0..3)"));
    assert!(emitted.contains("relates curated-member[] as member[] @distinct @card(0..3)"));
    typeql::parse_query(&emitted).expect("TypeQL 3.12 accepts the generated ordered schema");
}
