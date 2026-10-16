//! Opt-in proof that `query-plan/v3` programs execute staged against TypeDB 3.13.6.
#![cfg(feature = "typedb")]
#[path = "support/plan_v3_race.rs"]
mod race;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::query_plan::v3::Operation;
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::session::staged::{PlanAnswer, StagedQuery, TransactionOptions};
use type_bridge_orm::{
    ConnectOptions, Database, InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedCreate,
    ProjectedCrudExecutor, ProjectedReference, TxType,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

fn rows(answer: &QueryResult) -> usize {
    match answer {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    }
}

/// Connect to the opt-in live server with a fresh database name, or `None`
/// when the live address is unset or the server cannot carry `given` plans.
async fn live_database(stem: &str) -> Option<Database> {
    let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").ok()?;
    let name = format!(
        "{stem}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let db = Database::connect_with_options(
        &address,
        &name,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".into())
                .parse()
                .expect("valid TypeDB HTTP port"),
            tls: false,
            server_version: Some(
                std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_VERSION")
                    .unwrap_or_else(|_| "3.13.6".into())
                    .parse()
                    .unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    // Plan inputs travel as `given` rows (TypeDB 3.12+ over band 9).
    if !db.supports_given_stage() {
        println!("skipped: TypeDB at {address} cannot carry the given stage plans need");
        return None;
    }
    db.create_database().await.unwrap();
    Some(db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_query_purchase_and_write_plans_resolve_at_commit() {
    let Some(db) = live_database("tb_w11e_plans").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $i isa item, has item-id "I1", has stock 1, has price 40;
           $a isa account, has account-id "A1", has balance 100;
           $d isa doctor, has name "alice", has on-call true;"#,
        TxType::Write,
    )
    .await
    .unwrap();

    let mut tx = db.transaction(TransactionOptions::default());
    let bought = tx.stage_plan(&race::purchase("O1", "I1", "A1")).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(*bought.get().unwrap(), PlanAnswer::Matched(1));

    // Stock is now 0: the same pipeline matches no row and writes nothing.
    let refused = tx.stage_plan(&race::purchase("O2", "I1", "A1")).unwrap();
    let orders = tx
        .stage(StagedQuery::new("match $p isa purchase; select $p;", false))
        .unwrap();
    let stock = tx
        .stage(StagedQuery::new(
            "match $i isa item, has stock 0; $a isa account, has balance 60; select $i, $a;",
            false,
        ))
        .unwrap();
    let stocked = tx.stage_plan(&race::stock_read(Operation::Rows)).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(*refused.get().unwrap(), PlanAnswer::Matched(0));
    assert_eq!(rows(&orders.get().unwrap()), 1);
    assert_eq!(rows(&stock.get().unwrap()), 1);
    assert!(matches!(&*stocked.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));

    let touched = tx.stage_plan(&race::touch_rows()).unwrap();
    let touch = tx.stage_plan(&race::touch()).unwrap();
    let dropped = tx.stage_plan(&race::drop_price()).unwrap();
    let exists = tx.stage_plan(&race::stock_read(Operation::Exists)).unwrap();
    let count = tx.stage_plan(&race::stock_read(Operation::Count)).unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(&*touched.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));
    assert_eq!(*touch.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*dropped.get().unwrap(), PlanAnswer::Matched(1));
    assert_eq!(*exists.get().unwrap(), PlanAnswer::Exists(true));
    assert_eq!(*count.get().unwrap(), PlanAnswer::Count(1));
    let prices = db
        .execute_raw("match $i isa item, has price $p; select $i;", TxType::Read)
        .await
        .unwrap();
    assert_eq!(rows(&prices), 0);
    let on_call = db
        .execute_raw(
            "match $d isa doctor, has on-call true; select $d;",
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&on_call), 1);

    db.delete_database().await.unwrap();
    db.close().unwrap();
}

const PARTY_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  note: { value: string }
entities:
  party:
    abstract: true
    owns:
      identifier: { key: true }
  person:
    sub: party
  company:
    sub: party
relations:
  membership:
    owns:
      note: { card: { min: 0, max: 1 } }
    relates:
      member: { card: { min: 1, max: 2 } }
plays:
  party:
    membership: [member]
"#;

const PARTY_SCHEMA_TYPEQL: &str = "define
attribute identifier, value string; attribute note, value string;
entity party @abstract, owns identifier @key, plays membership:member;
entity person sub party; entity company sub party;
relation membership, relates member @card(1..2), owns note @card(0..1);";

fn party_projection() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-party.yaml").unwrap(),
        PARTY_SCHEMA_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

fn party_type(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn party_text(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: &str,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        party_type(TypeKind::Attribute, attribute),
        CanonicalValue::String(CanonicalString::new(value).unwrap()),
    )
    .unwrap()
}

fn membership(
    installed: &InstalledRuntimeProjection,
    reference: ProjectedReference,
    note: &str,
) -> ProjectedCreate {
    let membership = party_type(TypeKind::Relation, "membership");
    let note_field = OwnsFactId::new(
        membership.clone(),
        type_bridge_contract::id::AttributeId::new("note").unwrap(),
    )
    .unwrap();
    ProjectedCreate::try_new(
        installed,
        membership,
        vec![(note_field, vec![party_text(installed, "note", note)])],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
    )
    .unwrap()
}

fn key_reference(
    installed: &InstalledRuntimeProjection,
    label: &str,
    key: &str,
) -> ProjectedReference {
    let referenced = party_type(TypeKind::Entity, label);
    let party = party_type(TypeKind::Entity, "party");
    let field = OwnsFactId::new(
        party,
        type_bridge_contract::id::AttributeId::new("identifier").unwrap(),
    )
    .unwrap();
    let field = installed.projection().models()[&referenced]
        .query_tokens()
        .fields()
        .values()
        .find(|token| token.declaring_id() == &field)
        .map(|token| token.id().clone())
        .unwrap();
    ProjectedReference::try_new(
        installed,
        referenced,
        None,
        vec![(field, party_text(installed, "identifier", key))],
    )
    .unwrap()
}

/// Relation inserts resolve exact, polymorphic and IID references inside the
/// write's own pipeline; a missing reference writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_relation_inserts_resolve_references_inside_the_write() {
    let Some(db) = live_database("tb_w11f_references").await else {
        return;
    };
    db.execute_raw(PARTY_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada"; $c isa company, has identifier "acme";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let installed = party_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let member = RoleId::new("membership", "member").unwrap();
    let membership_type = party_type(TypeKind::Relation, "membership");

    // Exact: one candidate type.
    let exact = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "person", "ada"),
                "exact",
            ),
        )
        .await
        .unwrap();
    let ada = &exact.roles()[&member][0];
    assert_eq!(ada.type_id(), &party_type(TypeKind::Entity, "person"));

    // Polymorphic: the abstract `party` resolves through a disjunction.
    let polymorphic = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "party", "acme"),
                "polymorphic",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        polymorphic.roles()[&member][0].type_id(),
        &party_type(TypeKind::Entity, "company")
    );

    // By IID, through the abstract type as well.
    let by_iid = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "party"),
        Some(ada.iid().to_owned()),
        vec![],
    )
    .unwrap();
    let again = crud
        .insert_relation(&db, &membership(&installed, by_iid, "iid"))
        .await
        .unwrap();
    assert_eq!(again.roles()[&member][0].iid(), ada.iid());

    // Two references of one role resolving to one player: refused after
    // the write and rolled back, as the released resolver refused it.
    let party_iid = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "party"),
        Some(ada.iid().to_owned()),
        vec![],
    )
    .unwrap();
    let doubled = ProjectedCreate::try_new(
        &installed,
        membership_type.clone(),
        vec![],
        vec![(
            member.clone(),
            vec![party_iid, key_reference(&installed, "party", "ada")],
        )],
    )
    .unwrap();
    let duplicate = crud.insert_relation(&db, &doubled).await.unwrap_err();
    assert_eq!(duplicate.code().as_str(), "duplicate_resolved_role_player");

    // Missing: the write matches no row, so nothing is stored.
    let missing = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "party", "nobody"),
                "missing",
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(
        missing.code().as_str(),
        "provider_identity_cardinality_invalid"
    );
    assert_eq!(
        crud.count_relations(&db, &membership_type).await.unwrap(),
        3
    );

    db.delete_database().await.unwrap();
    db.close().unwrap();
}
