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
    let Some(db) = live_database("tb_plans").await else {
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
    let Some(db) = live_database("tb_plan_references").await else {
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
    // Hydrated from the write's own fetch, equal to a separate read.
    let read = crud
        .get_relation_by_iid(&db, &membership_type, exact.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), exact.fields());
    assert_eq!(read.roles(), exact.roles());
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

const PERSON_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  email: { value: string }
  nickname: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      email: { card: { min: 0, max: 1 } }
      nickname: { card: { min: 0 } }
  employee:
    sub: person
"#;

const PERSON_SCHEMA_TYPEQL: &str = "define
attribute identifier, value string; attribute email, value string;
attribute nickname, value string;
entity person, owns identifier @key, owns email @card(0..1), owns nickname @card(0..);
entity employee sub person;";

fn person_projection() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-person.yaml").unwrap(),
        PERSON_SCHEMA_YAML,
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

fn person_field(name: &str) -> OwnsFactId {
    OwnsFactId::new(
        party_type(TypeKind::Entity, "person"),
        type_bridge_contract::id::AttributeId::new(name).unwrap(),
    )
    .unwrap()
}

fn person(
    installed: &InstalledRuntimeProjection,
    key: &str,
    email: Option<&str>,
) -> ProjectedCreate {
    let mut fields = vec![(
        person_field("identifier"),
        vec![party_text(installed, "identifier", key)],
    )];
    if let Some(email) = email {
        fields.push((
            person_field("email"),
            vec![party_text(installed, "email", email)],
        ));
    }
    ProjectedCreate::try_new(
        installed,
        party_type(TypeKind::Entity, "person"),
        fields,
        vec![],
    )
    .unwrap()
}

fn texts(thing: &type_bridge_orm::ProjectedThing, field: &str) -> Vec<String> {
    thing
        .fields()
        .get(&person_field(field))
        .map(|values| {
            values
                .iter()
                .map(|value| match value.value() {
                    CanonicalValue::String(text) => text.as_str().to_owned(),
                    other => panic!("expected text, got {other:?}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A put is one plan: it creates a new key, replaces every non-key field of
/// an existing one (omitted fields are cleared, as the helper always did),
/// and two puts racing on one new key serialize on its `@key` lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_put_is_one_plan_with_replace_semantics_and_key_races() {
    let Some(db) = live_database("tb_plan_put").await else {
        return;
    };
    db.execute_raw(PERSON_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = person_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");

    // New key: inserted.
    let created = crud
        .put_entity(&db, &person(&installed, "ada", Some("a@example.test")))
        .await
        .unwrap();
    assert_eq!(texts(&created, "email"), ["a@example.test"]);
    db.execute_raw(
        r#"match $p isa person, has identifier "ada"; insert $p has nickname "A", has nickname "Ad";"#,
        TxType::Write,
    )
    .await
    .unwrap();

    // Existing key, different fields: updated in place; the omitted
    // multi-valued nickname is cleared.
    let updated = crud
        .put_entity(&db, &person(&installed, "ada", Some("b@example.test")))
        .await
        .unwrap();
    assert_eq!(updated.iid(), created.iid());
    assert_eq!(texts(&updated, "email"), ["b@example.test"]);
    let read = crud
        .get_entity_by_iid(&db, &person_type, updated.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), updated.fields());
    assert!(texts(&updated, "nickname").is_empty());

    // Partial attribute set (key only): the stored email is cleared, not kept.
    let partial = crud
        .put_entity(&db, &person(&installed, "ada", None))
        .await
        .unwrap();
    assert_eq!(partial.iid(), created.iid());
    assert!(texts(&partial, "email").is_empty());
    assert_eq!(crud.count_entities(&db, &person_type).await.unwrap(), 1);

    // Two puts of one new key in overlapping transactions: the first commit
    // wins, the second is rejected and stores nothing.
    let first = db.transaction_context(TxType::Write).await.unwrap();
    let second = db.transaction_context(TxType::Write).await.unwrap();
    crud.put_entity_in_transaction(&first, &person(&installed, "bob", Some("1@example.test")))
        .await
        .unwrap();
    crud.put_entity_in_transaction(&second, &person(&installed, "bob", Some("2@example.test")))
        .await
        .unwrap();
    first.commit_classified().await.unwrap();
    assert!(second.commit_classified().await.is_err());
    assert_eq!(crud.count_entities(&db, &person_type).await.unwrap(), 2);
    let bob = db
        .execute_raw(
            r#"match $p isa person, has identifier "bob", has email "1@example.test"; select $p;"#,
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&bob), 1);

    // A subtype instance holding the key is not a match for the exact
    // type: the put inserts, collides on the key, and leaves it untouched.
    db.execute_raw(
        r#"insert $e isa employee, has identifier "emp", has email "e@example.test";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    assert!(
        crud.put_entity(&db, &person(&installed, "emp", Some("x@example.test")))
            .await
            .is_err()
    );
    let employee = db
        .execute_raw(
            r#"match $e isa! employee, has identifier "emp", has email "e@example.test"; select $e;"#,
            TxType::Read,
        )
        .await
        .unwrap();
    assert_eq!(rows(&employee), 1);

    db.delete_database().await.unwrap();
    db.close().unwrap();
}

fn person_with(
    installed: &InstalledRuntimeProjection,
    key: &str,
    email: Option<&str>,
    nicknames: &[&str],
) -> ProjectedCreate {
    let mut fields = vec![(
        person_field("identifier"),
        vec![party_text(installed, "identifier", key)],
    )];
    if let Some(email) = email {
        fields.push((
            person_field("email"),
            vec![party_text(installed, "email", email)],
        ));
    }
    fields.push((
        person_field("nickname"),
        nicknames
            .iter()
            .map(|nickname| party_text(installed, "nickname", nickname))
            .collect(),
    ));
    ProjectedCreate::try_new(
        installed,
        party_type(TypeKind::Entity, "person"),
        fields,
        vec![],
    )
    .unwrap()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// Insert, update, put and delete are one plan each, with the helpers'
/// complete-replacement semantics, multi-valued fields and relation roles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_single_writes_are_one_plan_each() {
    let Some(db) = live_database("tb_plan_single").await else {
        return;
    };
    db.execute_raw(PERSON_SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let installed = person_projection();
    let crud = ProjectedCrudExecutor::new(&installed);
    let person_type = party_type(TypeKind::Entity, "person");

    let inserted = crud
        .insert_entity(
            &db,
            &person_with(&installed, "ada", Some("a@x.test"), &["x", "y"]),
        )
        .await
        .unwrap();
    assert_eq!(sorted(texts(&inserted, "nickname")), ["x", "y"]);
    let updated = crud
        .update_entity(
            &db,
            inserted.iid(),
            &person_with(&installed, "ada", Some("b@x.test"), &["z"]),
        )
        .await
        .unwrap();
    assert_eq!(texts(&updated, "email"), ["b@x.test"]);
    assert_eq!(texts(&updated, "nickname"), ["z"]);
    let read = crud
        .get_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), updated.fields());
    // A put giving a multi-valued field replaces its values in one plan.
    let put = crud
        .put_entity(&db, &person_with(&installed, "ada", None, &["p", "q"]))
        .await
        .unwrap();
    assert_eq!(put.iid(), inserted.iid());
    assert_eq!(sorted(texts(&put, "nickname")), ["p", "q"]);
    assert!(texts(&put, "email").is_empty());
    crud.delete_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap();
    assert!(
        crud.get_entity_by_iid(&db, &person_type, inserted.iid())
            .await
            .unwrap()
            .is_none()
    );
    // Deleting an absent thing is not an error.
    crud.delete_entity_by_iid(&db, &person_type, inserted.iid())
        .await
        .unwrap();
    // In a transaction the delete lands at commit; absent is not an error.
    let staged = crud
        .insert_entity(&db, &person_with(&installed, "bob", None, &[]))
        .await
        .unwrap();
    let tx = db.transaction_context(TxType::Write).await.unwrap();
    for _ in 0..2 {
        crud.delete_entity_by_iid_in_transaction(&tx, &person_type, staged.iid())
            .await
            .unwrap();
    }
    assert!(
        crud.get_entity_by_iid(&db, &person_type, staged.iid())
            .await
            .unwrap()
            .is_some()
    );
    tx.commit_classified().await.unwrap();
    assert!(
        crud.get_entity_by_iid(&db, &person_type, staged.iid())
            .await
            .unwrap()
            .is_none()
    );
    db.delete_database().await.unwrap();

    let Some(db) = live_database("tb_plan_single_rel").await else {
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
    let relation = crud
        .insert_relation(
            &db,
            &membership(
                &installed,
                key_reference(&installed, "person", "ada"),
                "first",
            ),
        )
        .await
        .unwrap();
    // Relation update clears the role and attaches the new player.
    let moved = crud
        .update_relation(
            &db,
            relation.iid(),
            &membership(
                &installed,
                key_reference(&installed, "party", "acme"),
                "second",
            ),
        )
        .await
        .unwrap();
    assert_eq!(moved.iid(), relation.iid());
    assert_eq!(moved.roles()[&member].len(), 1);
    assert_eq!(
        moved.roles()[&member][0].type_id(),
        &party_type(TypeKind::Entity, "company")
    );
    let read = crud
        .get_relation_by_iid(&db, &membership_type, relation.iid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.fields(), moved.fields());
    assert_eq!(read.roles(), moved.roles());
    for _ in 0..2 {
        crud.delete_relation_by_iid(&db, &membership_type, relation.iid())
            .await
            .unwrap();
    }
    assert_eq!(
        crud.count_relations(&db, &membership_type).await.unwrap(),
        0
    );
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

const TAGGING_SCHEMA_YAML: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  tagging:
    relates:
      target: { card: { min: 0, max: 2 } }
plays:
  person:
    tagging: [target]
"#;

/// Relation updates with several players stay one document set; an update
/// with no players runs the released path, which refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_relation_updates_with_many_players_and_none() {
    let Some(db) = live_database("tb_plan_rel_update").await else {
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
    let two = |installed: &InstalledRuntimeProjection, note: &str| {
        let mut create = membership(installed, key_reference(installed, "person", "ada"), note);
        create = ProjectedCreate::try_new(
            installed,
            party_type(TypeKind::Relation, "membership"),
            create
                .fields()
                .iter()
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect(),
            vec![(
                member.clone(),
                vec![
                    key_reference(installed, "person", "ada"),
                    key_reference(installed, "party", "acme"),
                ],
            )],
        )
        .unwrap();
        create
    };
    let relation = crud
        .insert_relation(&db, &two(&installed, "a"))
        .await
        .unwrap();
    assert_eq!(relation.roles()[&member].len(), 2);
    // Two old players times two new ones: still one relation document set.
    let updated = crud
        .update_relation(&db, relation.iid(), &two(&installed, "b"))
        .await
        .unwrap();
    assert_eq!(updated.roles()[&member].len(), 2);
    db.delete_database().await.unwrap();

    let Some(db) = live_database("tb_plan_rel_empty").await else {
        return;
    };
    db.execute_raw(
        "define attribute identifier, value string; \
         entity person, owns identifier @key, plays tagging:target; \
         relation tagging, relates target @card(0..2);",
        TxType::Schema,
    )
    .await
    .unwrap();
    db.execute_raw(
        r#"insert $p isa person, has identifier "ada";"#,
        TxType::Write,
    )
    .await
    .unwrap();
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("plan-execute-live-tagging.yaml").unwrap(),
        TAGGING_SCHEMA_YAML,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let crud = ProjectedCrudExecutor::new(&installed);
    let tagging = party_type(TypeKind::Relation, "tagging");
    let target = RoleId::new("tagging", "target").unwrap();
    let ada = ProjectedReference::try_new(
        &installed,
        party_type(TypeKind::Entity, "person"),
        None,
        vec![(
            person_field("identifier"),
            party_text(&installed, "identifier", "ada"),
        )],
    )
    .unwrap();
    let tagged = crud
        .insert_relation(
            &db,
            &ProjectedCreate::try_new(
                &installed,
                tagging.clone(),
                vec![],
                vec![(target, vec![ada])],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let empty = ProjectedCreate::try_new(&installed, tagging.clone(), vec![], vec![]).unwrap();
    // The released update refuses an empty relation; the relation stays.
    let refused = crud
        .update_relation(&db, tagged.iid(), &empty)
        .await
        .unwrap_err();
    assert_eq!(refused.code().as_str(), "provider_operation_failed");
    assert_eq!(crud.count_relations(&db, &tagging).await.unwrap(), 1);
    db.delete_database().await.unwrap();
    db.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_documents_resolve_and_enforce_answer_limits_before_commit() {
    use type_bridge_orm::session::backend::BoundedAnswerLimits;
    let Some(db) = live_database("tb_fetch_documents").await else {
        return;
    };
    db.execute_raw(race::SCHEMA_TYPEQL, TxType::Schema)
        .await
        .unwrap();
    let plan = race::insert_doctor_fetch("alice");
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage_plan(&plan).unwrap();
    tx.commit().await.unwrap();
    let answer = pending.get().unwrap();
    let PlanAnswer::Documents(documents) = answer.as_ref() else {
        panic!("{answer:?}");
    };
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["_type"], "doctor");
    assert_eq!(
        documents[0]["attributes"]["name"],
        serde_json::json!("alice")
    );
    let wire = answer.to_staged_v3(&plan).unwrap();
    assert_eq!(PlanAnswer::from_staged_v3(&wire, &plan).unwrap(), *answer);
    for (max_items, max_bytes, code) in [
        (0, 4096, "processed_item_limit"),
        (10, 1, "response_byte_limit"),
    ] {
        let mut tx = db.transaction(TransactionOptions::default().with_answer_limits(
            BoundedAnswerLimits {
                max_items,
                max_bytes,
                ..Default::default()
            },
        ));
        tx.stage_plan(&race::touch_fetch()).unwrap();
        assert_eq!(
            tx.commit().await.unwrap_err().diagnostic().code().as_str(),
            code
        );
    }
    db.delete_database().await.unwrap();
}
