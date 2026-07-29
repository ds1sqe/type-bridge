use std::collections::BTreeSet;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedContainer, ProjectedModelForm, ProjectionConfig, ProjectionHandler,
    ReferenceConstructionPolicy,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::{
    AnnotationKindId, AnnotationSubjectId, DocumentId, RelatesFactId, SchemaFactId, SubFactId,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

fn projection(source: &str) -> type_bridge_contract::projection::RuntimeProjection {
    projection_for(
        source,
        BindingTarget::Python,
        ProjectionConfig::python(),
        ProjectionHandler::python_v1(),
    )
}

fn projection_for(
    source: &str,
    target: BindingTarget,
    config: ProjectionConfig,
    handler: ProjectionHandler,
) -> type_bridge_contract::projection::RuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses");
    let declared = normalize_documents(&documents).expect("fixture normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("fixture resolves");
    project(&resolved, target, &config, &[handler], &[]).expect("fixture projects")
}

#[test]
fn expanded_sub_survives_normalize_resolve_all_projections_and_wire_round_trip() {
    let source = r#"format: typebridge.schema/v2
entities:
  actor: {}
  person:
    doc: "person type"
    sub:
      type: actor
      doc: "person edge"
      meta: { owner: "schema", stability: "stable" }
"#;
    let actor = TypeId::new(TypeKind::Entity, "actor").unwrap();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let sub_id = SubFactId::new(person.clone(), actor).unwrap();

    for (target, config, handler) in [
        (
            BindingTarget::Python,
            ProjectionConfig::python(),
            ProjectionHandler::python_v1(),
        ),
        (
            BindingTarget::TypeScript,
            ProjectionConfig::typescript(),
            ProjectionHandler::typescript_v1(),
        ),
        (
            BindingTarget::Rust,
            ProjectionConfig::rust(),
            ProjectionHandler::rust_v1(),
        ),
    ] {
        let projected = projection_for(source, target, config, handler);
        let direct_sub = projected.models()[&person]
            .declaration()
            .direct_sub()
            .expect("projected child retains its exact direct edge");
        assert_eq!(direct_sub.id(), &sub_id);
        assert_eq!(direct_sub.origin(), &SchemaFactId::Sub(sub_id.clone()));
        assert_eq!(direct_sub.annotations().len(), 3);
        assert_eq!(
            direct_sub
                .annotations()
                .keys()
                .map(|id| id.kind().clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                AnnotationKindId::Doc,
                AnnotationKindId::Meta(type_bridge_contract::id::Label::new("owner").unwrap()),
                AnnotationKindId::Meta(type_bridge_contract::id::Label::new("stability").unwrap(),),
            ]),
        );

        let projection_bytes = to_canonical_json(&projected).unwrap();
        let projection_json: serde_json::Value = serde_json::from_slice(&projection_bytes).unwrap();
        let person_wire = projection_json["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"]["label"] == "person")
            .unwrap();
        assert_eq!(
            person_wire["declaration"]["direct_sub"]["origin"]["kind"],
            "sub"
        );
        assert_eq!(
            person_wire["declaration"]["direct_sub"]["annotations"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let semantic_bytes = to_canonical_json(projected.semantic_fingerprint()).unwrap();
        let binding_bytes = to_canonical_json(projected.projection_fingerprint()).unwrap();
        let decoded =
            decode_runtime_projection_verified(&projection_bytes, &semantic_bytes, &binding_bytes)
                .unwrap();
        assert_eq!(decoded, projected);
    }
}

#[test]
fn typescript_projection_is_target_specific_and_emitter_complete() {
    let projected = projection_for(
        r#"format: typebridge.schema/v2
attributes:
  display-name:
    value:
      type: string
      regex: "^.+$"
entities:
  actor:
    owns: [display-name]
relations:
  container:
    relates: [item]
  specialized-container:
    sub: container
    relates:
      special-item: { as: item }
  event: {}
plays:
  event:
    container:
      item: { doc: event edge }
functions:
  find-events:
    parameters:
      - { name: input-event, type: event }
    returns: { stream: [event] }
    body: { typeql: "match $event isa event; return { $event };" }
    doc: event lookup
    meta: { stability: stable }
"#,
        BindingTarget::TypeScript,
        ProjectionConfig::typescript(),
        ProjectionHandler::typescript_v1(),
    );

    let attribute = TypeId::new(TypeKind::Attribute, "display-name").unwrap();
    assert_eq!(
        projected.models()[&attribute].target_name().as_str(),
        "DisplayName"
    );
    assert!(
        projected.models()[&attribute]
            .declaration()
            .value_annotations()
            .values()
            .any(|annotation| annotation.id().kind() == &AnnotationKindId::Regex)
    );

    let container = TypeId::new(TypeKind::Relation, "container").unwrap();
    let event = TypeId::new(TypeKind::Relation, "event").unwrap();
    let item = RoleId::new("container", "item").unwrap();
    let create_forms = projected.models()[&container].create().roles()[&item]
        .players()
        .iter()
        .filter(|player| player.id() == &event)
        .map(|player| player.form())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        create_forms,
        BTreeSet::from([ProjectedModelForm::Complete, ProjectedModelForm::Reference,])
    );
    let read_forms = projected.models()[&container].complete_read().roles()[&item]
        .players()
        .iter()
        .map(|player| player.form())
        .collect::<BTreeSet<_>>();
    assert_eq!(read_forms, BTreeSet::from([ProjectedModelForm::Reference]));
    assert_eq!(
        projected.models()[&event]
            .reference_read()
            .construction_policy(),
        ReferenceConstructionPolicy::IidOnly,
    );
    let playing = projected
        .playing_facts()
        .values()
        .find(|playing| playing.id().player() == &event)
        .unwrap();
    assert_eq!(
        playing.target_name().unwrap().as_str(),
        "playsEventContainerItem"
    );

    let specialized = TypeId::new(TypeKind::Relation, "specialized-container").unwrap();
    let special_item = RoleId::new("specialized-container", "special-item").unwrap();
    assert_eq!(
        projected.models()[&specialized]
            .complete_read()
            .role_upcasts()[&special_item],
        vec![item],
    );
    let function = projected.functions()[&FunctionId::new("find-events").unwrap()].clone();
    assert_eq!(function.target_name().as_str(), "findEvents");
    assert_eq!(function.annotations().len(), 2);
}

