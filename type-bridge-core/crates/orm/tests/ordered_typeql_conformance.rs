use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{AnnotationKindId, DocumentId};
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

const SDK_V3_SCHEMA: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml");
const SDK_V3_PROVIDER_SCHEMA: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql");

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

#[test]
fn sdk_v3_fixture_projects_the_exact_ordered_provider_interfaces() {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("sdk-v3.yaml").unwrap(), SDK_V3_SCHEMA)])
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
    let nickname_id = TypeId::new(TypeKind::Attribute, "nickname").unwrap();
    let nickname_annotations = installed.projection().models()[&nickname_id]
        .declaration()
        .value_annotations();
    assert!(
        nickname_annotations
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Regex)
    );
    assert!(
        nickname_annotations
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Values)
    );
    let constrained_id = TypeId::new(TypeKind::Attribute, "val_constrained").unwrap();
    assert!(
        installed.projection().models()[&constrained_id]
            .declaration()
            .value_annotations()
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Range)
    );
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
    assert!(emitted.contains("owns aliases[] @unique @distinct @card(0..3)"));
    assert!(emitted.contains("relates participant[] @distinct @card(0..3)"));
    typeql::parse_query(SDK_V3_PROVIDER_SCHEMA)
        .expect("TypeQL 3.12 accepts the complete Sdk V3 provider schema");
}
