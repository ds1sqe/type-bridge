use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, SchemaDocumentSet, normalize_documents, resolve,
    resolve_schema_with_capabilities,
};

fn declared(source: &str) -> type_bridge_contract::schema::DeclaredSchema {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses");
    normalize_documents(&documents).expect("fixture normalizes")
}

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid")
}

#[test]
fn resolves_inherited_value_owns_and_profile_cardinality() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  party:
    owns: [name]
  person:
    sub: party
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let person = TypeId::new(TypeKind::Entity, "person").expect("type identifier is valid");
    let name = AttributeId::new("name").expect("attribute identifier is valid");
    let person = &resolved.types()[&person];
    let owns = &person.owns()[&name];
    assert!(!owns.origin().is_direct());
    assert_eq!(owns.cardinality().min(), 0);
    assert_eq!(owns.cardinality().max(), Some(1));

    let name_type = TypeId::new(TypeKind::Attribute, "name").expect("type identifier is valid");
    assert_eq!(
        resolved.types()[&name_type]
            .value_type()
            .expect("value type resolves")
            .value_type(),
        type_bridge_contract::value::ValueTypeTag::String
    );
}

#[test]
fn key_owns_materializes_exactly_one_and_unique_through_inheritance() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  account:
    owns:
      identifier: { key: true }
  customer:
    sub: account
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("key schema resolves");
    let identifier = AttributeId::new("identifier").expect("attribute identifier is valid");

    for owner in ["account", "customer"] {
        let owner = TypeId::new(TypeKind::Entity, owner).expect("type identifier is valid");
        let owns = &resolved.types()[&owner].owns()[&identifier];
        assert_eq!(owns.cardinality().min(), 1);
        assert_eq!(owns.cardinality().max(), Some(1));
        assert!(owns.is_key());
        assert!(owns.is_unique());
    }
}

#[test]
fn role_specialization_replaces_the_parent_write_interface() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  membership:
    relates: [member]
  employment:
    sub: membership
    relates:
      employee: { as: member }
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let employment =
        TypeId::new(TypeKind::Relation, "employment").expect("type identifier is valid");
    let relation = &resolved.types()[&employment];
    let member = RoleId::new("membership", "member").expect("role identifier is valid");
    let employee = RoleId::new("employment", "employee").expect("role identifier is valid");
    assert!(!relation.relates().contains_key(&member));
    assert!(
        relation.relates()[&employee]
            .replaced_roles()
            .contains(&member)
    );
}

#[test]
fn inherited_relates_rebinds_effective_relation_but_preserves_origin() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  membership:
    relates: [member]
  employment:
    sub: membership
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let employment =
        TypeId::new(TypeKind::Relation, "employment").expect("type identifier is valid");
    let member = RoleId::new("membership", "member").expect("role identifier is valid");
    let relates = &resolved.types()[&employment].relates()[&member];
    assert_eq!(relates.id().relation(), &employment);
    assert!(!relates.origin().is_direct());
    assert_eq!(
        relates.origin().declared(),
        &type_bridge_contract::schema::SchemaFactId::Relates(
            type_bridge_contract::schema::RelatesFactId::new(
                TypeId::new(TypeKind::Relation, "membership").expect("type identifier is valid"),
                member,
            )
            .expect("relates identifier is valid"),
        )
    );
}

#[test]
fn specialized_roles_accept_only_explicit_effective_players() {
    let schema = declared(
        r#"format: typebridge.schema/v2
entities:
  parent-player: {}
  specialized-player: {}
relations:
  membership:
    relates: [member]
  employment:
    sub: membership
    relates:
      employee: { as: member }
plays:
  parent-player:
    membership: [member]
  specialized-player:
    employment: [employee]
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let member = RoleId::new("membership", "member").expect("role identifier is valid");
    let employee = RoleId::new("employment", "employee").expect("role identifier is valid");
    let parent_player =
        TypeId::new(TypeKind::Entity, "parent-player").expect("type identifier is valid");
    let specialized_player =
        TypeId::new(TypeKind::Entity, "specialized-player").expect("type identifier is valid");

    assert!(
        resolved.roles()[&member]
            .accepted_players()
            .contains(&parent_player)
    );
    assert!(
        !resolved.roles()[&employee]
            .accepted_players()
            .contains(&parent_player)
    );
    assert!(
        resolved.roles()[&employee]
            .accepted_players()
            .contains(&specialized_player)
    );
}

#[test]
fn relation_types_are_valid_concrete_role_players() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  container:
    relates: [item]
  event: {}
plays:
  event:
    container: [item]
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let role = RoleId::new("container", "item").expect("role identifier is valid");
    let container = TypeId::new(TypeKind::Relation, "container").expect("type identifier is valid");
    let event = TypeId::new(TypeKind::Relation, "event").expect("type identifier is valid");
    assert!(resolved.roles()[&role].accepted_players().contains(&event));
    assert!(
        resolved
            .dependency_graph()
            .dependencies(&event)
            .expect("event has projection dependencies")
            .contains(&container)
    );
    assert!(
        resolved
            .dependency_graph()
            .dependencies(&container)
            .expect("container has projection dependencies")
            .contains(&event)
    );
    assert!(
        resolved
            .dependency_graph()
            .strongly_connected_components()
            .iter()
            .any(|component| {
                component.len() == 2 && component.contains(&container) && component.contains(&event)
            })
    );
}