#[test]
fn rust_projection_names_every_native_surface_and_reuses_resolved_semantics() {
    let projected = projection_for(
        r#"format: typebridge.schema/v2
attributes:
  display-name:
    value:
      type: string
      regex: "^.+$"
entities:
  actor:
    owns:
      display-name: { key: true }
relations:
  container:
    relates: [item]
  specialized-container:
    sub: container
    relates:
      special-item: { as: item }
  event: {}
plays:
  event:
    container: [item]
structs:
  player-stats:
    fields:
      - { name: wins, type: integer }
functions:
  find-events:
    parameters:
      - { name: input-event, type: event }
    returns: { stream: [event] }
    body: { typeql: "match $event isa event; return { $event };" }
"#,
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        ProjectionHandler::rust_v1(),
    );

    let container = TypeId::new(TypeKind::Relation, "container").unwrap();
    let event = TypeId::new(TypeKind::Relation, "event").unwrap();
    let item = RoleId::new("container", "item").unwrap();
    let model = &projected.models()[&container];
    assert_eq!(model.target_name().as_str(), "Container");
    assert_eq!(
        model.create().target_name().unwrap().as_str(),
        "ContainerCreate"
    );
    assert_eq!(
        model.reference_read().target_name().unwrap().as_str(),
        "ContainerRef"
    );
    assert_eq!(
        model.query_tokens().target_name().unwrap().as_str(),
        "ContainerType"
    );
    assert_eq!(
        model.query_tokens().roles()[&item].target_name().as_str(),
        "item"
    );
    assert_eq!(
        model.query_tokens().roles()[&item]
            .player_union_target_name()
            .unwrap()
            .as_str(),
        "ContainerItemPlayer",
    );
    let create_forms = model.create().roles()[&item]
        .players()
        .iter()
        .map(|player| player.form())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        create_forms,
        BTreeSet::from([ProjectedModelForm::Complete, ProjectedModelForm::Reference,])
    );
    assert_eq!(
        model.complete_read().roles()[&item]
            .players()
            .iter()
            .map(|player| player.form())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([ProjectedModelForm::Reference]),
    );
    assert_eq!(
        model.reference_read().construction_policy(),
        ReferenceConstructionPolicy::IidOnly
    );

    let specialized = TypeId::new(TypeKind::Relation, "specialized-container").unwrap();
    let special_item = RoleId::new("specialized-container", "special-item").unwrap();
    assert_eq!(
        projected.models()[&specialized]
            .complete_read()
            .role_upcasts()[&special_item],
        vec![item],
    );
    assert!(
        projected
            .emission()
            .model_link_components()
            .iter()
            .any(|component| { component == &BTreeSet::from([container.clone(), event.clone()]) })
    );

    let attribute = TypeId::new(TypeKind::Attribute, "display-name").unwrap();
    let actor = TypeId::new(TypeKind::Entity, "actor").unwrap();
    let field = projected.models()[&actor]
        .query_tokens()
        .fields()
        .values()
        .next()
        .unwrap();
    assert_eq!(field.target_name().as_str(), "display_name");
    assert!(field.is_key());
    assert!(
        !projected.models()[&attribute]
            .declaration()
            .value_annotations()
            .is_empty()
    );
    let playing = projected
        .playing_facts()
        .values()
        .find(|playing| playing.id().player() == &event)
        .unwrap();
    assert_eq!(
        playing.target_name().unwrap().as_str(),
        "plays_event_container_item"
    );
    assert_eq!(
        projected.structs()[&StructId::new("player-stats").unwrap()]
            .target_name()
            .as_str(),
        "PlayerStats",
    );
    assert_eq!(
        projected.functions()[&FunctionId::new("find-events").unwrap()]
            .target_name()
            .as_str(),
        "find_events",
    );
}

