//! Execute each representative corpus stage against the real server grammar.
use type_bridge_core_lib::__internal as tql;
use type_bridge_orm::{ConnectOptions, Database, GivenRowsSpec, GivenValue, TxType};

#[tokio::test]
#[ignore = "requires a TypeDB 3.13.6 server"]
async fn official_ast_corpus_data_stages_execute_live() {
    let address = std::env::var("TYPEDB_ADDRESS").expect("TYPEDB_ADDRESS");
    let http_port = std::env::var("TYPEDB_HTTP_PORT")
        .expect("TYPEDB_HTTP_PORT")
        .parse()
        .unwrap();
    let database_name = format!("tb_w11c_ast_corpus_{}", std::process::id());
    let db = Database::connect_with_options(
        &address,
        &database_name,
        "admin",
        "password",
        ConnectOptions {
            http_port,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    let result = async {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/tql_corpus/live");
        let schema = std::fs::read_to_string(root.join("schema.tql")).unwrap();
        let schema = tql::render(&tql::parse(&schema).unwrap()).unwrap();
        db.execute_raw(&schema, TxType::Schema).await?;
        let mut paths = std::fs::read_dir(&root)
            .unwrap()
            .map(|v| v.unwrap().path())
            .filter(|p| p.file_name().unwrap() != "schema.tql")
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 9);
        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            let source = tql::render(&tql::parse(&source).unwrap()).unwrap();
            let name = path.file_name().unwrap().to_str().unwrap();
            let result = if name.contains("given") {
                db.execute_with_rows(
                    &source,
                    TxType::Read,
                    GivenRowsSpec {
                        variables: vec!["input".to_owned()],
                        rows: vec![vec![GivenValue::Integer(1)]],
                    },
                )
                .await
            } else {
                let kind = if ["insert", "put", "update", "delete"]
                    .iter()
                    .any(|v| name.contains(v))
                {
                    TxType::Write
                } else {
                    TxType::Read
                };
                db.execute_raw(&source, kind).await
            };
            if let Err(error) = result {
                eprintln!("corpus stage {name} failed");
                return Err(error);
            }
            eprintln!("corpus live: {name} accepted");
        }
        Ok::<_, type_bridge_orm::OrmError>(())
    }
    .await;
    let cleanup = db.delete_database().await;
    result.unwrap();
    cleanup.unwrap();
}
