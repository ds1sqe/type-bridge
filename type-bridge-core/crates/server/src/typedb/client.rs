use typedb_driver::concept::Concept;
use typedb_driver::TransactionType;

use super::backend::{DriverBackend, QueryResultKind};
use super::real_driver::RealTypeDBBackend;
use crate::config::TypeDBSection;
use crate::error::PipelineError;
use crate::executor::QueryExecutor;

/// Wrapper around the TypeDB Rust driver providing a clean async API
/// for query execution and schema retrieval.
pub struct TypeDBClient {
    backend: Box<dyn DriverBackend>,
}

impl TypeDBClient {
    /// Connect to a TypeDB server using the provided configuration.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn connect(config: &TypeDBSection) -> Result<Self, PipelineError> {
        let backend = RealTypeDBBackend::connect(config).await?;
        Ok(Self {
            backend: Box::new(backend),
        })
    }

    /// Create a TypeDBClient with a custom backend (for testing).
    #[cfg(test)]
    pub(crate) fn with_backend(backend: Box<dyn DriverBackend>) -> Self {
        Self { backend }
    }

    /// Execute a TypeQL query and return results as JSON.
    ///
    /// For read transactions, the transaction is used directly.
    /// For write and schema transactions, the transaction is committed after execution.
    pub async fn execute(
        &self,
        database: &str,
        typeql: &str,
        tx_type: &str,
    ) -> Result<serde_json::Value, PipelineError> {
        let transaction_type = parse_transaction_type(tx_type)?;

        let mut tx = self
            .backend
            .open_transaction(database, transaction_type)
            .await?;

        let answer = tx.query(typeql).await?;

        let needs_commit =
            matches!(transaction_type, TransactionType::Write | TransactionType::Schema);

        let results = match answer {
            QueryResultKind::Ok => {
                if needs_commit {
                    tx.commit().await?;
                }
                serde_json::json!({ "ok": true })
            }
            QueryResultKind::Rows(rows) => {
                if needs_commit {
                    tx.commit().await?;
                }
                serde_json::Value::Array(rows)
            }
            QueryResultKind::Documents(docs) => {
                if needs_commit {
                    let _ = tx.commit().await;
                }
                serde_json::Value::Array(docs)
            }
        };

        Ok(results)
    }

    /// Check if the driver connection is open.
    pub fn is_connected(&self) -> bool {
        self.backend.is_open()
    }
}

impl QueryExecutor for TypeDBClient {
    fn execute<'a>(
        &'a self,
        database: &'a str,
        typeql: &'a str,
        transaction_type: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<serde_json::Value, PipelineError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { self.execute(database, typeql, transaction_type).await })
    }

    fn is_connected(&self) -> bool {
        self.is_connected()
    }
}

/// Parse a transaction type string into a TypeDB TransactionType.
pub(crate) fn parse_transaction_type(tx_type: &str) -> Result<TransactionType, PipelineError> {
    match tx_type {
        "read" => Ok(TransactionType::Read),
        "write" => Ok(TransactionType::Write),
        "schema" => Ok(TransactionType::Schema),
        other => Err(PipelineError::QueryExecution(format!(
            "Unknown transaction type: {other}"
        ))),
    }
}

/// Convert a TypeDB Concept to a serde_json::Value.
pub(crate) fn concept_to_json(concept: &Concept) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    obj.insert(
        "category".to_string(),
        serde_json::Value::String(concept.get_category().name().to_string()),
    );
    obj.insert(
        "label".to_string(),
        serde_json::Value::String(concept.get_label().to_string()),
    );

    if let Some(iid) = concept.try_get_iid() {
        obj.insert(
            "iid".to_string(),
            serde_json::Value::String(iid.to_string()),
        );
    }

    if let Some(value) = concept.try_get_value() {
        obj.insert("value".to_string(), value_to_json(value));
    }

    if let Some(value_type) = concept.try_get_value_type() {
        obj.insert(
            "value_type".to_string(),
            serde_json::Value::String(value_type.name().to_string()),
        );
    }

    serde_json::Value::Object(obj)
}

