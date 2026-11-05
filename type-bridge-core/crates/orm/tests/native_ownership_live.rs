//! Opt-in ownership plan semantics against a live TypeDB server.
#![cfg(feature = "typedb")]
use std::sync::Arc;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::QueryOperand;
use type_bridge_contract::query_plan::v3::PlanBuilder;
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::session::staged::{PlanAnswer, TransactionOptions};
use type_bridge_orm::{ConnectOptions, Database, TxType};
use type_bridge_schema_compat::typeql_to_declared;

#[tokio::test]
#[ignore = "requires TYPE_BRIDGE_OWNERSHIP_LIVE_DATABASE and TYPEDB_ADDRESS"]
async fn selective_unlink_keeps_other_values_and_shared_attribute() {
    let db = database("").await;
    let result = tokio::spawn({
        let db = Arc::clone(&db);
        async move {
            let schema_text =
                "define attribute score, value integer; entity account, owns score @card(0..);";
            db.execute_raw(schema_text, TxType::Schema).await.unwrap();
            db.execute_raw(
                "insert $a isa account, has score 1, has score 2; $b isa account, has score 1;",
                TxType::Write,
            )
            .await
            .unwrap();
            let schema = typeql_to_declared(
                DocumentId::new("ownership-live.typeql").unwrap(),
                schema_text,
            )
            .unwrap();
            let owner_type = TypeId::new(TypeKind::Entity, "account").unwrap();
            let attribute = AttributeId::new("score").unwrap();
            let mut builder = PlanBuilder::new();
            let owner = builder.exact(owner_type.clone());
            let value = builder.field(owner, attribute.clone());
            let marker = builder.field(owner, attribute.clone());
            builder.compare(
                QueryOperand::Binding { binding: value },
                ValueComparator::Equal,
                QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                },
            );
            builder.compare(
                QueryOperand::Binding { binding: marker },
                ValueComparator::Equal,
                QueryOperand::Literal {
                    value: CanonicalValue::Long(2),
                },
            );
            builder.delete_field_value(
                owner,
                OwnsFactId::new(owner_type, attribute).unwrap(),
                value,
            );
            let plan = builder
                .build(
                    &schema,
                    ManagedSemanticSchemaFingerprint::compute(
                        SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                        b"native-ownership-live",
                    )
                    .unwrap(),
                )
                .unwrap();
            assert_eq!(plan.statement_counts(), (0, 1));
            let mut tx = db.transaction(TransactionOptions::default());
            let answer = tx.stage_plan(&plan).unwrap();
            tx.commit().await.unwrap();
            assert_eq!(*answer.get().unwrap(), PlanAnswer::Matched(1));
            for (query, expected) in [
                ("match $a isa account, has score 2; select $a;", 1),
                ("match $a isa account, has score 1; select $a;", 1),
                (
                    "match $a isa account, has score 1, has score 2; select $a;",
                    0,
                ),
                ("match $v isa score; select $v;", 2),
            ] {
                let QueryResult::Rows(rows) = db.execute_raw(query, TxType::Read).await.unwrap()
                else {
                    panic!("rows");
                };
                assert_eq!(rows.len(), expected, "{query}");
            }
        }
    })
    .await;
    db.delete_database().await.unwrap();
    result.unwrap();
}

async fn database(suffix: &str) -> Arc<Database> {
    let name = format!(
        "{}{suffix}",
        std::env::var("TYPE_BRIDGE_OWNERSHIP_LIVE_DATABASE").unwrap()
    );
    let db = Arc::new(
        Database::connect_with_options(
            &std::env::var("TYPEDB_ADDRESS").unwrap(),
            &name,
            "admin",
            "password",
            ConnectOptions {
                http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
                ..ConnectOptions::default()
            },
        )
        .await
        .unwrap(),
    );
    db.create_database().await.unwrap();
    db
}

#[tokio::test]
#[ignore = "requires TYPE_BRIDGE_OWNERSHIP_LIVE_DATABASE and TYPEDB_ADDRESS"]
async fn explicit_isa_matches_abstract_attributes_owned_only_by_an_owner_subtype() {
    use type_bridge_contract::query_plan::TypeMatchMode;
    let db = database("_mode").await;
    let result = tokio::spawn({ let db = Arc::clone(&db); async move {
        let schema_text = "define attribute original @abstract, value string; attribute specialised, sub original; entity base; entity child, sub base, owns specialised;";
        db.execute_raw(schema_text, TxType::Schema).await.unwrap();
        db.execute_raw("insert $c isa child, has specialised \"value\";", TxType::Write).await.unwrap();
        let schema = typeql_to_declared(DocumentId::new("ownership-mode-live.typeql").unwrap(), schema_text).unwrap();
        for mode in [TypeMatchMode::Isa, TypeMatchMode::IsaExact] {
            let mut builder = PlanBuilder::new();
            let owner = builder.subtypes(TypeId::new(TypeKind::Entity, "base").unwrap());
            let value = builder.field_with_mode(owner, AttributeId::new("original").unwrap(), mode);
            builder.returning(vec![value]);
            let plan = builder.build(&schema, ManagedSemanticSchemaFingerprint::compute(SemanticProfileId::new("typedb-3.13.6/v1").unwrap(), b"ownership-mode-live").unwrap());
            if mode == TypeMatchMode::IsaExact {
                assert_eq!(plan.unwrap_err().code().as_str(), "query_plan_abstract_exact_ownership");
                continue;
            }
            let plan = plan.unwrap();
            assert_eq!(plan.statement_counts(), (1,0));
            let mut tx = db.transaction(TransactionOptions::default());
            let answer = tx.stage_plan(&plan).unwrap();
            tx.commit().await.unwrap();
            let PlanAnswer::Rows(rows) = &*answer.get().unwrap() else { panic!("rows"); };
            assert_eq!(rows.len(), 1);
            let QueryResult::Rows(direct) = db.execute_raw("match $o isa base, has original $v; select $v;", TxType::Read).await.unwrap() else { panic!("direct rows"); };
            assert_eq!(direct.len(), rows.len());
        }
    }}).await;
    db.delete_database().await.unwrap();
    result.unwrap();
}