#[test]
fn inheritance_cycles_fail_before_resolution_output() {
    let schema = declared(
        r#"format: typebridge.schema/v2
entities:
  first: { sub: second }
  second: { sub: first }
"#,
    );
    assert!(resolve(&schema, &profile()).is_err());
}

#[test]
fn resolution_is_deterministic_and_does_not_mutate_declared_identity() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person: { owns: [name] }
"#,
    );
    let before = schema.declared_identity_fingerprint().clone();
    let first = resolve(&schema, &profile()).expect("first resolution succeeds");
    let second = resolve(&schema, &profile()).expect("second resolution succeeds");
    assert_eq!(first, second);
    assert_eq!(&before, schema.declared_identity_fingerprint());
    assert_eq!(first.descriptor_index().iter().len(), schema.facts().len());
}

#[test]
fn resolves_structs_without_reordering_fields() {
    let schema = declared(
        r#"format: typebridge.schema/v2
structs:
  player-stats:
    fields:
      - name: wins
        type: integer
      - name: losses
        type: integer
      - name: nickname
        type: string
        optional: true
"#,
    );

    let before = schema.declared_identity_fingerprint().clone();
    let first = resolve(&schema, &profile()).expect("schema resolves");
    let second = resolve(&schema, &profile()).expect("schema resolves");
    let id = StructId::new("player-stats").expect("struct identifier is valid");
    let fields = first.structs()[&id].fields();

    assert_eq!(fields[0].name().as_str(), "wins");
    assert_eq!(fields[1].name().as_str(), "losses");
    assert_eq!(fields[2].name().as_str(), "nickname");
    assert!(fields[2].optional());
    assert_eq!(first, second);
    assert_eq!(&before, schema.declared_identity_fingerprint());
}

#[test]
fn resolves_functions_without_rewriting_signature_or_body() {
    let schema = declared(
        r#"format: typebridge.schema/v2
entities:
  person: {}
functions:
  people:
    parameters:
      - name: person
        type: person
    returns:
      stream: [person]
    body:
      typeql: |-
        match
          $person isa person;
        return { $person };
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let id = FunctionId::new("people").expect("function identifier is valid");
    let declaration = resolved.functions()[&id].declaration();

    assert_eq!(
        declaration.signature().parameters()[0].name().as_str(),
        "person"
    );
    assert_eq!(
        declaration.body().text(),
        "match\n  $person isa person;\nreturn { $person };"
    );
}

#[test]
fn injected_capabilities_accept_exact_requirements_and_reject_missing_ones() {
    let schema = declared(
        r#"format: typebridge.schema/v2
capabilities:
  required: [schema.roles]
relations:
  membership: { relates: [member] }
"#,
    );
    let available = CapabilitySet::from_iter([
        CapabilityId::new("schema.roles").expect("capability ID is valid")
    ]);
    resolve_schema_with_capabilities(&schema, &profile(), &available)
        .expect("advertised required capability resolves");

    let error = resolve_schema_with_capabilities(&schema, &profile(), &CapabilitySet::new())
        .expect_err("missing required capability rejects");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "unsupported_required_capability",
    );
}

#[test]
fn compatibility_resolver_uses_a_closed_builtin_schema_capability_set() {
    assert_eq!(
        BUILTIN_SCHEMA_CAPABILITY_IDS,
        &["schema.annotations", "schema.doc-meta", "schema.roles"],
    );

    let supported = declared(
        r#"format: typebridge.schema/v2
capabilities:
  required: [schema.roles]
relations:
  membership: { relates: [member] }
"#,
    );
    resolve(&supported, &profile()).expect("built-in schema capability resolves");

    let future = declared(
        r#"format: typebridge.schema/v2
capabilities:
  required: [schema.future-feature]
"#,
    );
    let error = resolve(&future, &profile()).expect_err("unknown capability rejects");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "unsupported_required_capability",
    );

    let explicitly_available =
        CapabilitySet::from_iter([
            CapabilityId::new("schema.future-feature").expect("capability ID is valid")
        ]);
    resolve_schema_with_capabilities(&future, &profile(), &explicitly_available)
        .expect("explicitly injected open capability resolves");
}
