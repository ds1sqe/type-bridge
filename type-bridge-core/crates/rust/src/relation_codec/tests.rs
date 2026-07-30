use super::*;
use crate::__codegen::{EncodedScalar, ValidationPath};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::AttributeValue;
use type_bridge_orm::InstalledRuntimeProjection;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{PythonEmitter, RustEmitter};

fn fixture(target: BindingTarget) -> InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("relation.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  handle: { value: string }
  serial: { value: integer }
entities:
  person: {}
  admin: { sub: person }
  organization: {}
  abstract-player: { abstract: true }
  account:
    owns:
      handle: { key: true }
  premium-account: { sub: account }
  device:
    owns:
      serial: { key: true }
      handle: { key: true }
relations:
  association:
    abstract: true
    owns:
      identifier: { key: true }
    relates:
      member: { card: 1 }
      watcher: { card: { min: 0, max: 2 } }
      unplayed: { card: { min: 0, max: 1 } }
  membership:
    sub: association
  employment:
    sub: association
    relates:
      employee: { as: member, card: 1 }
  event:
    owns:
      identifier: { key: true }
plays:
  person:
    association: [member, watcher]
    employment: [employee]
  organization:
    association: [watcher]
  abstract-player:
    association: [watcher]
  account:
    association: [watcher]
  device:
    association: [watcher]
  event:
    association: [watcher]
"#,
    )])
    .unwrap();
    let declared = normalize_documents(&docs).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    if target == BindingTarget::Python {
        let emitter = PythonEmitter::new();
        let projection = project(
            &resolved,
            target,
            &ProjectionConfig::python(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        return InstalledRuntimeProjection::try_new(projection).unwrap();
    }
    let emitter = RustEmitter::new();
    let projection = project(
        &resolved,
        target,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn canonical(id: &TypeId) -> String {
    String::from_utf8(type_bridge_contract::codec::to_canonical_json(id).unwrap()).unwrap()
}

#[test]
fn relation_authority_returns_exact_relation_identity_and_descriptor() {
    let installed = fixture(BindingTarget::Rust);
    let id = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let json = canonical(&id);
    let (_, expected) = installed
        .relation_descriptor(&id)
        .map(|d| (id.clone(), d.clone()))
        .unwrap();
    for phase in [ModelValidationPhase::Input, ModelValidationPhase::Hydration] {
        let (got, desc) = resolve_relation_authority(&json, &installed, phase, true).unwrap();
        assert_eq!(got, id);
        assert_eq!(desc, expected);
    }
    let root = TypeId::new(TypeKind::Relation, "association").unwrap();
    let expected_root = installed.relation_descriptor(&root).unwrap().clone();
    let (root_id, root_descriptor) = resolve_relation_authority(
        &canonical(&root),
        &installed,
        ModelValidationPhase::Input,
        false,
    )
    .unwrap();
    assert_eq!(root_id, root);
    assert_eq!(root_descriptor, expected_root);
    let (got, desc) = resolve_discovered_relation("employment", &installed).unwrap();
    assert_eq!(got, id);
    assert_eq!(desc, expected);
}

fn assert_model_error(
    result: crate::Result<(TypeId, type_bridge_orm::RelationDescriptor)>,
    phase: ModelValidationPhase,
    code: &str,
) {
    match result {
        Err(crate::Error::ModelValidation {
            phase: got,
            code: actual,
            path,
            ..
        }) => {
            assert_eq!(got, phase);
            assert_eq!(actual, code);
            assert_eq!(path, vec!["type"]);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn relation_authority_rejects_canonical_kind_projection_and_constructibility_failures() {
    let installed = fixture(BindingTarget::Rust);
    let id = TypeId::new(TypeKind::Relation, "employment").unwrap();
    assert_model_error(
        resolve_relation_authority("bad", &installed, ModelValidationPhase::Input, false),
        ModelValidationPhase::Input,
        "invalid_model_identity",
    );
    assert_model_error(
        resolve_relation_authority(
            &format!(" {} ", canonical(&id)),
            &installed,
            ModelValidationPhase::Hydration,
            false,
        ),
        ModelValidationPhase::Hydration,
        "invalid_model_identity",
    );
    let entity = canonical(&TypeId::new(TypeKind::Entity, "person").unwrap());
    assert_model_error(
        resolve_relation_authority(&entity, &installed, ModelValidationPhase::Input, false),
        ModelValidationPhase::Input,
        "wrong_model_kind",
    );
    let python = fixture(BindingTarget::Python);
    assert_model_error(
        resolve_relation_authority(
            &canonical(&id),
            &python,
            ModelValidationPhase::Hydration,
            false,
        ),
        ModelValidationPhase::Hydration,
        "model_not_projected",
    );
    let missing = canonical(&TypeId::new(TypeKind::Relation, "missing").unwrap());
    assert_model_error(
        resolve_relation_authority(&missing, &installed, ModelValidationPhase::Input, false),
        ModelValidationPhase::Input,
        "model_not_projected",
    );
    let root = canonical(&TypeId::new(TypeKind::Relation, "association").unwrap());
    assert_model_error(
        resolve_relation_authority(&root, &installed, ModelValidationPhase::Hydration, true),
        ModelValidationPhase::Hydration,
        "model_not_constructible",
    );
}

#[test]
fn discovered_relation_labels_use_the_same_authority_path() {
    let installed = fixture(BindingTarget::Rust);
    let child = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let expected = installed.relation_descriptor(&child).unwrap().clone();
    let (discovered_id, discovered_descriptor) =
        resolve_discovered_relation("employment", &installed).unwrap();
    assert_eq!(discovered_id, child);
    assert_eq!(discovered_descriptor, expected);
    assert_model_error(
        resolve_discovered_relation("", &installed),
        ModelValidationPhase::Hydration,
        "invalid_discovered_type",
    );
    assert_model_error(
        resolve_discovered_relation("missing", &installed),
        ModelValidationPhase::Hydration,
        "model_not_projected",
    );
    assert_model_error(
        resolve_discovered_relation("association", &installed),
        ModelValidationPhase::Hydration,
        "model_not_constructible",
    );
}

#[test]
fn relation_create_shape_reuses_owned_fields_and_preserves_active_role_order() {
    let installed = fixture(BindingTarget::Rust);
    let ownership =
        r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let reference = |iid: &str| {
        EncodedReference::try_new(
            person,
            Some(iid.to_owned()),
            vec![],
            &ValidationPath::root(),
        )
        .unwrap()
    };
    let role = |owner: &str, name: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        ) as &'static str
    };
    let watcher = role("association", "watcher");
    let member = role("association", "member");
    let encoded = EncodedCreate::new(
        r#"{"kind":"relation","label":"membership"}"#,
        vec![(ownership, vec![EncodedScalar::String("x".into())])],
        vec![
            (watcher, vec![reference("0x1"), reference("0x2")]),
            (member, vec![reference("0x3")]),
        ],
    );
    let out = prepare_relation_create(
        &encoded,
        &TypeId::new(TypeKind::Relation, "membership").unwrap(),
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.attributes,
        vec![("identifier".into(), AttributeValue::String("x".into()))]
    );
    assert_eq!(
        out.role_players,
        vec![
            DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "person".into(),
                iid: Some("0x3".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "person".into(),
                iid: Some("0x1".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "person".into(),
                iid: Some("0x2".into()),
                key: None,
            },
        ]
    );
    let employee = role("employment", "employee");
    let encoded = EncodedCreate::new(
        r#"{"kind":"relation","label":"employment"}"#,
        vec![(ownership, vec![EncodedScalar::String("x".into())])],
        vec![(employee, vec![reference("0x4")])],
    );
    let out = prepare_relation_create(
        &encoded,
        &TypeId::new(TypeKind::Relation, "employment").unwrap(),
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.attributes,
        vec![("identifier".into(), AttributeValue::String("x".into()))]
    );
    assert_eq!(
        out.role_players,
        vec![DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0x4".into()),
            key: None,
        }]
    );
}

#[test]
fn relation_create_shape_rejects_exact_role_evidence_and_cardinality_failures() {
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let field = r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let reference = |iid: &str| {
        EncodedReference::try_new(person, Some(iid.into()), vec![], &ValidationPath::root())
            .unwrap()
    };
    let role = |o: &str, n: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(o, n).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        )
    };
    let member = role("association", "member");
    let watcher = role("association", "watcher");
    let unplayed = role("association", "unplayed");
    let ghost = role("membership", "ghost");
    let member_noncanonical: &'static str = Box::leak(format!(" {member}").into_boxed_str());
    let unplayed_id = type_bridge_contract::id::RoleId::new("association", "unplayed").unwrap();
    let membership_model = installed.projection().models().get(&membership).unwrap();
    assert!(
        membership_model
            .query_tokens()
            .roles()
            .contains_key(&unplayed_id)
    );
    assert!(!membership_model.create().roles().contains_key(&unplayed_id));
    let make = |kind: &'static str, roles: Vec<(&'static str, Vec<EncodedReference>)>| {
        EncodedCreate::new(
            kind,
            vec![(field, vec![EncodedScalar::String("x".into())])],
            roles,
        )
    };
    let cases = vec![
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![(member, vec![reference("0x1")])],
            ),
            employment.clone(),
            "wrong_model_identity",
            vec!["type"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![("bad", vec![reference("0x1")])],
            ),
            membership.clone(),
            "invalid_role_identity",
            vec!["roles[0]"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![(member_noncanonical, vec![reference("0x1")])],
            ),
            membership.clone(),
            "invalid_role_identity",
            vec!["roles[0]"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![(ghost, vec![reference("0x1")])],
            ),
            membership.clone(),
            "unexpected_role_evidence",
            vec!["roles[0]"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![(unplayed, vec![reference("0x1")])],
            ),
            membership.clone(),
            "non_create_role_evidence",
            vec!["roles[0]"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![
                    (member, vec![reference("0x1")]),
                    (member, vec![reference("0x2")]),
                ],
            ),
            membership.clone(),
            "duplicate_role_evidence",
            vec!["roles[1]"],
        ),
        (
            make(r#"{"kind":"relation","label":"membership"}"#, vec![]),
            membership.clone(),
            "missing_role_evidence",
            vec!["member"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![(member, vec![reference("0x1"), reference("0x2")])],
            ),
            membership.clone(),
            "duplicate_role_evidence",
            vec!["member"],
        ),
        (
            make(
                r#"{"kind":"relation","label":"membership"}"#,
                vec![
                    (member, vec![reference("0x1")]),
                    (
                        watcher,
                        vec![reference("0x2"), reference("0x3"), reference("0x4")],
                    ),
                ],
            ),
            membership,
            "role_cardinality_violation",
            vec!["watcher"],
        ),
    ];
    assert_eq!(cases.len(), 9);
    for (encoded, requested, code, path) in cases {
        match prepare_relation_create(&encoded, &requested, &installed) {
            Err(crate::Error::ModelValidation {
                phase,
                code: actual,
                path: actual_path,
                ..
            }) => {
                assert_eq!(phase, ModelValidationPhase::Input);
                assert_eq!(actual, code);
                assert_eq!(actual_path, path);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

#[test]
fn relation_reference_authority_accepts_heterogeneous_and_relation_players() {
    let installed = fixture(BindingTarget::Rust);
    let ownership =
        r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let reference = |type_json: &'static str, iid: &str| {
        EncodedReference::try_new(
            type_json,
            Some(iid.to_owned()),
            vec![],
            &ValidationPath::root(),
        )
        .unwrap()
    };
    let role = |owner: &str, name: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        )
    };
    let member = role("association", "member");
    let watcher = role("association", "watcher");
    let person = r#"{"kind":"entity","label":"person"}"#;
    let organization = r#"{"kind":"entity","label":"organization"}"#;
    let event = r#"{"kind":"relation","label":"event"}"#;
    let encoded = EncodedCreate::new(
        r#"{"kind":"relation","label":"membership"}"#,
        vec![(ownership, vec![EncodedScalar::String("x".into())])],
        vec![
            (member, vec![reference(person, "0x1")]),
            (
                watcher,
                vec![reference(organization, "0x2"), reference(event, "0x3")],
            ),
        ],
    );
    let out = prepare_relation_create(
        &encoded,
        &TypeId::new(TypeKind::Relation, "membership").unwrap(),
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.attributes,
        vec![("identifier".into(), AttributeValue::String("x".into()))]
    );
    assert_eq!(
        out.role_players,
        vec![
            DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "person".into(),
                iid: Some("0x1".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "organization".into(),
                iid: Some("0x2".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "event".into(),
                iid: Some("0x3".into()),
                key: None,
            },
        ]
    );
}

#[test]
fn relation_reference_authority_rejects_exact_player_failures() {
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let field = r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let person_noncanonical: &'static str = Box::leak(format!(" {person} ").into_boxed_str());
    let reference = |type_json: &'static str, iid: &str| {
        EncodedReference::try_new(
            type_json,
            Some(iid.to_owned()),
            vec![],
            &ValidationPath::root(),
        )
        .unwrap()
    };
    let role = |owner: &str, name: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        )
    };
    let member = role("association", "member");
    let watcher = role("association", "watcher");
    let cases: Vec<(&'static str, bool, &str, Vec<&str>)> = vec![
        (
            "bad",
            false,
            "invalid_player_identity",
            vec!["watcher[0]", "type"],
        ),
        (
            person_noncanonical,
            false,
            "invalid_player_identity",
            vec!["watcher[0]", "type"],
        ),
        (
            r#"{"kind":"attribute","label":"identifier"}"#,
            false,
            "wrong_player_kind",
            vec!["watcher[0]", "type"],
        ),
        (
            r#"{"kind":"entity","label":"ghost"}"#,
            false,
            "player_not_projected",
            vec!["watcher[0]", "type"],
        ),
        (
            r#"{"kind":"entity","label":"organization"}"#,
            true,
            "player_not_allowed",
            vec!["member[0]", "type"],
        ),
        (
            r#"{"kind":"entity","label":"abstract-player"}"#,
            false,
            "player_not_constructible",
            vec!["watcher[0]", "type"],
        ),
    ];
    assert_eq!(cases.len(), 6);
    for (player_json, on_member, code, path) in cases {
        let roles = if on_member {
            vec![(member, vec![reference(player_json, "0x9")])]
        } else {
            vec![
                (member, vec![reference(person, "0x1")]),
                (watcher, vec![reference(player_json, "0x9")]),
            ]
        };
        let encoded = EncodedCreate::new(
            r#"{"kind":"relation","label":"membership"}"#,
            vec![(field, vec![EncodedScalar::String("x".into())])],
            roles,
        );
        match prepare_relation_create(&encoded, &membership, &installed) {
            Err(crate::Error::ModelValidation {
                phase,
                code: actual,
                path: actual_path,
                ..
            }) => {
                assert_eq!(phase, ModelValidationPhase::Input);
                assert_eq!(actual, code);
                assert_eq!(actual_path, path);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

const ACCOUNT: &str = r#"{"kind":"entity","label":"account"}"#;
const HANDLE_OWNS: &str = r#"{"attribute":"handle","owner":{"kind":"entity","label":"account"}}"#;

#[test]
fn relation_reference_lowering_prefers_iid_and_lowers_single_key_fallback() {
    let installed = fixture(BindingTarget::Rust);
    let ownership =
        r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let role = |owner: &str, name: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        )
    };
    let member = role("association", "member");
    let watcher = role("association", "watcher");
    let by_key = EncodedReference::try_new(
        ACCOUNT,
        None,
        vec![(HANDLE_OWNS, EncodedScalar::String("h1".into()))],
        &ValidationPath::root(),
    )
    .unwrap();
    let by_iid_with_key = EncodedReference::try_new(
        ACCOUNT,
        Some("0x2".into()),
        vec![(HANDLE_OWNS, EncodedScalar::String("h2".into()))],
        &ValidationPath::root(),
    )
    .unwrap();
    let member_reference =
        EncodedReference::try_new(person, Some("0x1".into()), vec![], &ValidationPath::root())
            .unwrap();
    let encoded = EncodedCreate::new(
        r#"{"kind":"relation","label":"membership"}"#,
        vec![(ownership, vec![EncodedScalar::String("x".into())])],
        vec![
            (member, vec![member_reference]),
            (watcher, vec![by_key, by_iid_with_key]),
        ],
    );
    let out = prepare_relation_create(
        &encoded,
        &TypeId::new(TypeKind::Relation, "membership").unwrap(),
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.attributes,
        vec![("identifier".into(), AttributeValue::String("x".into()))]
    );
    assert_eq!(
        out.role_players,
        vec![
            DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "person".into(),
                iid: Some("0x1".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "account".into(),
                iid: None,
                key: Some(("handle".into(), AttributeValue::String("h1".into()))),
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "account".into(),
                iid: Some("0x2".into()),
                key: None,
            },
        ]
    );
}

#[test]
fn relation_reference_lowering_rejects_exact_identity_failures() {
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let field = r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let role = |owner: &str, name: &str| -> &'static str {
        Box::leak(
            String::from_utf8(
                type_bridge_contract::codec::to_canonical_json(
                    &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .into_boxed_str(),
        )
    };
    let member = role("association", "member");
    let watcher = role("association", "watcher");
    let reference =
        |type_json: &'static str, iid: Option<&str>, keys: Vec<(&'static str, EncodedScalar)>| {
            EncodedReference::try_new(
                type_json,
                iid.map(str::to_owned),
                keys,
                &ValidationPath::root(),
            )
            .unwrap()
        };
    let cases: Vec<(EncodedReference, bool, &str, Vec<&str>)> = vec![
        (
            reference(ACCOUNT, None, vec![(HANDLE_OWNS, EncodedScalar::Long(7))]),
            false,
            "wrong_scalar_domain",
            vec!["watcher[0]", "handle"],
        ),
        (
            reference(
                ACCOUNT,
                None,
                vec![("bad", EncodedScalar::String("h1".into()))],
            ),
            false,
            "unexpected_reference_key",
            vec!["watcher[0]", "keys[0]"],
        ),
        (
            reference(person, Some("not-an-iid"), vec![]),
            false,
            "noncanonical_player_iid",
            vec!["watcher[0]", "iid"],
        ),
        (
            reference(
                person,
                None,
                vec![(HANDLE_OWNS, EncodedScalar::String("x".into()))],
            ),
            true,
            "unexpected_reference_key",
            vec!["member[0]", "keys[0]"],
        ),
        (
            reference(
                ACCOUNT,
                Some("0x5"),
                vec![(HANDLE_OWNS, EncodedScalar::Long(7))],
            ),
            false,
            "wrong_scalar_domain",
            vec!["watcher[0]", "handle"],
        ),
    ];
    assert_eq!(cases.len(), 5);
    for (player, on_member, code, path) in cases {
        let member_reference =
            EncodedReference::try_new(person, Some("0x1".into()), vec![], &ValidationPath::root())
                .unwrap();
        let roles = if on_member {
            vec![(member, vec![player])]
        } else {
            vec![(member, vec![member_reference]), (watcher, vec![player])]
        };
        let encoded = EncodedCreate::new(
            r#"{"kind":"relation","label":"membership"}"#,
            vec![(field, vec![EncodedScalar::String("x".into())])],
            roles,
        );
        match prepare_relation_create(&encoded, &membership, &installed) {
            Err(crate::Error::ModelValidation {
                phase,
                code: actual,
                path: actual_path,
                ..
            }) => {
                assert_eq!(phase, ModelValidationPhase::Input);
                assert_eq!(actual, code);
                assert_eq!(actual_path, path);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

const DEVICE: &str = r#"{"kind":"entity","label":"device"}"#;
const DEVICE_SERIAL_OWNS: &str =
    r#"{"attribute":"serial","owner":{"kind":"entity","label":"device"}}"#;
const DEVICE_HANDLE_OWNS: &str =
    r#"{"attribute":"handle","owner":{"kind":"entity","label":"device"}}"#;
const PREMIUM_ACCOUNT: &str = r#"{"kind":"entity","label":"premium-account"}"#;
const EVENT_IDENTIFIER_OWNS: &str =
    r#"{"attribute":"identifier","owner":{"kind":"relation","label":"event"}}"#;

fn matrix_role(owner: &str, name: &str) -> &'static str {
    Box::leak(
        String::from_utf8(
            type_bridge_contract::codec::to_canonical_json(
                &type_bridge_contract::id::RoleId::new(owner, name).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .into_boxed_str(),
    )
}

fn matrix_create(roles: Vec<(&'static str, Vec<EncodedReference>)>) -> EncodedCreate {
    EncodedCreate::new(
        r#"{"kind":"relation","label":"membership"}"#,
        vec![(
            r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#,
            vec![EncodedScalar::String("x".into())],
        )],
        roles,
    )
}

#[test]
fn relation_reference_matrix_settles_inherited_multi_key_and_relation_key_players() {
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let member = matrix_role("association", "member");
    let watcher = matrix_role("association", "watcher");
    let admin = r#"{"kind":"entity","label":"admin"}"#;
    let person = r#"{"kind":"entity","label":"person"}"#;
    let event = r#"{"kind":"relation","label":"event"}"#;

    let inherited_member =
        EncodedReference::try_new(admin, Some("0xa".into()), vec![], &ValidationPath::root())
            .unwrap();
    let multi_key_with_iid = EncodedReference::try_new(
        DEVICE,
        Some("0xb".into()),
        vec![
            (DEVICE_SERIAL_OWNS, EncodedScalar::Long(7)),
            (DEVICE_HANDLE_OWNS, EncodedScalar::String("d1".into())),
        ],
        &ValidationPath::root(),
    )
    .unwrap();
    let inherited_key = EncodedReference::try_new(
        PREMIUM_ACCOUNT,
        None,
        vec![(HANDLE_OWNS, EncodedScalar::String("p1".into()))],
        &ValidationPath::root(),
    )
    .unwrap();
    let out = prepare_relation_create(
        &matrix_create(vec![
            (member, vec![inherited_member]),
            (watcher, vec![multi_key_with_iid, inherited_key]),
        ]),
        &membership,
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.role_players,
        vec![
            DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "admin".into(),
                iid: Some("0xa".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "device".into(),
                iid: Some("0xb".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "premium-account".into(),
                iid: None,
                key: Some(("handle".into(), AttributeValue::String("p1".into()))),
            },
        ]
    );

    let member_reference =
        EncodedReference::try_new(person, Some("0x1".into()), vec![], &ValidationPath::root())
            .unwrap();
    let serial_only = EncodedReference::try_new(
        DEVICE,
        None,
        vec![(DEVICE_SERIAL_OWNS, EncodedScalar::Long(7))],
        &ValidationPath::root(),
    )
    .unwrap();
    let relation_by_key = EncodedReference::try_new(
        event,
        None,
        vec![(EVENT_IDENTIFIER_OWNS, EncodedScalar::String("e1".into()))],
        &ValidationPath::root(),
    )
    .unwrap();
    let out = prepare_relation_create(
        &matrix_create(vec![
            (member, vec![member_reference]),
            (watcher, vec![serial_only, relation_by_key]),
        ]),
        &membership,
        &installed,
    )
    .unwrap();
    assert_eq!(
        out.role_players,
        vec![
            DynamicRolePlayerInput {
                role_name: "member".into(),
                player_type_name: "person".into(),
                iid: Some("0x1".into()),
                key: None,
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "device".into(),
                iid: None,
                key: Some(("serial".into(), AttributeValue::Long(7))),
            },
            DynamicRolePlayerInput {
                role_name: "watcher".into(),
                player_type_name: "event".into(),
                iid: None,
                key: Some(("identifier".into(), AttributeValue::String("e1".into()))),
            },
        ]
    );
}

#[test]
#[allow(clippy::type_complexity)]
fn relation_reference_matrix_rejects_construction_and_exact_key_failures() {
    let construction_cases: Vec<(
        &'static str,
        Option<&str>,
        Vec<(&'static str, EncodedScalar)>,
        &str,
        &str,
    )> = vec![
        (
            DEVICE,
            None,
            vec![
                (DEVICE_SERIAL_OWNS, EncodedScalar::Long(7)),
                (DEVICE_HANDLE_OWNS, EncodedScalar::String("d1".into())),
            ],
            "multiple_reference_keys_without_iid",
            "",
        ),
        (
            DEVICE,
            Some("0xb"),
            vec![
                (DEVICE_HANDLE_OWNS, EncodedScalar::String("d1".into())),
                (DEVICE_HANDLE_OWNS, EncodedScalar::String("d2".into())),
            ],
            "duplicate_reference_key",
            "keys[1]",
        ),
        (
            r#"{"kind":"entity","label":"person"}"#,
            Some(" "),
            vec![],
            "empty_iid",
            "iid",
        ),
        (
            r#"{"kind":"entity","label":"person"}"#,
            None,
            vec![],
            "missing_reference_identity",
            "",
        ),
    ];
    assert_eq!(construction_cases.len(), 4);
    for (type_json, iid, keys, code, field) in construction_cases {
        let error = EncodedReference::try_new(
            type_json,
            iid.map(str::to_owned),
            keys,
            &ValidationPath::root(),
        )
        .unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
    }

    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let member = matrix_role("association", "member");
    let watcher = matrix_role("association", "watcher");
    let person = r#"{"kind":"entity","label":"person"}"#;
    let wrong_declaring_owner: &'static str =
        r#"{"attribute":"handle","owner":{"kind":"entity","label":"premium-account"}}"#;
    let codec_cases: Vec<(EncodedReference, bool, &str, Vec<&str>)> = vec![
        (
            EncodedReference::try_new(
                DEVICE,
                None,
                vec![(DEVICE_SERIAL_OWNS, EncodedScalar::String("x".into()))],
                &ValidationPath::root(),
            )
            .unwrap(),
            false,
            "wrong_scalar_domain",
            vec!["watcher[0]", "serial"],
        ),
        (
            EncodedReference::try_new(
                PREMIUM_ACCOUNT,
                None,
                vec![(wrong_declaring_owner, EncodedScalar::String("p1".into()))],
                &ValidationPath::root(),
            )
            .unwrap(),
            false,
            "unexpected_reference_key",
            vec!["watcher[0]", "keys[0]"],
        ),
        (
            EncodedReference::try_new(DEVICE, Some("0xb".into()), vec![], &ValidationPath::root())
                .unwrap(),
            true,
            "player_not_allowed",
            vec!["member[0]", "type"],
        ),
    ];
    assert_eq!(codec_cases.len(), 3);
    for (player, on_member, code, path) in codec_cases {
        let member_reference =
            EncodedReference::try_new(person, Some("0x1".into()), vec![], &ValidationPath::root())
                .unwrap();
        let roles = if on_member {
            vec![(member, vec![player])]
        } else {
            vec![(member, vec![member_reference]), (watcher, vec![player])]
        };
        match prepare_relation_create(&matrix_create(roles), &membership, &installed) {
            Err(crate::Error::ModelValidation {
                phase,
                code: actual,
                path: actual_path,
                ..
            }) => {
                assert_eq!(phase, ModelValidationPhase::Input);
                assert_eq!(actual, code);
                assert_eq!(actual_path, path);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

fn provider_player(
    role: &str,
    type_name: Option<&str>,
    iid: Option<&str>,
    attributes: Vec<(&str, serde_json::Value)>,
) -> DynamicRolePlayer {
    DynamicRolePlayer {
        role_name: role.to_owned(),
        player_iid: iid.map(str::to_owned),
        player_type_name: type_name.map(str::to_owned),
        attributes: attributes
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    }
}

#[test]
fn relation_row_hydration_settles_exact_coalesced_evidence() {
    use serde_json::json;
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let row = DynamicRelationRow {
        iid: Some("0x10".into()),
        type_name: Some("membership".into()),
        attributes: vec![("identifier".into(), AttributeValue::String("x".into()))],
        role_players: vec![
            provider_player(
                "watcher",
                Some("device"),
                Some("0xb"),
                vec![("serial", json!(7)), ("handle", json!("d1"))],
            ),
            provider_player("member", Some("person"), Some("0x1"), vec![]),
            provider_player(
                "watcher",
                Some("event"),
                Some("0xe"),
                vec![("identifier", json!("e1"))],
            ),
        ],
    };
    let out = hydrate_relation(row, &membership, &installed).unwrap();
    assert_eq!(out.type_id_json(), canonical(&membership));
    assert_eq!(out.iid(), "0x10");
    assert_eq!(
        out.fields(),
        &[(
            r#"{"attribute":"identifier","owner":{"kind":"relation","label":"association"}}"#
                .to_owned(),
            vec![EncodedScalar::String("x".into())]
        )]
    );
    let member = matrix_role("association", "member");
    let watcher = matrix_role("association", "watcher");
    assert_eq!(
        out.roles(),
        &[
            (
                member.to_owned(),
                vec![crate::__codegen::HydratedPlayer::from_owned(
                    r#"{"kind":"entity","label":"person"}"#.to_owned(),
                    Some("0x1".into()),
                    vec![],
                )]
            ),
            (
                watcher.to_owned(),
                vec![
                    crate::__codegen::HydratedPlayer::from_owned(
                        DEVICE.to_owned(),
                        Some("0xb".into()),
                        vec![
                            (
                                DEVICE_HANDLE_OWNS.to_owned(),
                                EncodedScalar::String("d1".into())
                            ),
                            (DEVICE_SERIAL_OWNS.to_owned(), EncodedScalar::Long(7)),
                        ],
                    ),
                    crate::__codegen::HydratedPlayer::from_owned(
                        r#"{"kind":"relation","label":"event"}"#.to_owned(),
                        Some("0xe".into()),
                        vec![(
                            EVENT_IDENTIFIER_OWNS.to_owned(),
                            EncodedScalar::String("e1".into())
                        )],
                    ),
                ]
            ),
        ]
    );

    let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
    let row = DynamicRelationRow {
        iid: Some("0x11".into()),
        type_name: Some("employment".into()),
        attributes: vec![("identifier".into(), AttributeValue::String("y".into()))],
        role_players: vec![provider_player(
            "employee",
            Some("admin"),
            Some("0xa"),
            vec![],
        )],
    };
    let out = hydrate_relation(row, &employment, &installed).unwrap();
    assert_eq!(out.type_id_json(), canonical(&employment));
    assert_eq!(out.iid(), "0x11");
    let employee = matrix_role("employment", "employee");
    assert_eq!(
        out.roles(),
        &[(
            employee.to_owned(),
            vec![crate::__codegen::HydratedPlayer::from_owned(
                r#"{"kind":"entity","label":"admin"}"#.to_owned(),
                Some("0xa".into()),
                vec![],
            )]
        )]
    );
}

#[test]
fn relation_row_hydration_rejects_exact_row_and_player_failures() {
    use serde_json::json;
    let installed = fixture(BindingTarget::Rust);
    let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
    let member_player = || provider_player("member", Some("person"), Some("0x1"), vec![]);
    let base_row = |role_players: Vec<DynamicRolePlayer>| DynamicRelationRow {
        iid: Some("0x10".into()),
        type_name: Some("membership".into()),
        attributes: vec![("identifier".into(), AttributeValue::String("x".into()))],
        role_players,
    };
    let with_watcher = |watcher: DynamicRolePlayer| base_row(vec![member_player(), watcher]);
    let association = TypeId::new(TypeKind::Relation, "association").unwrap();
    match hydrate_relation(base_row(vec![member_player()]), &association, &installed) {
        Err(crate::Error::ModelValidation {
            phase, code, path, ..
        }) => {
            assert_eq!(phase, ModelValidationPhase::Hydration);
            assert_eq!(code, "model_not_constructible");
            assert!(path.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
    let cases: Vec<(DynamicRelationRow, &str, Vec<&str>)> = vec![
        (
            DynamicRelationRow {
                iid: None,
                ..base_row(vec![member_player()])
            },
            "missing_iid",
            vec!["iid"],
        ),
        (
            DynamicRelationRow {
                iid: Some("xyz".into()),
                ..base_row(vec![member_player()])
            },
            "noncanonical_iid",
            vec!["iid"],
        ),
        (
            DynamicRelationRow {
                type_name: None,
                ..base_row(vec![member_player()])
            },
            "missing_concrete_type",
            vec!["type"],
        ),
        (
            DynamicRelationRow {
                type_name: Some("employment".into()),
                ..base_row(vec![member_player()])
            },
            "wrong_concrete_type",
            vec!["type"],
        ),
        (
            base_row(vec![
                member_player(),
                provider_player("ghost", Some("person"), Some("0x2"), vec![]),
            ]),
            "unexpected_provider_role",
            vec!["role_players[1]"],
        ),
        (base_row(vec![]), "missing_role_evidence", vec!["member"]),
        (
            base_row(vec![member_player(), member_player()]),
            "duplicate_role_evidence",
            vec!["member"],
        ),
        (
            base_row(vec![
                member_player(),
                provider_player("watcher", Some("person"), Some("0x2"), vec![]),
                provider_player("watcher", Some("person"), Some("0x3"), vec![]),
                provider_player("watcher", Some("person"), Some("0x4"), vec![]),
            ]),
            "role_cardinality_violation",
            vec!["watcher"],
        ),
        (
            with_watcher(provider_player("watcher", Some("person"), None, vec![])),
            "missing_player_iid",
            vec!["watcher[0]", "iid"],
        ),
        (
            with_watcher(provider_player(
                "watcher",
                Some("person"),
                Some("bad"),
                vec![],
            )),
            "noncanonical_player_iid",
            vec!["watcher[0]", "iid"],
        ),
        (
            with_watcher(provider_player("watcher", None, Some("0x2"), vec![])),
            "missing_player_type",
            vec!["watcher[0]", "type"],
        ),
        (
            with_watcher(provider_player(
                "watcher",
                Some("abstract-player"),
                Some("0x2"),
                vec![],
            )),
            "player_not_allowed",
            vec!["watcher[0]", "type"],
        ),
        (
            with_watcher(provider_player(
                "watcher",
                Some("device"),
                Some("0xb"),
                vec![("identifier", json!("zz"))],
            )),
            "invalid_player_attributes",
            vec!["watcher[0]", "attributes"],
        ),
        (
            with_watcher(provider_player(
                "watcher",
                Some("device"),
                Some("0xb"),
                vec![("serial", json!("not-a-long"))],
            )),
            "invalid_player_attributes",
            vec!["watcher[0]", "attributes"],
        ),
        (
            with_watcher(provider_player(
                "watcher",
                Some("device"),
                Some("0xb"),
                vec![("serial", json!(7)), ("serial", json!(8))],
            )),
            "duplicate_reference_key",
            vec!["watcher[0]", "serial"],
        ),
    ];
    assert_eq!(cases.len(), 15);
    for (row, code, path) in cases {
        match hydrate_relation(row, &membership, &installed) {
            Err(crate::Error::ModelValidation {
                phase,
                code: actual,
                path: actual_path,
                ..
            }) => {
                assert_eq!(phase, ModelValidationPhase::Hydration);
                assert_eq!(actual, code, "path {actual_path:?}");
                assert_eq!(actual_path, path);
            }
            other => panic!("unexpected result: {other:?} (expected {code})"),
        }
    }
}
