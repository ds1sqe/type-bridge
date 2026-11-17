use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationKindId, CollectionMode, DocumentId, SchemaFactId, SubFactId,
};
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
fn ordered_collection_mode_and_distinct_survive_plain_inheritance() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article:
    owns:
      tag: { ordered: true, distinct: true }
  review:
    sub: article
relations:
  collection:
    relates:
      member: { ordered: true, distinct: true }
  curated-collection:
    sub: collection
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("ordered schema resolves");
    let tag = AttributeId::new("tag").unwrap();
    let review = TypeId::new(TypeKind::Entity, "review").unwrap();
    let owns = &resolved.types()[&review].owns()[&tag];
    assert_eq!(owns.collection_mode(), CollectionMode::OrderedList);
    assert!(owns.is_distinct());
    assert!(!owns.origin().is_direct());

    let curated = TypeId::new(TypeKind::Relation, "curated-collection").unwrap();
    let member = RoleId::new("collection", "member").unwrap();
    let relates = &resolved.types()[&curated].relates()[&member];
    assert_eq!(relates.collection_mode(), CollectionMode::OrderedList);
    assert!(relates.is_distinct());
    assert!(!relates.origin().is_direct());
}

#[test]
fn inherited_owns_redeclaration_must_preserve_collection_mode() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article:
    owns:
      tag: { ordered: true }
  review:
    sub: article
    owns: [tag]
"#,
    );
    let error = resolve(&schema, &profile()).expect_err("mode conflict rejects");
    let diagnostic = error.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "inherited_owns_collection_mode_conflict"
    );
    assert!(diagnostic.primary().is_some());
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn specialized_role_must_preserve_effective_parent_collection_mode() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  collection:
    relates:
      member: { ordered: true }
  curated-collection:
    sub: collection
    relates:
      curated-member: { as: member }
"#,
    );
    let error = resolve(&schema, &profile()).expect_err("mode conflict rejects");
    let diagnostic = error.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "specialized_role_collection_mode_conflict"
    );
    assert!(diagnostic.primary().is_some());
    assert_eq!(diagnostic.related().len(), 1);
}

#[test]
fn direct_redeclarations_and_specializations_may_add_or_drop_distinct() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  base:
    owns:
      tag: { ordered: true, distinct: true }
  dropped:
    sub: base
    owns:
      tag: { ordered: true }
  added-base:
    owns:
      tag: { ordered: true }
  added:
    sub: added-base
    owns:
      tag: { ordered: true, distinct: true }
relations:
  distinct-roles:
    relates:
      member: { ordered: true, distinct: true }
  dropped-role:
    sub: distinct-roles
    relates:
      child: { as: member, ordered: true }
  plain-roles:
    relates:
      member: { ordered: true }
  added-role:
    sub: plain-roles
    relates:
      child: { as: member, ordered: true, distinct: true }
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("distinct transitions resolve");
    let tag = AttributeId::new("tag").unwrap();
    for (owner, distinct) in [("dropped", false), ("added", true)] {
        let id = TypeId::new(TypeKind::Entity, owner).unwrap();
        let owns = &resolved.types()[&id].owns()[&tag];
        assert_eq!(owns.collection_mode(), CollectionMode::OrderedList);
        assert_eq!(owns.is_distinct(), distinct);
    }
    for (relation, declaring_relation, distinct) in [
        ("dropped-role", "dropped-role", false),
        ("added-role", "added-role", true),
    ] {
        let id = TypeId::new(TypeKind::Relation, relation).unwrap();
        let role = RoleId::new(declaring_relation, "child").unwrap();
        let relates = &resolved.types()[&id].relates()[&role];
        assert_eq!(relates.collection_mode(), CollectionMode::OrderedList);
        assert_eq!(relates.is_distinct(), distinct);
    }
}

#[test]
fn ordered_collections_require_a_retained_semantic_profile() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article:
    owns:
      tag: { ordered: true }