#[test]
fn rust_projection_rejects_global_derived_name_collisions() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
entities:
  person: {}
  person-create: {}
"#,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let error = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &[ProjectionHandler::rust_v1()],
        &[],
    )
    .unwrap_err();
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "projection_name_collision"
    );
}

#[test]
fn rust_projection_retains_member_names_for_namespace_aware_emitter_validation() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  try-new: { value: string }
entities:
  account:
    owns: [try-new]
"#,
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
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &[ProjectionHandler::rust_v1()],
        &[],
    )
    .unwrap();
    let account = TypeId::new(TypeKind::Entity, "account").unwrap();
    assert_eq!(
        projection.models()[&account]
            .query_tokens()
            .fields()
            .values()
            .next()
            .unwrap()
            .target_name()
            .as_str(),
        "try_new"
    );
}

#[test]
fn typescript_runtime_reserved_names_fail_before_emission() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
relations:
  bad:
    relates: [prototype]
"#,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let error = project(
        &resolved,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &[ProjectionHandler::typescript_v1()],
        &[],
    )
    .unwrap_err();
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "reserved_typescript_projection_identifier"
    );
}

#[test]
fn projects_five_facets_specialization_and_per_playing_metadata() {
    let projected = projection(
        r#"format: typebridge.schema/v2
entities:
  parent-player: {}
  specialized-player: {}
relations:
  membership:
    relates:
      member: { card: { min: 0, max: 2 } }
  employment:
    sub: membership
    relates:
      employee: { as: member, card: 1 }
plays:
  parent-player:
    membership:
      member: { card: { min: 0, max: 1 }, doc: parent edge }
  specialized-player:
    employment:
      employee: { card: { min: 0, max: 3 }, doc: child edge }
"#,
    );
    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    let employee = RoleId::new("employment", "employee").unwrap();
    let model = &projected.models()[&employment];
    assert_eq!(model.target_name().as_str(), "Employment");
    assert_eq!(
        model.declaration().parent().unwrap().label().as_str(),
        "membership"
    );
    assert!(!model.create().roles().contains_key(&member));
    assert!(model.create().roles().contains_key(&employee));
    let role = &model.query_tokens().roles()[&employee];
    assert_eq!(role.specializes(), Some(&member));
    assert_eq!(role.accepted_players().len(), 1);
    assert_eq!(role.multiplicity().container(), ProjectedContainer::Scalar);
    assert!(role.multiplicity().required());
    assert_eq!(projected.playing_facts().len(), 2);
    assert!(
        projected
            .playing_facts()
            .values()
            .any(|playing| playing.multiplicity().container() == ProjectedContainer::Sequence)
    );
}

