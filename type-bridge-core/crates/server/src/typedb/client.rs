use super::backend::{DriverBackend, QueryResultKind, TransactionType};
#[cfg(feature = "v2-query")]
use super::real_driver::sanitize_prepared_connect_error;
use super::real_driver::{RealTypeDBBackend, prepare_secure_connect_options};
use crate::config::{SecureTypeDBSection, TypeDBSection};
use crate::error::PipelineError;
use crate::executor::QueryExecutor;
use type_bridge_typedb_runtime::PreparedSecureConnectOptions;

/// One immutable server connection identity bound to one prepared transport.
///
/// The fields are intentionally private: a caller cannot pair trust material,
/// HTTP discovery settings, or a selected driver band with a different host or
/// credential set after preparation.
#[doc(hidden)]
pub struct PreparedSecureTypeDBConnection {
    address: String,
    #[cfg(feature = "v2-query")]
    database: String,
    username: String,
    password: String,
    options: PreparedSecureConnectOptions,
}

impl PreparedSecureTypeDBConnection {
    /// Connect the V2 ORM authority through this exact prepared identity.
    #[cfg(feature = "v2-query")]
    #[doc(hidden)]
    pub async fn connect_database(&self) -> Result<type_bridge_orm::Database, PipelineError> {
        type_bridge_orm::Database::connect_prepared_secure_with_options(
            &self.address,
            &self.database,
            &self.username,
            &self.password,
            self.options.clone(),
        )
        .await
        .map_err(sanitize_prepared_connect_error)
    }
}

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

    /// Connect using the standalone server's validated secure configuration.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn connect_secure(config: &SecureTypeDBSection) -> Result<Self, PipelineError> {
        let prepared = Self::prepare_secure_transport(config)?;
        Self::connect_prepared_secure(&prepared).await
    }

    /// Validate and snapshot one secure transport policy for reuse.
    pub fn prepare_secure_transport(
        config: &SecureTypeDBSection,
    ) -> Result<PreparedSecureTypeDBConnection, PipelineError> {
        // Resolve trust before retaining any credential-bearing connection
        // identity, then bind both into one opaque value.
        let options = prepare_secure_connect_options(config)?;
        let connection = &config.connection;
        Ok(PreparedSecureTypeDBConnection {
            address: connection.address.clone(),
            #[cfg(feature = "v2-query")]
            database: connection.database.clone(),
            username: connection.username.clone(),
            password: connection.password.clone(),
            options,
        })
    }

    /// Connect through an already prepared immutable transport snapshot.
    pub async fn connect_prepared_secure(
        prepared: &PreparedSecureTypeDBConnection,
    ) -> Result<Self, PipelineError> {
        let backend = RealTypeDBBackend::connect_prepared_secure(
            &prepared.address,
            &prepared.username,
            &prepared.password,
            prepared.options.clone(),
        )
        .await?;
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

        let needs_commit = matches!(
            transaction_type,
            TransactionType::Write | TransactionType::Schema
        );

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

    /// Return whether a TypeDB database exists on the connected server.
    pub async fn database_exists(&self, database: &str) -> Result<bool, PipelineError> {
        self.backend.database_exists(database).await
    }

    /// Create a TypeDB database.
    pub async fn create_database(&self, database: &str) -> Result<(), PipelineError> {
        self.backend.create_database(database).await
    }

    /// Delete a TypeDB database.
    pub async fn delete_database(&self, database: &str) -> Result<(), PipelineError> {
        self.backend.delete_database(database).await
    }

    /// Delete a TypeDB database if it exists, then create it.
    pub async fn reset_database(&self, database: &str) -> Result<(), PipelineError> {
        if self.database_exists(database).await? {
            self.delete_database(database).await?;
        }
        self.create_database(database).await
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
        Box<dyn std::future::Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>,
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

#[cfg(test)]
#[cfg(feature = "band8")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::error::Error as _;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

    #[derive(Default)]
    struct MockDatabaseAdmin {
        exists_results: std::sync::Mutex<VecDeque<Result<bool, String>>>,
        create_errors: std::sync::Mutex<VecDeque<String>>,
        delete_errors: std::sync::Mutex<VecDeque<String>>,
        exists_called: AtomicUsize,
        create_called: AtomicUsize,
        delete_called: AtomicUsize,
        operations: std::sync::Mutex<Vec<String>>,
    }

    struct MockBackend {
        transaction: std::sync::Mutex<Option<MockTransaction>>,
        open_error: Option<String>,
        is_open: bool,
        open_called: Arc<AtomicUsize>,
        database_admin: Arc<MockDatabaseAdmin>,
    }

    impl MockBackend {
        fn new(tx: MockTransaction) -> Self {
            Self {
                transaction: std::sync::Mutex::new(Some(tx)),
                open_error: None,
                is_open: true,
                open_called: Arc::new(AtomicUsize::new(0)),
                database_admin: Arc::new(MockDatabaseAdmin::default()),
            }
        }

        fn failing(msg: &str) -> Self {
            Self {
                transaction: std::sync::Mutex::new(None),
                open_error: Some(msg.to_string()),
                is_open: true,
                open_called: Arc::new(AtomicUsize::new(0)),
                database_admin: Arc::new(MockDatabaseAdmin::default()),
            }
        }

        fn with_database_exists_results(self, results: Vec<Result<bool, String>>) -> Self {
            *self.database_admin.exists_results.lock().unwrap() = results.into();
            self
        }

        fn with_create_errors(self, errors: Vec<&str>) -> Self {
            *self.database_admin.create_errors.lock().unwrap() =
                errors.into_iter().map(str::to_string).collect();
            self
        }

        fn with_delete_errors(self, errors: Vec<&str>) -> Self {
            *self.database_admin.delete_errors.lock().unwrap() =
                errors.into_iter().map(str::to_string).collect();
            self
        }

        fn database_admin_state(&self) -> Arc<MockDatabaseAdmin> {
            Arc::clone(&self.database_admin)
        }
    }

    impl DriverBackend for MockBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TransactionType,
        ) -> Pin<Box<dyn Future<Output = Result<Box<dyn TransactionOps>, PipelineError>> + Send + '_>>
        {
            self.open_called.fetch_add(1, Ordering::SeqCst);
            let tx = self.transaction.lock().unwrap().take();
            let error = self.open_error.clone();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::QueryExecution(msg));
                }
                Ok(
                    Box::new(tx.expect("MockBackend: no transaction configured"))
                        as Box<dyn TransactionOps>,
                )
            })
        }

        fn is_open(&self) -> bool {
            self.is_open
        }

        fn database_exists(
            &self,
            database: &str,
        ) -> Pin<Box<dyn Future<Output = Result<bool, PipelineError>> + Send + '_>> {
            self.database_admin
                .exists_called
                .fetch_add(1, Ordering::SeqCst);
            self.database_admin
                .operations
                .lock()
                .unwrap()
                .push(format!("exists:{database}"));
            let result = self
                .database_admin
                .exists_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(false));
            Box::pin(async move { result.map_err(PipelineError::Connection) })
        }

        fn create_database(
            &self,
            database: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>> {
            self.database_admin
                .create_called
                .fetch_add(1, Ordering::SeqCst);
            self.database_admin
                .operations
                .lock()
                .unwrap()
                .push(format!("create:{database}"));
            let error = self
                .database_admin
                .create_errors
                .lock()
                .unwrap()
                .pop_front();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::Connection(msg));
                }
                Ok(())
            })
        }

        fn delete_database(
            &self,
            database: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>> {
            self.database_admin
                .delete_called
                .fetch_add(1, Ordering::SeqCst);
            self.database_admin
                .operations
                .lock()
                .unwrap()
                .push(format!("delete:{database}"));
            let error = self
                .database_admin
                .delete_errors
                .lock()
                .unwrap()
                .pop_front();
            Box::pin(async move {
                if let Some(msg) = error {
                    return Err(PipelineError::Connection(msg));
                }
                Ok(())
            })
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
        assert!(
            matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("Unknown transaction type: unknown"))
        );
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

        let result = client
            .execute("db", "match $x isa thing;", "read")
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_ok_write_commits() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client
            .execute("db", "insert $x isa thing;", "write")
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_ok_schema_commits() {
        let tx = MockTransaction::new(QueryResultKind::Ok);
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client
            .execute("db", "define entity thing;", "schema")
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert!(committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_rows_read_no_commit() {
        let rows = vec![
            serde_json::json!({"name": "Alice"}),
            serde_json::json!({"name": "Bob"}),
        ];
        let tx = MockTransaction::new(QueryResultKind::Rows(rows.clone()));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client
            .execute("db", "match $p isa person;", "read")
            .await
            .unwrap();
        assert_eq!(result, serde_json::Value::Array(rows));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_rows_write_commits() {
        let rows = vec![serde_json::json!({"id": 1})];
        let tx = MockTransaction::new(QueryResultKind::Rows(rows));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client
            .execute("db", "insert $x isa thing;", "write")
            .await
            .unwrap();
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

        let result = client
            .execute("db", "match $p isa person;", "read")
            .await
            .unwrap();
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

        let result = client
            .execute("db", "match $p isa person; fetch {};", "read")
            .await
            .unwrap();
        assert_eq!(result, serde_json::Value::Array(docs));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_docs_write_commits() {
        let docs = vec![serde_json::json!({"doc": "data"})];
        let tx = MockTransaction::new(QueryResultKind::Documents(docs));
        let committed = tx.committed.clone();
        let client = make_client(MockBackend::new(tx));

        let result = client
            .execute("db", "insert $x isa thing;", "write")
            .await
            .unwrap();
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
        let result = client
            .execute("db", "insert $x isa thing;", "write")
            .await
            .unwrap();
        assert_eq!(result, serde_json::Value::Array(docs));
    }

    #[tokio::test]
    async fn execute_transaction_open_failure() {
        let client = make_client(MockBackend::failing("connection refused"));

        let result = client.execute("db", "match $x isa thing;", "read").await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("connection refused"))
        );
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
        let tx = MockTransaction::new(QueryResultKind::Ok).with_commit_error("commit failed");
        let client = make_client(MockBackend::new(tx));

        // Ok + write: commit error IS propagated
        let result = client.execute("db", "insert $x isa thing;", "write").await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("commit failed"))
        );
    }

    #[tokio::test]
    async fn execute_commit_failure_rows_propagated() {
        let tx =
            MockTransaction::new(QueryResultKind::Rows(vec![])).with_commit_error("commit failed");
        let client = make_client(MockBackend::new(tx));

        // Rows + write: commit error IS propagated
        let result = client.execute("db", "insert $x isa thing;", "write").await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("commit failed"))
        );
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
    // database admin tests (via MockBackend)
    // =============================================

    #[tokio::test]
    async fn database_exists_true_delegates_to_backend() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Ok(true)]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        assert!(client.database_exists("admin_db").await.unwrap());
        assert_eq!(admin.exists_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["exists:admin_db".to_string()]
        );
    }

    #[tokio::test]
    async fn database_exists_false_delegates_to_backend() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Ok(false)]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        assert!(!client.database_exists("missing_db").await.unwrap());
        assert_eq!(admin.exists_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["exists:missing_db".to_string()]
        );
    }

    #[tokio::test]
    async fn create_database_delegates_to_backend() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok));
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        client.create_database("new_db").await.unwrap();
        assert_eq!(admin.create_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["create:new_db".to_string()]
        );
    }

    #[tokio::test]
    async fn delete_database_delegates_to_backend() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok));
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        client.delete_database("old_db").await.unwrap();
        assert_eq!(admin.delete_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["delete:old_db".to_string()]
        );
    }

    #[tokio::test]
    async fn reset_database_deletes_existing_database_before_create() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Ok(true)]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        client.reset_database("reset_db").await.unwrap();
        assert_eq!(admin.exists_called.load(Ordering::SeqCst), 1);
        assert_eq!(admin.delete_called.load(Ordering::SeqCst), 1);
        assert_eq!(admin.create_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec![
                "exists:reset_db".to_string(),
                "delete:reset_db".to_string(),
                "create:reset_db".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn reset_database_creates_when_database_is_absent() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Ok(false)]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        client.reset_database("reset_db").await.unwrap();
        assert_eq!(admin.exists_called.load(Ordering::SeqCst), 1);
        assert_eq!(admin.delete_called.load(Ordering::SeqCst), 0);
        assert_eq!(admin.create_called.load(Ordering::SeqCst), 1);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["exists:reset_db".to_string(), "create:reset_db".to_string(),]
        );
    }

    #[tokio::test]
    async fn database_exists_propagates_backend_error() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Err("lookup failed".to_string())]);
        let client = make_client(backend);

        let err = client.database_exists("db").await.unwrap_err();
        assert!(matches!(&err, PipelineError::Connection(msg) if msg.contains("lookup failed")));
    }

    #[tokio::test]
    async fn create_database_propagates_backend_error() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_create_errors(vec!["create failed"]);
        let client = make_client(backend);

        let err = client.create_database("db").await.unwrap_err();
        assert!(matches!(&err, PipelineError::Connection(msg) if msg.contains("create failed")));
    }

    #[tokio::test]
    async fn delete_database_propagates_backend_error() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_delete_errors(vec!["delete failed"]);
        let client = make_client(backend);

        let err = client.delete_database("db").await.unwrap_err();
        assert!(matches!(&err, PipelineError::Connection(msg) if msg.contains("delete failed")));
    }

    #[tokio::test]
    async fn reset_database_propagates_lookup_error_without_mutating() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Err("lookup failed".to_string())]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        let err = client.reset_database("db").await.unwrap_err();
        assert!(matches!(&err, PipelineError::Connection(msg) if msg.contains("lookup failed")));
        assert_eq!(admin.delete_called.load(Ordering::SeqCst), 0);
        assert_eq!(admin.create_called.load(Ordering::SeqCst), 0);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["exists:db".to_string()]
        );
    }

    #[tokio::test]
    async fn reset_database_propagates_delete_error_without_create() {
        let backend = MockBackend::new(MockTransaction::new(QueryResultKind::Ok))
            .with_database_exists_results(vec![Ok(true)])
            .with_delete_errors(vec!["delete failed"]);
        let admin = backend.database_admin_state();
        let client = make_client(backend);

        let err = client.reset_database("db").await.unwrap_err();
        assert!(matches!(&err, PipelineError::Connection(msg) if msg.contains("delete failed")));
        assert_eq!(admin.create_called.load(Ordering::SeqCst), 0);
        assert_eq!(
            admin.operations.lock().unwrap().clone(),
            vec!["exists:db".to_string(), "delete:db".to_string()]
        );
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

        let result = executor
            .execute("db", "match $x isa thing;", "read")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn prepared_first_and_v2_connections_never_expose_credentials_or_provider_identity() {
        const ADDRESS: &str = "admin:TB_ADDRESS_SECRET@provider.invalid:1729";
        const USERNAME: &str = "TB_USERNAME_SECRET";
        const PASSWORD: &str = "TB_PASSWORD_SECRET";
        let config = SecureTypeDBSection::new(
            TypeDBSection {
                address: ADDRESS.to_owned(),
                database: "db".to_owned(),
                username: USERNAME.to_owned(),
                password: PASSWORD.to_owned(),
                http_port: 8000,
                server_version: Some("3.12.1".to_owned()),
            },
            crate::config::OutboundTlsMode::Disabled,
        );
        let prepared = TypeDBClient::prepare_secure_transport(&config).unwrap();

        let first = match TypeDBClient::connect_prepared_secure(&prepared).await {
            Ok(_) => panic!("malformed provider address unexpectedly connected"),
            Err(error) => error,
        };
        let rendered = format!("{first}\n{first:?}");
        for secret in [ADDRESS, USERNAME, PASSWORD] {
            assert!(!rendered.contains(secret), "{secret}: {rendered}");
        }
        assert!(first.source().is_none());

        #[cfg(feature = "v2-query")]
        {
            let second = match prepared.connect_database().await {
                Ok(_) => panic!("malformed V2 provider address unexpectedly connected"),
                Err(error) => error,
            };
            let rendered = format!("{second}\n{second:?}");
            for secret in [ADDRESS, USERNAME, PASSWORD] {
                assert!(!rendered.contains(secret), "{secret}: {rendered}");
            }
            assert!(second.source().is_none());
        }
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
            http_port: 8000,
            server_version: None,
        };
        let result = TypeDBClient::connect(&config).await;
        assert!(result.is_err());
    }

    /// Live-test target resolved from the environment so the suite can point
    /// at any disposable TypeDB container instead of a fixed local install.
    fn live_config() -> TypeDBSection {
        TypeDBSection {
            address: std::env::var("TYPEDB_ADDRESS")
                .unwrap_or_else(|_| "localhost:1729".to_string()),
            database: std::env::var("TYPEDB_DATABASE").unwrap_or_else(|_| "test".to_string()),
            username: "admin".to_string(),
            password: "password".to_string(),
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(8000),
            server_version: std::env::var("TYPEDB_SERVER_VERSION").ok(),
        }
    }

    #[tokio::test]
    #[ignore = "requires running TypeDB server"]
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn integration_connect_success() {
        let result = TypeDBClient::connect(&live_config()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_connected());
    }

    #[tokio::test]
    #[ignore = "requires running TypeDB server"]
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn integration_database_admin_roundtrip() {
        let config = live_config();
        let client = TypeDBClient::connect(&config)
            .await
            .expect("connect failed");
        let database = format!("type_bridge_server_admin_{}", uuid::Uuid::new_v4().simple());

        if client.database_exists(&database).await.unwrap_or(false) {
            let _ = client.delete_database(&database).await;
        }

        assert!(!client.database_exists(&database).await.unwrap());
        client
            .create_database(&database)
            .await
            .expect("create failed");
        assert!(client.database_exists(&database).await.unwrap());

        client
            .reset_database(&database)
            .await
            .expect("reset existing database failed");
        assert!(client.database_exists(&database).await.unwrap());

        client
            .delete_database(&database)
            .await
            .expect("delete failed");
        assert!(!client.database_exists(&database).await.unwrap());

        client
            .reset_database(&database)
            .await
            .expect("reset absent database failed");
        assert!(client.database_exists(&database).await.unwrap());

        client
            .delete_database(&database)
            .await
            .expect("cleanup delete failed");
    }

    #[tokio::test]
    #[ignore = "requires running TypeDB server"]
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn integration_execute_roundtrip() {
        let config = live_config();
        let client = TypeDBClient::connect(&config)
            .await
            .expect("connect failed");

        client
            .execute(&config.database, "define entity smoke_marker;", "schema")
            .await
            .expect("schema define failed");

        let rows = client
            .execute(&config.database, "match entity $t;", "read")
            .await
            .expect("read query failed");
        let rows = rows.as_array().expect("read result must be a JSON array");
        assert!(
            !rows.is_empty(),
            "expected at least the smoke_marker entity type"
        );
    }
}