/// Convert a TypeDB Value to a serde_json::Value.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn value_to_json(value: &typedb_driver::concept::Value) -> serde_json::Value {
    if let Some(b) = value.get_boolean() {
        return serde_json::Value::Bool(b);
    }
    if let Some(i) = value.get_integer() {
        return serde_json::json!(i);
    }
    if let Some(d) = value.get_double() {
        return serde_json::json!(d);
    }
    if let Some(s) = value.get_string() {
        return serde_json::Value::String(s.to_string());
    }
    if let Some(date) = value.get_date() {
        return serde_json::Value::String(date.to_string());
    }
    if let Some(dt) = value.get_datetime() {
        return serde_json::Value::String(dt.to_string());
    }
    if let Some(dt_tz) = value.get_datetime_tz() {
        return serde_json::Value::String(dt_tz.to_string());
    }
    if let Some(dec) = value.get_decimal() {
        return serde_json::Value::String(dec.to_string());
    }
    if let Some(dur) = value.get_duration() {
        return serde_json::Value::String(dur.to_string());
    }
    // Fallback: use Display representation
    value_to_json_fallback(value)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn value_to_json_fallback(value: &typedb_driver::concept::Value) -> serde_json::Value {
    serde_json::Value::String(value.to_string())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use typedb_driver::IID;
    use typedb_driver::concept::{Attribute, AttributeType, Concept, Entity, EntityType, Value, ValueType};
    use typedb_driver::concept::value::{Decimal, Duration, TimeZone};
    use typedb_driver::TransactionType;

    use super::*;
    use crate::error::PipelineError;
    use crate::typedb::backend::TransactionOps;

    // =============================================
    // Mock infrastructure
    // =============================================

    struct MockTransaction {
        query_result: Option<QueryResultKind>,
        query_error: Option<String>,
        commit_error: Option<String>,
        committed: Arc<AtomicBool>,
        query_called: Arc<AtomicBool>,
    }

    impl MockTransaction {
        fn new(result: QueryResultKind) -> Self {
            Self {
                query_result: Some(result),
                query_error: None,
                commit_error: None,
                committed: Arc::new(AtomicBool::new(false)),
                query_called: Arc::new(AtomicBool::new(false)),
            }
        }

        fn failing_query(msg: &str) -> Self {
            Self {
                query_result: None,
                query_error: Some(msg.to_string()),
                commit_error: None,
                committed: Arc::new(AtomicBool::new(false)),
                query_called: Arc::new(AtomicBool::new(false)),
            }
        }

        fn with_commit_error(mut self, msg: &str) -> Self {
            self.commit_error = Some(msg.to_string());
            self
        }
    }

    impl TransactionOps for MockTransaction {
        fn query(
            &mut self,
            _typeql: &str,
        ) -> Pin<Box<dyn Future<Output = Result<QueryResultKind, PipelineError>> + Send + '_>>
        {
            self.query_called.store(true, Ordering::SeqCst);
            let result = self.query_result.take();
            let error = self.query_error.take();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::QueryExecution(msg));
                }
                Ok(result.expect("MockTransaction::query called more than once"))
            })
        }

        fn commit(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>> {
            self.committed.store(true, Ordering::SeqCst);
            let error = self.commit_error.take();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::QueryExecution(msg));
                }
                Ok(())
            })
        }
    }

    struct MockBackend {
        transaction: std::sync::Mutex<Option<MockTransaction>>,
        open_error: Option<String>,
        is_open: bool,
        open_called: Arc<AtomicUsize>,
    }

    impl MockBackend {
        fn new(tx: MockTransaction) -> Self {
            Self {
                transaction: std::sync::Mutex::new(Some(tx)),
                open_error: None,
                is_open: true,
                open_called: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn failing(msg: &str) -> Self {
            Self {
                transaction: std::sync::Mutex::new(None),
                open_error: Some(msg.to_string()),
                is_open: true,
                open_called: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl DriverBackend for MockBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TransactionType,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Box<dyn TransactionOps>, PipelineError>>
                    + Send
                    + '_,
            >,
        > {
            self.open_called.fetch_add(1, Ordering::SeqCst);
            let tx = self.transaction.lock().unwrap().take();
            let error = self.open_error.clone();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::QueryExecution(msg));
                }
                Ok(Box::new(tx.expect("MockBackend: no transaction configured"))
                    as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            self.is_open
        }
    }

    fn make_client(backend: MockBackend) -> TypeDBClient {
        TypeDBClient::with_backend(Box::new(backend))
    }

    // =============================================
    // parse_transaction_type tests
    // =============================================

    #[test]
    fn parse_transaction_type_read() {
        let result = parse_transaction_type("read").unwrap();
        assert_eq!(result, TransactionType::Read);
    }

    #[test]
    fn parse_transaction_type_write() {
        let result = parse_transaction_type("write").unwrap();
        assert_eq!(result, TransactionType::Write);
    }

    #[test]
    fn parse_transaction_type_schema() {
        let result = parse_transaction_type("schema").unwrap();
        assert_eq!(result, TransactionType::Schema);
    }

    #[test]
    fn parse_transaction_type_unknown() {
        let result = parse_transaction_type("unknown");
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("Unknown transaction type: unknown")));
    }

    #[test]
    fn parse_transaction_type_empty() {
        let result = parse_transaction_type("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_transaction_type_case_sensitive() {
        let result = parse_transaction_type("Read");
        assert!(result.is_err());
    }

    // =============================================
    // execute tests (via MockBackend)
    // =============================================

    #[tokio::test]
    async fn execute_ok_read_no_commit() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "match $x isa thing;", "read").await.unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_ok_write_commits() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "insert $x isa thing;", "write").await.unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_ok_schema_commits() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "define entity thing;", "schema").await.unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_rows_read_no_commit() {
        let rows = vec![serde_json::json!({"name": "Alice"}), serde_json::json!({"name": "Bob"})];
        let tx = MockTransaction::new(QueryResultKind::Rows(rows.clone()));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "match $p isa person;", "read").await.unwrap();
        assert_eq!(result, serde_json::Value::Array(rows));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_rows_write_commits() {
        let rows = vec![serde_json::json!({"id": 1})];
        let tx = MockTransaction::new(QueryResultKind::Rows(rows));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "insert $x isa thing;", "write").await.unwrap();
        assert!(result.is_array());
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_rows_data_preserved() {
        let rows = vec![
            serde_json::json!({"name": "Alice", "age": 30}),
            serde_json::json!({"name": "Bob", "age": 25}),
        ];
        let tx = MockTransaction::new(QueryResultKind::Rows(rows.clone()));
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "match $p isa person;", "read").await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "Alice");
        assert_eq!(arr[1]["age"], 25);
    }

    #[tokio::test]
    async fn execute_docs_read_no_commit() {
        let docs = vec![serde_json::json!({"doc": "data"})];
        let tx = MockTransaction::new(QueryResultKind::Documents(docs.clone()));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "match $p isa person; fetch {};", "read").await.unwrap();
        assert_eq!(result, serde_json::Value::Array(docs));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_docs_write_commits() {
        let docs = vec![serde_json::json!({"doc": "data"})];
        let tx = MockTransaction::new(QueryResultKind::Documents(docs));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "insert $x isa thing;", "write").await.unwrap();
        assert!(result.is_array());
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_docs_commit_error_ignored() {
        let docs = vec![serde_json::json!({"doc": "data"})];
        let tx = MockTransaction::new(QueryResultKind::Documents(docs.clone()))
            .with_commit_error("commit failed");
        let client = make_client(MockBackend::new(tx));

        // Documents + write: commit error is intentionally ignored (let _ = ...)
        let result = client.execute("db", "insert $x isa thing;", "write").await.unwrap();
        assert_eq!(result, serde_json::Value::Array(docs));
    }

    #[tokio::test]
    async fn execute_transaction_open_failure() {
        let client = make_client(MockBackend::failing("connection refused"));

        let result = client.execute("db", "match $x isa thing;", "read").await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("connection refused")));
    }

    #[tokio::test]
    async fn execute_query_failure() {
        let tx = MockTransaction::failing_query("syntax error");
        let client = make_client(MockBackend::new(tx));

        let result = client.execute("db", "bad query", "read").await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("syntax error")));
    }

    #[tokio::test]
    async fn execute_commit_failure_ok_propagated() {
        let tx = MockTransaction::new(QueryResultKind::Ok)
            .with_commit_error("commit failed");
        let client = make_client(MockBackend::new(tx));

        // Ok + write: commit error IS propagated
        let result = client.execute("db", "insert $x isa thing;", "write").await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("commit failed")));
    }

    #[tokio::test]
    async fn execute_commit_failure_rows_propagated() {
        let tx = MockTransaction::new(QueryResultKind::Rows(vec![]))
            .with_commit_error("commit failed");
        let client = make_client(MockBackend::new(tx));

        // Rows + write: commit error IS propagated
        let result = client.execute("db", "insert $x isa thing;", "write").await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("commit failed")));
    }

    #[tokio::test]
    async fn execute_invalid_transaction_type() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let backend = MockBackend::new(tx);
        let open_called = backend.open_called.clone();
        let client = make_client(backend);

        let result = client.execute("db", "match $x;", "invalid").await;
        assert!(result.is_err());
        // Backend should never be called if transaction type is invalid
        assert_eq!(open_called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn is_connected_delegates_to_backend() {
        let mut backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok));
        backend.is_open = true;
        let client = make_client(backend);
        assert!(client.is_connected());
    }

    #[test]
    fn is_connected_false_when_backend_closed() {
        let mut backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok));
        backend.is_open = false;
        let client = make_client(backend);
        assert!(!client.is_connected());
    }

    // =============================================
    // value_to_json tests
    // =============================================

    #[test]
    fn value_to_json_boolean_true() {
        let value = Value::Boolean(true);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::Value::Bool(true));
    }

    #[test]
    fn value_to_json_boolean_false() {
        let value = Value::Boolean(false);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::Value::Bool(false));
    }

    #[test]
    fn value_to_json_integer() {
        let value = Value::Integer(42);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::json!(42));
    }

    #[test]
    fn value_to_json_integer_negative() {
        let value = Value::Integer(-100);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::json!(-100));
    }

    #[test]
    fn value_to_json_double() {
        let value = Value::Double(3.15);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::json!(3.15));
    }

    #[test]
    fn value_to_json_string() {
        let value = Value::String("hello".to_string());
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn value_to_json_string_empty() {
        let value = Value::String(String::new());
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::Value::String(String::new()));
    }

    #[test]
    fn value_to_json_date() {
        let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let value = Value::Date(date);
        let json = value_to_json(&value);
        assert_eq!(json, serde_json::Value::String("2024-01-15".to_string()));
    }

    #[test]
    fn value_to_json_datetime() {
        let dt = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
            .unwrap()
            .and_hms_opt(10, 30, 0)
            .unwrap();
        let value = Value::Datetime(dt);
        let json = value_to_json(&value);
        let s = json.as_str().unwrap();
        assert!(s.contains("2024-01-15"));
    }

    #[test]
    fn value_to_json_decimal() {
        let dec = Decimal::new(42, 0);
        let value = Value::Decimal(dec);
        let json = value_to_json(&value);
        assert!(json.is_string());
    }

    #[test]
    fn value_to_json_duration() {
        let dur = Duration::new(1, 2, 3_000_000_000);
        let value = Value::Duration(dur);
        let json = value_to_json(&value);
        assert!(json.is_string());
    }

    #[test]
    fn value_to_json_datetime_tz() {
        use chrono::TimeZone as _;
        let tz = TimeZone::Fixed(chrono::FixedOffset::east_opt(3600).unwrap());
        let dt = tz.with_ymd_and_hms(2024, 6, 15, 12, 30, 0).unwrap();
        let value = Value::DatetimeTZ(dt);
        let json = value_to_json(&value);
        let s = json.as_str().unwrap();
        assert!(s.contains("2024"));
    }

    // =============================================
    // concept_to_json tests
    // =============================================

    #[test]
    fn concept_to_json_entity_type() {
        let concept = Concept::EntityType(EntityType {
            label: "person".to_string(),
        });
        let json = concept_to_json(&concept);
        assert_eq!(json["category"], "EntityType");
        assert_eq!(json["label"], "person");
        assert!(json.get("iid").is_none());
        assert!(json.get("value").is_none());
    }

    #[test]
    fn concept_to_json_attribute_type() {
        let concept = Concept::AttributeType(AttributeType {
            label: "name".to_string(),
            value_type: Some(ValueType::String),
        });
        let json = concept_to_json(&concept);
        assert_eq!(json["category"], "AttributeType");
        assert_eq!(json["label"], "name");
        assert_eq!(json["value_type"], "string");
    }

    #[test]
    fn concept_to_json_value_boolean() {
        let concept = Concept::Value(Value::Boolean(true));
        let json = concept_to_json(&concept);
        assert_eq!(json["category"], "Value");
        assert_eq!(json["value"], true);
    }

    #[test]
    fn concept_to_json_value_integer() {
        let concept = Concept::Value(Value::Integer(42));
        let json = concept_to_json(&concept);
        assert_eq!(json["value"], 42);
    }

    #[test]
    fn concept_to_json_value_string() {
        let concept = Concept::Value(Value::String("hello".to_string()));
        let json = concept_to_json(&concept);
        assert_eq!(json["value"], "hello");
    }

    #[test]
    fn concept_to_json_entity_with_iid() {
        let iid: IID = vec![0x01, 0x02, 0x03].into();
        let concept = Concept::Entity(Entity {
            iid,
            type_: Some(EntityType { label: "person".to_string() }),
        });
        let json = concept_to_json(&concept);
        assert_eq!(json["category"], "Entity");
        assert_eq!(json["label"], "person");
        // IID should be present
        let iid_str = json["iid"].as_str().unwrap();
        assert!(iid_str.starts_with("0x"));
    }

    #[test]
    fn concept_to_json_attribute_with_value() {
        let iid: IID = vec![0xAA, 0xBB].into();
        let concept = Concept::Attribute(Attribute {
            iid,
            value: Value::String("hello".to_string()),
            type_: Some(AttributeType { label: "name".to_string(), value_type: Some(ValueType::String) }),
        });
        let json = concept_to_json(&concept);
        assert_eq!(json["category"], "Attribute");
        assert_eq!(json["label"], "name");
        // Attribute IID is not exposed via try_get_iid()
        assert!(json.get("iid").is_none());
        assert_eq!(json["value"], "hello");
        assert_eq!(json["value_type"], "string");
    }

    #[test]
    fn concept_to_json_attribute_type_without_value_type() {
        let concept = Concept::AttributeType(AttributeType {
            label: "abstract_attr".to_string(),
            value_type: None,
        });
        let json = concept_to_json(&concept);
        assert_eq!(json["label"], "abstract_attr");
        assert!(json.get("value_type").is_none());
    }

    // =============================================
    // QueryExecutor trait impl tests
    // =============================================

    #[test]
    fn type_db_client_implements_query_executor() {
        fn assert_executor<T: QueryExecutor>() {}
        assert_executor::<TypeDBClient>();
    }

    #[tokio::test]
    async fn query_executor_execute_delegates_to_client() {
        let tx = MockTransaction::new(QueryResultKind::Rows(vec![serde_json::json!({"x": 1})]));
        let client = make_client(MockBackend::new(tx));
        let executor: Box<dyn QueryExecutor> = Box::new(client);

        let result = executor.execute("db", "match $x isa thing;", "read").await.unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 1);
    }

    #[test]
    fn query_executor_is_connected_delegates_to_client() {
        let mut backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok));
        backend.is_open = true;
        let client = make_client(backend);
        let executor: Box<dyn QueryExecutor> = Box::new(client);
        assert!(executor.is_connected());
    }

    // =============================================
    // Integration tests (require running TypeDB)
    // =============================================

    #[tokio::test]
    #[ignore = "requires running TypeDB server"]
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn integration_connect_invalid_address() {
        let config = TypeDBSection {
            address: "localhost:99999".to_string(),
            database: "test".to_string(),
            username: "admin".to_string(),
            password: "password".to_string(),
        };
        let result = TypeDBClient::connect(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore = "requires running TypeDB server"]
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn integration_connect_success() {
        let config = TypeDBSection {
            address: "localhost:1729".to_string(),
            database: "test".to_string(),
            username: "admin".to_string(),
            password: "password".to_string(),
        };
        let result = TypeDBClient::connect(&config).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_connected());
    }
}
