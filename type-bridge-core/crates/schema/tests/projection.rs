use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectedContainer, ProjectedModelForm, ProjectedTokenIdentity,
    ProjectedTokenKind, ProjectionConfig, ProjectionHandler, ReferenceConstructionPolicy,
    TYPE_BRIDGE_C_CREATE_FIELD_MAX, TYPE_BRIDGE_C_CREATE_MEMBER_MAX, TYPE_BRIDGE_C_CREATE_ROLE_MAX,
    TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::{
    AnnotationKindId, AnnotationSubjectId, CollectionMode, DocumentId, OwnsFactId, RelatesFactId,
    SchemaFactId, SubFactId,
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

#[test]
fn ordered_max_one_owns_and_relates_project_as_ordered_sequences_in_every_facet() {
    let runtime = projection(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article:
    owns:
      tag: { ordered: true, distinct: true, card: { min: 0, max: 1 } }
  item: {}
relations:
  collection:
    relates:
      member: { ordered: true, distinct: true, card: { min: 0, max: 1 } }
plays:
  item:
    collection: [member]
"#,
    );

    let article = TypeId::new(TypeKind::Entity, "article").unwrap();
    let tag = AttributeId::new("tag").unwrap();
    let field_token = runtime.models()[&article]
        .query_tokens()
        .fields()
        .values()
        .find(|field| field.id().attribute() == &tag)
        .unwrap();
    assert_eq!(
        field_token.multiplicity().collection_mode(),
        CollectionMode::OrderedList
    );
    assert_eq!(
        field_token.multiplicity().container(),
        ProjectedContainer::Sequence
    );
    assert!(
        field_token
            .annotations()
            .keys()
            .any(|annotation| { annotation.kind() == &AnnotationKindId::Distinct })
    );
    let model = &runtime.models()[&article];
    assert_eq!(
        model.create().fields()[0].multiplicity(),
        field_token.multiplicity()
    );
    assert_eq!(
        model.complete_read().fields()[0].multiplicity(),
        field_token.multiplicity()
    );

    let collection = TypeId::new(TypeKind::Relation, "collection").unwrap();
    let member = RoleId::new("collection", "member").unwrap();
    let model = &runtime.models()[&collection];
    let role_token = &model.query_tokens().roles()[&member];
    assert_eq!(
        role_token.multiplicity().collection_mode(),
        CollectionMode::OrderedList
    );
    assert_eq!(
        role_token.multiplicity().container(),
        ProjectedContainer::Sequence
    );
    assert!(
        role_token
            .annotations()
            .keys()
            .any(|annotation| { annotation.kind() == &AnnotationKindId::Distinct })
    );
    assert_eq!(
        model.create().roles()[&member].multiplicity(),
        role_token.multiplicity()
    );
    assert_eq!(
        model.complete_read().roles()[&member].multiplicity(),
        role_token.multiplicity()
    );
}

#[test]
fn unordered_max_one_projection_retains_scalar_shape_and_omits_mode_bytes() {
    let runtime = projection(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article: { owns: [tag] }
relations:
  collection: { relates: [member] }
"#,
    );
    for multiplicity in runtime.models().values().flat_map(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .map(|token| token.multiplicity())
            .chain(
                model
                    .query_tokens()
                    .roles()
                    .values()
                    .map(|token| token.multiplicity()),
            )
    }) {
        assert_eq!(multiplicity.collection_mode(), CollectionMode::Unordered);
        assert_eq!(multiplicity.container(), ProjectedContainer::Scalar);
    }
    let canonical = String::from_utf8(to_canonical_json(&runtime).unwrap()).unwrap();
    assert!(!canonical.contains("collection_mode"));
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

fn c_projection(
    source: &str,
    symbol_prefix: &str,
) -> type_bridge_contract::projection::RuntimeProjection {
    c_projection_result(source, symbol_prefix).expect("fixture projects")
}

fn c_projection_result(
    source: &str,
    symbol_prefix: &str,
) -> Result<
    type_bridge_contract::projection::RuntimeProjection,
    type_bridge_contract::schema::SchemaDiagnostics,
> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses");
    let declared = normalize_documents(&documents).expect("fixture normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("fixture resolves");
    project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new(symbol_prefix).expect("C prefix is valid")),
        &[ProjectionHandler::c_v2()],
        &[],
    )
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
        (
            BindingTarget::C,
            ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
            ProjectionHandler::c_v2(),
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
fn c_projection_prefixes_and_names_every_native_surface() {
    let projected = c_projection(
        r#"format: typebridge.schema/v2
attributes:
  display-name:
    value: string
entities:
  actor:
    owns:
      display-name: { key: true }
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
      - { name: win-count, type: integer }
functions:
  find-events:
    parameters:
      - { name: input-event, type: event }
    returns: { stream: [event] }
    body: { typeql: "match $event isa event; return { $event };" }
"#,
        "acme",
    );

    let container = TypeId::new(TypeKind::Relation, "container").unwrap();
    let item = RoleId::new("container", "item").unwrap();
    let model = &projected.models()[&container];
    assert_eq!(model.target_name().as_str(), "acme_container");
    assert_eq!(
        model.create().target_name().unwrap().as_str(),
        "acme_container_create"
    );
    assert_eq!(
        model.reference_read().target_name().unwrap().as_str(),
        "acme_container_ref"
    );
    assert_eq!(
        model.query_tokens().target_name().unwrap().as_str(),
        "acme_container_type"
    );
    assert_eq!(
        model.query_tokens().roles()[&item].target_name().as_str(),
        "acme_item"
    );
    assert_eq!(
        model.query_tokens().roles()[&item]
            .player_union_target_name()
            .unwrap()
            .as_str(),
        "acme_container_item_player"
    );

    let actor = TypeId::new(TypeKind::Entity, "actor").unwrap();
    assert_eq!(
        projected.models()[&actor]
            .query_tokens()
            .fields()
            .values()
            .next()
            .unwrap()
            .target_name()
            .as_str(),
        "acme_displayzhname"
    );
    assert_eq!(
        projected
            .playing_facts()
            .values()
            .next()
            .unwrap()
            .target_name()
            .unwrap()
            .as_str(),
        "acme_plays_event_relation_container_role_item"
    );
    assert_eq!(
        projected.structs()[&StructId::new("player-stats").unwrap()]
            .target_name()
            .as_str(),
        "acme_playerzhstats"
    );
    assert_eq!(
        projected.structs()[&StructId::new("player-stats").unwrap()].fields()[0]
            .target_name()
            .as_str(),
        "acme_winzhcount"
    );
    let function = &projected.functions()[&FunctionId::new("find-events").unwrap()];
    assert_eq!(function.target_name().as_str(), "acme_findzhevents");
    assert_eq!(
        function.parameters()[0].target_name().as_str(),
        "acme_inputzhevent"
    );

    let assert_prefixed = |name: &str| {
        assert!(name.starts_with("acme_"), "{name}");
        assert!(
            !name.contains("__"),
            "C++ reserves identifiers containing a double underscore: {name}",
        );
    };
    for model in projected.models().values() {
        assert_prefixed(model.target_name().as_str());
        assert_prefixed(model.query_tokens().target_name().unwrap().as_str());
        if let Some(name) = model.create().target_name() {
            assert_prefixed(name.as_str());
        }
        if let Some(name) = model.reference_read().target_name() {
            assert_prefixed(name.as_str());
        }
        for field in model.query_tokens().fields().values() {
            assert_prefixed(field.target_name().as_str());
        }
        for role in model.query_tokens().roles().values() {
            assert_prefixed(role.target_name().as_str());
            assert_prefixed(role.player_union_target_name().unwrap().as_str());
        }
    }
    for structure in projected.structs().values() {
        assert_prefixed(structure.target_name().as_str());
        for field in structure.fields() {
            assert_prefixed(field.target_name().as_str());
        }
    }
    for function in projected.functions().values() {
        assert_prefixed(function.target_name().as_str());
        for parameter in function.parameters() {
            assert_prefixed(parameter.target_name().as_str());
        }
    }
    for playing in projected.playing_facts().values() {
        assert_prefixed(playing.target_name().unwrap().as_str());
    }
}

#[test]
fn generated_projection_token_ordinals_round_trip_semantic_identities() {
    let projected = c_projection(
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  membership:
    relates: [member]
plays:
  person:
    membership: [member]
functions:
  alpha-score:
    parameters: []
    returns: { scalar: integer }
    body: { typeql: "match let $score = 1; return first $score;" }
  find-people:
    parameters: []
    returns: { stream: [person] }
    body: { typeql: "match $person isa person; return { $person };" }
"#,
        "workforce",
    );

    assert_eq!(TYPE_BRIDGE_PROJECTED_TOKEN_VERSION, 1);
    assert_eq!(ProjectedTokenKind::Model.as_u32(), 1);
    assert_eq!(ProjectedTokenKind::Field.as_u32(), 2);
    assert_eq!(ProjectedTokenKind::Role.as_u32(), 3);
    assert_eq!(ProjectedTokenKind::Function.as_u32(), 4);
    assert_eq!(ProjectedTokenKind::Struct.as_u32(), 5);
    assert_eq!(ProjectedTokenKind::Attribute.as_u32(), 6);
    for (value, expected) in [
        (0, None),
        (1, Some(ProjectedTokenKind::Model)),
        (2, Some(ProjectedTokenKind::Field)),
        (3, Some(ProjectedTokenKind::Role)),
        (4, Some(ProjectedTokenKind::Function)),
        (5, Some(ProjectedTokenKind::Struct)),
        (6, Some(ProjectedTokenKind::Attribute)),
        (7, None),
    ] {
        assert_eq!(ProjectedTokenKind::from_u32(value), expected);
    }

    for kind in [
        ProjectedTokenKind::Model,
        ProjectedTokenKind::Field,
        ProjectedTokenKind::Role,
        ProjectedTokenKind::Function,
    ] {
        let mut ordinal = 0_u32;
        while let Some(identity) = projected.projected_token_identity(kind, ordinal) {
            assert_eq!(identity.kind(), kind);
            assert_eq!(projected.projected_token_ordinal(&identity), Some(ordinal));
            ordinal += 1;
        }
        assert!(ordinal > 0, "fixture must expose a {kind:?} token");
        assert!(projected.projected_token_identity(kind, ordinal).is_none());
        assert!(projected.projected_token_identity(kind, u32::MAX).is_none());
    }

    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let identifier =
        OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
    let field = ProjectedTokenIdentity::Field {
        owner: person.clone(),
        field: identifier.clone(),
    };
    assert!(projected.projected_token_ordinal(&field).is_some());
    assert!(
        projected
            .projected_token_ordinal(&ProjectedTokenIdentity::Field {
                owner: TypeId::new(TypeKind::Relation, "membership").unwrap(),
                field: identifier,
            })
            .is_none(),
        "effective owner branding must participate in token identity",
    );

    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    assert!(
        projected
            .projected_token_ordinal(&ProjectedTokenIdentity::Role {
                owner: membership,
                role: member,
            })
            .is_some()
    );

    let alpha = FunctionId::new("alpha-score").unwrap();
    let function = FunctionId::new("find-people").unwrap();
    assert_eq!(
        projected.projected_token_identity(ProjectedTokenKind::Function, 0),
        Some(ProjectedTokenIdentity::Function(alpha.clone()))
    );
    assert_eq!(
        projected.projected_token_identity(ProjectedTokenKind::Function, 1),
        Some(ProjectedTokenIdentity::Function(function.clone()))
    );
    assert_eq!(
        projected.projected_token_ordinal(&ProjectedTokenIdentity::Function(alpha)),
        Some(0)
    );
    assert_eq!(
        projected.projected_token_ordinal(&ProjectedTokenIdentity::Function(function)),
        Some(1)
    );
}

#[test]
fn c_v1_projection_escaping_is_injective_and_keyword_safe() {
    let projected = c_projection(
        r#"format: typebridge.schema/v2
entities:
  foo-bar: {}
  foo_bar: {}
  café: {}
  z: {}
  zh: {}
  z-h: {}
  while: {}
  __private: {}
  person: {}
  person-ref: {}
  person_ref: {}
"#,
        "acme",
    );

    for (label, expected) in [
        ("foo-bar", "acme_foozhbar"),
        ("foo_bar", "acme_foozubar"),
        ("café", "acme_cafzxc3zxa9"),
        ("z", "acme_zz"),
        ("zh", "acme_zzh"),
        ("z-h", "acme_zzzhh"),
        ("while", "acme_while"),
        ("__private", "acme_zuzuprivate"),
        ("person-ref", "acme_personzhref"),
        ("person_ref", "acme_personzuref"),
    ] {
        let id = TypeId::new(TypeKind::Entity, label).unwrap();
        assert_eq!(projected.models()[&id].target_name().as_str(), expected);
    }
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    assert_eq!(
        projected.models()[&person]
            .reference_read()
            .target_name()
            .unwrap()
            .as_str(),
        "acme_person_ref"
    );
}

#[test]
fn c_v1_projection_shortens_long_names_deterministically() {
    let first_label = format!("{}a", "z".repeat(254));
    let second_label = format!("{}b", "z".repeat(254));
    let source = format!(
        "format: typebridge.schema/v2\nentities:\n  {first_label}: {{}}\n  {second_label}: {{}}\n"
    );
    let first = c_projection(&source, "acme");
    let second = c_projection(&source, "acme");
    assert_eq!(
        to_canonical_json(&first).unwrap(),
        to_canonical_json(&second).unwrap()
    );

    let first_name = first.models()[&TypeId::new(TypeKind::Entity, first_label).unwrap()]
        .target_name()
        .as_str();
    let second_name = first.models()[&TypeId::new(TypeKind::Entity, second_label).unwrap()]
        .target_name()
        .as_str();
    assert_eq!(first_name.len(), 255);
    assert_eq!(second_name.len(), 255);
    assert!(first_name.starts_with(&format!("acme_{}", "z".repeat(185))));
    assert!(second_name.starts_with(&format!("acme_{}", "z".repeat(185))));
    assert!(
        first_name.ends_with("_8790df4a6c5449b294c77b3743f1b53cd754f2048fd425cca4dbbd579d096eb9"),
        "{first_name}",
    );
    assert!(
        second_name.ends_with("_fe6e75edaee69358e82e1c8e7ab811ababb3cb3d470588b0008b55b7597fc1eb"),
        "{second_name}",
    );
    assert_ne!(first_name, second_name);
    for name in [first_name, second_name] {
        let (_, digest) = name.rsplit_once('_').unwrap();
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
}

fn c_create_field_limit_source(field_count: usize) -> String {
    let mut source = String::from("format: typebridge.schema/v2\nattributes:\n");
    for index in 0..field_count {
        let _ = writeln!(source, "  field-{index:04}: {{ value: string }}");
    }
    source.push_str("entities:\n  record:\n    owns:\n");
    for index in 0..field_count {
        let _ = writeln!(source, "      field-{index:04}: {{}}");
    }
    source
}

fn c_create_role_limit_source(role_count: usize) -> String {
    let mut source = String::from(
        "format: typebridge.schema/v2\nentities:\n  player: {}\nrelations:\n  record:\n    relates:\n",
    );
    for index in 0..role_count {
        let _ = writeln!(source, "      role-{index:04}: {{}}");
    }
    source.push_str("plays:\n  player:\n    record:\n");
    for index in 0..role_count {
        let _ = writeln!(source, "      - role-{index:04}");
    }
    source
}

fn c_create_combined_limit_source(field_count: usize, role_count: usize) -> String {
    let mut source = String::from("format: typebridge.schema/v2\nattributes:\n");
    for index in 0..field_count {
        let _ = writeln!(source, "  field-{index:04}: {{ value: string }}");
    }
    source.push_str("relations:\n  record:\n    owns:\n");
    for index in 0..field_count {
        let _ = writeln!(source, "      field-{index:04}: {{}}");
    }
    source.push_str("    relates:\n");
    for index in 0..role_count {
        let _ = writeln!(source, "      role-{index:04}: {{}}");
    }
    source.push_str("entities:\n  player: {}\nplays:\n  player:\n    record:\n");
    for index in 0..role_count {
        let _ = writeln!(source, "      - role-{index:04}");
    }
    source
}

#[test]
fn c_projection_accepts_1020_create_fields_and_rejects_1021_before_emission() {
    c_projection(
        &c_create_field_limit_source(TYPE_BRIDGE_C_CREATE_FIELD_MAX),
        "acme",
    );
    let error = c_projection_result(
        &c_create_field_limit_source(TYPE_BRIDGE_C_CREATE_FIELD_MAX + 1),
        "acme",
    )
    .expect_err("C projection must reject one create field above its translation ceiling");
    assert_eq!(
        error
            .iter()
            .next()
            .expect("one projection diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "c_projection_create_field_limit_exceeded"
    );
}

#[test]
fn c_projection_accepts_each_create_role_and_combined_ceiling_then_rejects_one_more() {
    c_projection(
        &c_create_role_limit_source(TYPE_BRIDGE_C_CREATE_ROLE_MAX),
        "acme",
    );
    let role_error = c_projection_result(
        &c_create_role_limit_source(TYPE_BRIDGE_C_CREATE_ROLE_MAX + 1),
        "acme",
    )
    .expect_err("C projection must reject one create role above its translation ceiling");
    assert_eq!(
        role_error
            .iter()
            .next()
            .expect("one projection diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "c_projection_create_role_limit_exceeded"
    );

    let fields = TYPE_BRIDGE_C_CREATE_MEMBER_MAX / 2;
    let roles = TYPE_BRIDGE_C_CREATE_MEMBER_MAX - fields;
    c_projection(&c_create_combined_limit_source(fields, roles), "acme");
    let combined_error =
        c_projection_result(&c_create_combined_limit_source(fields + 1, roles), "acme").expect_err(
            "C projection must reject one combined member above its translation ceiling",
        );
    assert_eq!(
        combined_error
            .iter()
            .next()
            .expect("one projection diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "c_projection_create_member_limit_exceeded"
    );
}

#[test]
fn c_projection_collision_checks_global_and_local_namespaces() {
    let global_error = c_projection_result(
        r#"format: typebridge.schema/v2
entities:
  shared: {}
functions:
  shared:
    parameters: []
    returns: { stream: [shared] }
    body: { typeql: "match $shared isa shared; return { $shared };" }
"#,
        "acme",
    )
    .expect_err("a model and function cannot share one emitted C ordinary identifier");
    assert_eq!(
        global_error
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "projection_name_collision"
    );

    let local_error = c_projection_result(
        r#"format: typebridge.schema/v2
attributes:
  participant: { value: string }
relations:
  membership:
    owns: [participant]
    relates: [participant]
"#,
        "acme",
    )
    .expect_err("a field and role cannot share one emitted C member identifier");
    assert_eq!(
        local_error
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "projection_name_collision"
    );
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
fn projects_plain_inherited_abstract_roles_as_child_create_inputs() {
    let source = r#"format: typebridge.schema/v2
entities:
  actor: {}
relations:
  base-event:
    relates:
      participant: { abstract: true, card: 1 }
  plain-event:
    sub: base-event
plays:
  actor:
    base-event: [participant]
"#;
    let base_event = TypeId::new(TypeKind::Relation, "base-event").unwrap();
    let plain_event = TypeId::new(TypeKind::Relation, "plain-event").unwrap();
    let participant = RoleId::new("base-event", "participant").unwrap();

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
        let base = &projected.models()[&base_event];
        assert!(!base.declaration().is_constructible());
        assert!(!base.create().enabled());

        let child = &projected.models()[&plain_event];
        assert!(child.declaration().is_constructible());
        assert!(child.create().enabled());
        assert!(child.create().roles().contains_key(&participant));
        assert!(child.query_tokens().roles()[&participant].is_abstract());
    }
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
