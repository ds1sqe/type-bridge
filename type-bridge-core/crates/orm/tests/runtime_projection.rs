use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::schema::DocumentId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_orm::manager::query_builder::{
    build_dynamic_entity_fetch, build_dynamic_entity_fetch_exact,
    build_dynamic_relation_fetch, build_dynamic_relation_fetch_exact,
};
use type_bridge_orm::{InstalledRuntimeProjection, TypeDescriptor};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

fn installed() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name: { card: 1 }
relations:
  membership:
    relates:
      member: { card: 1 }
  employment:
    sub: membership
    relates:
      employee: { as: member, card: 1 }
plays:
  person:
    membership: [member]
    employment: [employee]
"#,
    )]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(&declared, &SemanticProfileId::new("typedb-3.12.1/v1").unwrap()).unwrap();
    let runtime = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    ).unwrap();
    InstalledRuntimeProjection::try_new(runtime).unwrap()
}

#[test]
fn installed_projection_derives_exact_provider_descriptors_without_registry_state() {
    let installed = installed();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();

    let TypeDescriptor::Entity(person_descriptor) = installed.descriptor(&person).unwrap() else { panic!("person must be an entity") };
    assert_eq!(person_descriptor.owned_attributes[0].field_name, "name");
    assert_eq!(person_descriptor.owned_attributes[0].cardinality(), Some((1, Some(1))));

    let TypeDescriptor::Relation(employment_descriptor) = installed.descriptor(&employment).unwrap() else { panic!("employment must be a relation") };
    assert_eq!(employment_descriptor.parent_type.as_deref(), Some("membership"));
    assert_eq!(employment_descriptor.roles.len(), 1);
    assert_eq!(employment_descriptor.roles[0].role_name, "employee");
    assert_eq!(employment_descriptor.roles[0].overrides.as_deref(), Some("member"));
    assert_eq!(employment_descriptor.roles[0].player_type_names, ["person"]);
}

#[test]
fn exact_fetch_builders_add_isa_bang_without_changing_inclusive_builders() {
    let installed = installed();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let person = installed.entity_descriptor(&person).unwrap();
    let employment = installed.relation_descriptor(&employment).unwrap();

    let inclusive_entity = build_dynamic_entity_fetch(person, &[], "$e").unwrap();
    let exact_entity = build_dynamic_entity_fetch_exact(person, &[], "$e").unwrap();
    assert!(!inclusive_entity.contains("$e isa! person"));
    assert!(exact_entity.contains("$e isa! person"));

    let inclusive_relation = build_dynamic_relation_fetch(employment, &[], "$r").unwrap();
    let exact_relation = build_dynamic_relation_fetch_exact(employment, &[], "$r").unwrap();
    assert!(!inclusive_relation.contains("$r isa! employment"));
    assert!(exact_relation.contains("$r isa! employment"));
}