#[test]
fn inherited_annotations_use_effective_projected_subject_identities() {
    let projected = projection(
        r#"format: typebridge.schema/v2
attributes:
  name:
    value: string
entities:
  base:
    owns:
      name: { doc: inherited ownership }
  child:
    sub: base
    doc: effective child type
relations:
  base-link:
    relates:
      member: { doc: inherited relation role }
  child-link:
    sub: base-link
plays:
  base:
    base-link:
      member: { doc: inherited playing }
"#,
    );

    let child = TypeId::new(TypeKind::Entity, "child").unwrap();
    let child_model = &projected.models()[&child];
    let type_annotation = child_model
        .declaration()
        .annotations()
        .values()
        .next()
        .expect("child type annotation is projected");
    assert_eq!(
        type_annotation.id().subject(),
        &AnnotationSubjectId::Type(child.clone()),
    );

    let field = child_model
        .query_tokens()
        .fields()
        .values()
        .next()
        .expect("inherited ownership is projected");
    let field_annotation = field
        .annotations()
        .values()
        .next()
        .expect("inherited ownership annotation is projected");
    assert_eq!(
        field_annotation.id().subject(),
        &AnnotationSubjectId::Owns(field.id().clone()),
    );

    let base_role = RoleId::new("base-link", "member").unwrap();
    let child_relation = TypeId::new(TypeKind::Relation, "child-link").unwrap();
    let role = &projected.models()[&child_relation].query_tokens().roles()[&base_role];
    let role_annotation = role
        .annotations()
        .values()
        .next()
        .expect("inherited relates annotation is projected");
    let effective_role = RoleId::new("child-link", "member").unwrap();
    let effective_relates = RelatesFactId::new(child_relation, effective_role).unwrap();
    assert_eq!(
        role_annotation.id().subject(),
        &AnnotationSubjectId::Relates(effective_relates),
    );

    let playing = projected
        .playing_facts()
        .values()
        .find(|playing| playing.id().player() == &child)
        .expect("inherited playing is projected for the child");
    let playing_annotation = playing
        .annotations()
        .values()
        .next()
        .expect("inherited playing annotation is projected");
    assert_eq!(
        playing_annotation.id().subject(),
        &AnnotationSubjectId::Plays(playing.id().clone()),
    );
}

#[test]
fn projects_relation_player_cycles_structs_and_typed_function_refs() {
    let projected = projection(
        r#"format: typebridge.schema/v2
relations:
  container:
    relates: [item]
  event: {}
plays:
  event:
    container: [item]
structs:
  player-stats:
    fields:
      - { name: wins, type: integer }
      - { name: nickname, type: string, optional: true }
functions:
  events:
    parameters:
      - { name: event, type: event }
    returns:
      stream: [event]
    body:
      typeql: |-
        match
          $event isa event;
        return { $event };
"#,
    );
    let container = TypeId::new(TypeKind::Relation, "container").unwrap();
    let event = TypeId::new(TypeKind::Relation, "event").unwrap();
    assert!(
        projected
            .emission()
            .model_link_components()
            .iter()
            .any(|component| component == &BTreeSet::from([container.clone(), event.clone()]))
    );
    let role = RoleId::new("container", "item").unwrap();
    let read_player = projected.models()[&container].complete_read().roles()[&role]
        .players()
        .first()
        .expect("relation player is projected");
    assert_eq!(read_player.form(), ProjectedModelForm::Reference);
    let structure = &projected.structs()[&StructId::new("player-stats").unwrap()];
    assert_eq!(structure.fields()[0].name().as_str(), "wins");
    assert!(structure.fields()[1].optional());
    let function = &projected.functions()[&FunctionId::new("events").unwrap()];
    assert_eq!(function.parameters()[0].target_name().as_str(), "event");
}

#[test]
fn naming_collisions_fail_closed_without_auto_suffixes() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").unwrap(),
        r#"format: typebridge.schema/v2
entities:
  foo-bar: {}
  foo_bar: {}
"#,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let error = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    )
    .unwrap_err();
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "projection_name_collision"
    );
}

#[test]
fn key_owns_projects_as_required_unique_exactly_one() {
    let runtime = projection(
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  account:
    owns:
      identifier: { key: true }
"#,
    );
    let account = TypeId::new(TypeKind::Entity, "account").expect("type identifier is valid");
    let identifier = AttributeId::new("identifier").expect("attribute identifier is valid");
    let field = runtime.models()[&account]
        .complete_read()
        .fields()
        .iter()
        .find(|field| field.token().attribute() == &identifier)
        .expect("key read field is projected");

    assert_eq!(field.multiplicity().cardinality().min(), 1);
    assert_eq!(field.multiplicity().cardinality().max(), Some(1));
}