"#,
    );
    resolve(&schema, &profile()).expect("3.12.1 accepts ordered collections");

    let legacy = SemanticProfileId::new(format!("typedb-3.{}.5/v1", 12 - 1)).unwrap();
    let error = resolve(&schema, &legacy).expect_err("retired profiles reject schema resolution");
    let diagnostic = error.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "unsupported_semantic_profile"
    );
    assert!(diagnostic.primary().is_none());

    let unordered = declared(
        r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  article: { owns: [tag] }
"#,
    );
    let error =
        resolve(&unordered, &legacy).expect_err("retired profiles reject unordered schemas too");
    assert_eq!(
        error.iter().next().unwrap().diagnostic().code().as_str(),
        "unsupported_semantic_profile"
    );
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
fn resolves_exact_direct_sub_origin_and_independent_edge_annotations() {
    let schema = declared(
        r#"format: typebridge.schema/v2
entities:
  actor: {}
  person:
    doc: "person type"
    sub:
      type: actor
      doc: "person edge"
      meta: { owner: "schema", stability: "stable" }
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let actor = TypeId::new(TypeKind::Entity, "actor").expect("type identifier is valid");
    let person = TypeId::new(TypeKind::Entity, "person").expect("type identifier is valid");
    let expected = SubFactId::new(person.clone(), actor).expect("sub identifier is valid");
    let person = &resolved.types()[&person];
    let direct_sub = person.direct_sub().expect("person retains its direct edge");

    assert_eq!(direct_sub.id(), &expected);
    assert_eq!(direct_sub.origin().declared(), &SchemaFactId::Sub(expected));
    assert!(direct_sub.origin().is_direct());
    assert_eq!(direct_sub.annotations().len(), 3);
    assert!(
        direct_sub
            .annotations()
            .contains_key(&AnnotationKindId::Doc)
    );
    assert!(
        direct_sub
            .annotations()
            .contains_key(&AnnotationKindId::Meta(
                type_bridge_contract::id::Label::new("owner").unwrap()
            ))
    );
    assert!(
        direct_sub
            .annotations()
            .contains_key(&AnnotationKindId::Meta(
                type_bridge_contract::id::Label::new("stability").unwrap()
            ))
    );
    assert_eq!(person.annotations().len(), 1);
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
fn inherited_abstract_role_is_playable_only_below_its_declaring_scope() {
    let schema = declared(
        r#"format: typebridge.schema/v2
entities:
  actor: {}
relations:
  base-event:
    relates:
      participant: { abstract: true, card: 1 }
  plain-event:
    sub: base-event
  specialized-event:
    sub: base-event
    relates:
      actor: { as: participant, card: 1 }
plays:
  actor:
    base-event: [participant]
    specialized-event: [actor]
"#,
    );
    let resolved = resolve(&schema, &profile()).expect("schema resolves");
    let base_event = TypeId::new(TypeKind::Relation, "base-event").unwrap();
    let plain_event = TypeId::new(TypeKind::Relation, "plain-event").unwrap();
    let specialized_event = TypeId::new(TypeKind::Relation, "specialized-event").unwrap();
    let participant = RoleId::new("base-event", "participant").unwrap();

    let declared_role = &resolved.types()[&base_event].relates()[&participant];
    assert!(declared_role.is_abstract());
    assert!(declared_role.origin().is_direct());
    assert!(!declared_role.accepts_players_at_effective_scope());
    assert!(!resolved.types()[&base_event].is_constructible());

    let inherited_role = &resolved.types()[&plain_event].relates()[&participant];
    assert!(inherited_role.is_abstract());
    assert!(!inherited_role.origin().is_direct());
    assert!(inherited_role.accepts_players_at_effective_scope());
    assert!(resolved.types()[&plain_event].is_constructible());
    assert!(resolved.types()[&specialized_event].is_constructible());
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
  cycle-alpha: { sub: cycle-beta }
  cycle-beta: { sub: cycle-alpha }
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

#[test]
fn backslash_values_are_refused_before_provider_writes() {
    let schema = declared(
        r#"format: typebridge.schema/v2
attributes:
  path:
    value:
      type: string
      values: ['back\slash']
"#,
    );
    let error = resolve(&schema, &profile()).unwrap_err();
    let diagnostic = error.iter().next().unwrap();
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "unsupported_values_backslash"
    );
    assert!(diagnostic.primary().is_some());
}

#[test]
fn specializing_roles_intersect_local_and_parent_cardinality_constraints() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  base:
    abstract: true
    relates:
      source: {card: 1}
  child:
    sub: base
    relates:
      origin: {as: source}
"#,
    );
    let resolved = resolve(&schema, &profile()).unwrap();
    let child = TypeId::new(TypeKind::Relation, "child").unwrap();
    let origin = RoleId::new("child", "origin").unwrap();
    assert_eq!(
        resolved.types()[&child].relates()[&origin].cardinality(),
        type_bridge_contract::value::Cardinality::new(1, Some(1)).unwrap()
    );
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  base:
    relates:
      source: {card: {min: 1, max: 3}}
  child:
    sub: base
    relates:
      origin: {as: source, card: {min: 0, max: 2}}
"#,
    );
    let resolved = resolve(&schema, &profile()).unwrap();
    assert_eq!(
        resolved.types()[&child].relates()[&origin].cardinality(),
        type_bridge_contract::value::Cardinality::new(1, Some(2)).unwrap()
    );
}

#[test]
fn contradictory_specialization_cardinality_is_source_diagnosed() {
    let schema = declared(
        r#"format: typebridge.schema/v2
relations:
  base:
    relates:
      source: {card: {min: 2, max: 3}}
  child:
    sub: base
    relates:
      origin: {as: source}
"#,
    );
    let diagnostics = resolve(&schema, &profile()).unwrap_err();
    let diagnostic = diagnostics.iter().next().unwrap();
    assert_eq!(
        diagnostic.diagnostic().code().as_str(),
        "incompatible_specialization_cardinality"
    );
    assert!(diagnostic.primary().is_some());
}

