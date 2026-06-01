use crate::common::rust_binding::*;
use type_bridge_orm::SchemaManager;

#[tokio::test]
async fn schema_sync_registers_expected_types() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    sync_full_schema(&db).await;

    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();
    let registered = schema.schema_info();

    assert!(
        registered.attributes.contains_key("name"),
        "expected registered 'name' attribute"
    );
    assert!(
        registered.attributes.contains_key("age"),
        "expected registered 'age' attribute"
    );
    assert!(
        registered.entities.contains_key("person"),
        "expected registered 'person' entity"
    );
}
