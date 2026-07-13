//! Primary database connection handle.
//!
//! # Connection Pooling
//!
//! The TypeDB driver manages connection pooling internally — no custom
//! pooling layer is needed. Each [`Database`] instance wraps a single
//! driver that efficiently reuses connections under the hood.
//!
//! To share a `Database` across async tasks, wrap it in `Arc`:
//!
//! ```ignore
//! let db = Arc::new(Database::connect("localhost:1729", "mydb", "admin", "password").await?);
//! let db2 = Arc::clone(&db);
//! tokio::spawn(async move {
//!     let manager = EntityManager::<Person>::new(&db2);
//!     manager.all().await.unwrap();
//! });
//! ```

use std::sync::Arc;

use super::backend::{DriverBackend, QueryResult, TxType};
use super::context::TransactionContext;
use super::transaction::Transaction;
use crate::error::Result;

/// Primary connection handle wrapping a TypeDB driver.
///
/// Provides methods to create transactions and execute raw queries.
/// Use [`EntityManager`](crate::manager::EntityManager) for typed CRUD.
///
/// `Database` is `Send + Sync`, so it can be shared across tasks via
/// [`Arc`]. The TypeDB driver handles connection pooling internally.
pub struct Database {
    backend: Box<dyn DriverBackend>,
    database_name: String,
}

impl Database {
    /// Create a Database with a custom backend (for testing).
    pub fn with_backend(backend: Box<dyn DriverBackend>, database_name: impl Into<String>) -> Self {
        Self {
            backend,
            database_name: database_name.into(),
        }
    }

    /// Connect to a TypeDB server with default [`ConnectOptions`].
    ///
    /// Permanent convenience wrapper over [`Self::connect_with_options`] for
    /// the common case (HTTP probe on the default port, no TLS).
    ///
    /// [`ConnectOptions`]: super::real_driver::ConnectOptions
    #[cfg(feature = "typedb")]
    pub async fn connect(
        address: &str,
        database: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        Self::connect_with_options(
            address,
            database,
            username,
            password,
            super::real_driver::ConnectOptions::default(),
        )
        .await
    }

    /// Connect to a TypeDB server with explicit [`ConnectOptions`].
    #[cfg(feature = "typedb")]
    pub async fn connect_with_options(
        address: &str,
        database: &str,
        username: &str,
        password: &str,
        options: super::real_driver::ConnectOptions,
    ) -> Result<Self> {
        let backend =
            super::real_driver::RealBackend::connect(address, username, password, options).await?;
        Ok(Self {
            backend: Box::new(backend),
            database_name: database.to_string(),
        })
    }

    /// Open a read transaction.
    pub async fn read_transaction(&self) -> Result<Transaction> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, TxType::Read)
            .await?;
        Ok(Transaction::new(tx, TxType::Read))
    }

    /// Open a write transaction.
    pub async fn write_transaction(&self) -> Result<Transaction> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, TxType::Write)
            .await?;
        Ok(Transaction::new(tx, TxType::Write))
    }

    /// Create a shared [`TransactionContext`] for grouping operations.
    pub async fn transaction_context(&self, tx_type: TxType) -> Result<TransactionContext> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        Ok(TransactionContext::new(tx, tx_type))
    }

    /// Get the database name.
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    /// Check if the underlying connection is alive.
    pub fn is_connected(&self) -> bool {
        self.backend.is_open()
    }

    /// The server version detected at connect time, when known.
    ///
    /// `None` for backends without a version gate and on the band-7 gRPC
    /// fallback, where the server cannot report its version.
    pub fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.backend.server_version()
    }

    /// Version-gate schema DDL that uses `@doc`/`@meta` annotations.
    ///
    /// When the TypeQL uses schema annotations (TypeDB 3.12+) and the detected
    /// server version predates 3.12, fail with an actionable versioned error
    /// instead of letting the server produce a syntax error. When the server
    /// version is unknown (band-7 gRPC fallback without `server_version=`),
    /// the DDL is sent as-is and the server decides.
    pub fn check_schema_annotation_support(&self, typeql: &str) -> Result<()> {
        use type_bridge_core_lib::version::{Feature, check_feature_supported};

        if let Some(server) = self.server_version()
            && crate::schema::annotations::typeql_uses_schema_annotations(typeql)
        {
            check_feature_supported(Feature::SchemaAnnotations, &server)
                .map_err(crate::error::OrmError::UnsupportedVersion)?;
        }
        Ok(())
    }

    /// Return whether this database exists on the connected TypeDB server.
    pub async fn database_exists(&self) -> Result<bool> {
        self.backend.database_exists(&self.database_name).await
    }

    /// Create this database if it does not already exist.
    pub async fn create_database(&self) -> Result<()> {
        if !self.database_exists().await? {
            self.backend.create_database(&self.database_name).await?;
        }
        Ok(())
    }

    /// Delete this database if it exists.
    pub async fn delete_database(&self) -> Result<()> {
        if self.database_exists().await? {
            self.backend.delete_database(&self.database_name).await?;
        }
        Ok(())
    }

    /// Export the database schema as TypeQL text.
    pub async fn schema_text(&self) -> Result<String> {
        self.backend.schema_text(&self.database_name).await
    }

    /// Wrap this database in an `Arc` for sharing across async tasks.
    pub fn into_shared(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Execute a raw TypeQL query, auto-managing the transaction lifecycle.
    ///
    /// Opens a new transaction, executes the query, and commits if the
    /// transaction type is `Write` or `Schema`.
    #[tracing::instrument(skip(self, typeql), fields(db = %self.database_name))]
    pub async fn execute_raw(&self, typeql: &str, tx_type: TxType) -> Result<QueryResult> {
        let mut tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        let result = tx.query(typeql).await?;
        if matches!(tx_type, TxType::Write | TxType::Schema) {
            tx.commit().await?;
        }
        Ok(result)
    }
}