#[test]
fn owns_constraints_must_narrow_direct_and_inherited_attribute_constraints() {
    for (attribute, own) in [
        (
            "range: {min: -9223372036854775808, max: 9223372036854775807}",
            "range: {min: 0}",
        ),
        ("range: {min: 0, max: 100}", "range: {min: 0, max: 100}"),
        ("range: {min: 0, max: 100}", "range: {min: -1, max: 90}"),
        ("values: [1, 2]", "values: [1, 2, 3]"),
        ("values: [1, 2]", "values: [1, 2]"),
    ] {
        for inherited in [false, true] {
            let child = if inherited {
                "  local-score: {sub: score}\n"
            } else {
                ""
            };
            let label = if inherited { "local-score" } else { "score" };
            let schema = declared(&format!(
                "format: typebridge.schema/v2\nattributes:\n  score:\n    value: {{type: integer, {attribute}}}\n{child}entities:\n  person:\n    owns:\n      {label}: {{{own}}}\n"
            ));
            let error = resolve(&schema, &profile()).unwrap_err();
            let diagnostic = error.iter().next().unwrap();
            assert_eq!(
                diagnostic.diagnostic().code().as_str(),
                "owns_constraint_not_narrowing"
            );
            assert!(diagnostic.primary().is_some());
            assert_eq!(diagnostic.related().len(), 1);
        }
    }
}

#[test]
fn owns_value_narrowing_compares_numeric_values_but_preserves_annotation_identity() {
    // TypeDB accepts a distinct annotation spelling of signed zero while
    // still matching the same numeric value; duplicate numeric members fail.
    for values in ["[0.0]", "[0.0, 1.0]"] {
        let schema = declared(&format!(
            "format: typebridge.schema/v2\nattributes:\n  score:\n    value: {{type: double, values: [-0.0, 1.0]}}\nentities:\n  person:\n    owns:\n      score: {{values: {values}}}\n"
        ));
        resolve(&schema, &profile()).unwrap();
    }
    let schema = declared(
        "format: typebridge.schema/v2\nattributes:\n  score:\n    value: {type: double, values: [-0.0, 1.0]}\nentities:\n  person:\n    owns:\n      score: {values: [-0.0, 0.0]}\n",
    );
    assert_eq!(
        resolve(&schema, &profile())
            .unwrap_err()
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "owns_constraint_not_narrowing"
    );
}

#[test]
fn owns_constraints_accept_strict_subsets_and_distinct_conjunctive_regexes() {
    for (value_type, attribute, own) in [
        (
            "integer",
            "range: {min: 0, max: 100}",
            "range: {min: 1, max: 99}",
        ),
        ("integer", "range: {min: 0}", "range: {min: 1}"),
        ("integer", "values: [1, 2]", "values: [1]"),
        ("string", "regex: '^ABC$'", "regex: '.*'"),
    ] {
        let schema = declared(&format!(
            "format: typebridge.schema/v2\nattributes:\n  score:\n    value: {{type: {value_type}, {attribute}}}\nentities:\n  person:\n    owns:\n      score: {{{own}}}\n"
        ));
        resolve(&schema, &profile()).unwrap();
    }
    let schema = declared(
        "format: typebridge.schema/v2\nattributes:\n  code: {value: {type: string, regex: '^ABC$'}}\nentities:\n  person:\n    owns:\n      code: {regex: '^ABC$'}\n",
    );
    assert_eq!(
        resolve(&schema, &profile())
            .unwrap_err()
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "owns_constraint_not_narrowing"
    );
}

#[test]
fn odd_backslash_before_regex_quote_refuses_lossy_provider_export() {
    for pattern in [r#"a\"b"#, r#"a\\\"b"#] {
        let schema = declared(&format!(
            "format: typebridge.schema/v2\nattributes:\n  code: {{value: {{type: string, regex: '{pattern}'}}}}\n"
        ));
        let error = resolve(&schema, &profile()).unwrap_err();
        let diagnostic = error.iter().next().unwrap();
        assert_eq!(
            diagnostic.diagnostic().code().as_str(),
            "unsupported_regex_escaped_quote"
        );
        assert!(
            diagnostic
                .diagnostic()
                .to_string()
                .contains("redundant backslash")
        );
        assert!(diagnostic.primary().is_some());
    }
    for pattern in [r#"a"b"#, r#"a\\"b"#] {
        let schema = declared(&format!(
            "format: typebridge.schema/v2\nattributes:\n  code: {{value: {{type: string, regex: '{pattern}'}}}}\n"
        ));
        resolve(&schema, &profile()).unwrap();
    }
}

#[test]
fn equal_owns_value_sets_explain_redundancy_independent_of_order() {
    for values in ["[a, b]", "[b, a]"] {
        let schema = declared(&format!(
            "format: typebridge.schema/v2\nattributes:\n  code: {{value: {{type: string, values: [a, b]}}}}\nentities:\n  item:\n    owns:\n      code: {{values: {values}}}\n"
        ));
        let error = resolve(&schema, &profile()).unwrap_err();
        let diagnostic = error.iter().next().unwrap().diagnostic();
        assert_eq!(diagnostic.code().as_str(), "owns_constraint_not_narrowing");
        assert!(diagnostic.message().contains("equal"));
        assert!(diagnostic.message().contains("redundant"));
        assert!(diagnostic.message().contains("remove"));
    }
}
