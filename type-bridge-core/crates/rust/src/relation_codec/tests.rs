use super::*;
use crate::__codegen::EncodedScalar;
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
    result: crate::Result<(TypeId, type_bridge_orm::_descriptor::RelationDescriptor)>,
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

const DEVICE: &str = r#"{"kind":"entity","label":"device"}"#;

const DEVICE_HANDLE_OWNS: &str =
    r#"{"attribute":"handle","owner":{"kind":"entity","label":"device"}}"#;

const DEVICE_SERIAL_OWNS: &str =
    r#"{"attribute":"serial","owner":{"kind":"entity","label":"device"}}"#;

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
                vec![crate::__codegen::HydratedPlayer::from_complete_row(
                    crate::__codegen::HydratedRow::from_owned(
                        r#"{"kind":"entity","label":"person"}"#.to_owned(),
                        "0x1".into(),
                        vec![],
                        vec![],
                    )
                )]
            ),
            (
                watcher.to_owned(),
                vec![
                    crate::__codegen::HydratedPlayer::from_complete_row_with_keys(
                        crate::__codegen::HydratedRow::from_owned(
                            DEVICE.to_owned(),
                            "0xb".into(),
                            vec![
                                (
                                    DEVICE_HANDLE_OWNS.to_owned(),
                                    vec![EncodedScalar::String("d1".into())]
                                ),
                                (DEVICE_SERIAL_OWNS.to_owned(), vec![EncodedScalar::Long(7)]),
                            ],
                            vec![],
                        ),
                        vec![
                            (
                                DEVICE_HANDLE_OWNS.to_owned(),
                                EncodedScalar::String("d1".into()),
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
            vec![crate::__codegen::HydratedPlayer::from_complete_row(
                crate::__codegen::HydratedRow::from_owned(
                    r#"{"kind":"entity","label":"admin"}"#.to_owned(),
                    "0xa".into(),
                    vec![],
                    vec![],
                )
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
