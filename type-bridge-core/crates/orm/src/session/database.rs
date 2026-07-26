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

use std::fmt;
use std::net::Ipv6Addr;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::backend::{DriverBackend, GivenRowsSpec, QueryResult, TxType};
use super::context::TransactionContext;
use super::transaction::Transaction;
use crate::error::Result;
use crate::match_request::selected_result_executor::SelectedResultExecutor;
use crate::match_request::{MatchExecutionLimits, ValidatedMatchRequest, ValidatedMatchResult};
use crate::registry::DescriptorRegistry;

/// Primary connection handle wrapping a TypeDB driver.
///
/// Provides methods to create transactions and execute raw queries.
/// Use [`EntityManager`](crate::manager::EntityManager) for typed CRUD.
///
/// `Database` is `Send + Sync`, so it can be shared across tasks via
/// [`Arc`]. The TypeDB driver handles connection pooling internally.
pub struct Database {
    backend: Box<dyn DriverBackend>,
    connection_authority: DatabaseConnectionAuthority,
    database_name: String,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct DatabaseExecutionIdentity {
    connection_authority: DatabaseConnectionAuthority,
    database_name: String,
}

/// Opaque authority proving that database handles target one provider endpoint.
///
/// Real connections derive this private token from the TypeDB address only;
/// credentials, database names, TLS material, and connection options never
/// contribute. Real handles compare equal only when that address has the exact
/// same spelling; DNS and loopback aliases intentionally fail closed. Custom
/// backends receive an isolated token by default and must explicitly clone one
/// token across handles that intentionally share a provider authority. The token
/// is not serializable and its debug projection is permanently redacted.
#[derive(Clone)]
pub struct DatabaseConnectionAuthority(DatabaseConnectionAuthorityKind);

#[derive(Clone)]
enum DatabaseConnectionAuthorityKind {
    Provider([u8; 32]),
    Custom(Arc<()>),
}

impl DatabaseConnectionAuthority {
    /// Create one isolated authority token for explicitly paired custom backends.
    ///
    /// Clone this value only for backend handles that are guaranteed by their
    /// owner to reach the same provider. Independently created tokens compare
    /// unequal and therefore fail closed at migration-store construction.
    #[must_use]
    pub fn isolated() -> Self {
        Self(DatabaseConnectionAuthorityKind::Custom(Arc::new(())))
    }

    fn for_typedb_address(address: &str) -> Self {
        if !identity_safe_provider_address(address) {
            // Released connection constructors continue to pass unusual
            // addresses to the driver unchanged. They simply cannot acquire a
            // reusable V2 migration authority because credential-free provider
            // identity cannot be proven from that spelling.
            return Self::isolated();
        }
        let mut digest = Sha256::new();
        digest.update(b"typebridge.orm.database-connection-authority/v1\0");
        digest.update(address.as_bytes());
        Self(DatabaseConnectionAuthorityKind::Provider(
            digest.finalize().into(),
        ))
    }
}

fn identity_safe_provider_address(address: &str) -> bool {
    !address.is_empty() && address.split(',').all(identity_safe_provider_endpoint)
}

fn identity_safe_provider_endpoint(endpoint: &str) -> bool {
    let (host_is_valid, port) = if let Some(bracketed) = endpoint.strip_prefix('[') {
        let Some((address, port)) = bracketed.split_once("]:") else {
            return false;
        };
        (
            !address.is_empty()
                && !port.contains(['[', ']', ':'])
                && address.parse::<Ipv6Addr>().is_ok(),
            port,
        )
    } else {
        let Some((host, port)) = endpoint.rsplit_once(':') else {
            return false;
        };
        let host = host.strip_suffix('.').unwrap_or(host);
        (
            !host.is_empty()
                && host.len() <= 253
                && !host.contains(['[', ']', ':'])
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        && label
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .as_bytes()
                            .last()
                            .is_some_and(u8::is_ascii_alphanumeric)
                }),
            port,
        )
    };
    host_is_valid
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

impl PartialEq for DatabaseConnectionAuthority {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (
                DatabaseConnectionAuthorityKind::Provider(left),
                DatabaseConnectionAuthorityKind::Provider(right),
            ) => left == right,
            (
                DatabaseConnectionAuthorityKind::Custom(left),
                DatabaseConnectionAuthorityKind::Custom(right),
            ) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for DatabaseConnectionAuthority {}

impl fmt::Debug for DatabaseConnectionAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DatabaseConnectionAuthority([REDACTED])")
    }
}

impl Database {
    /// Create a Database with a custom backend (for testing).
    pub fn with_backend(backend: Box<dyn DriverBackend>, database_name: impl Into<String>) -> Self {
        Self::with_backend_authority(
            backend,
            database_name,
            DatabaseConnectionAuthority::isolated(),
        )
    }

    /// Create a database with an explicitly shared custom-backend authority.
    ///
    /// This additive constructor is intended for V2 migration embedders and
    /// tests that own both backend handles. Passing the same cloned authority
    /// is an assertion that both handles reach the same provider endpoint;
    /// [`Self::with_backend`] deliberately gives every handle a distinct token.
    pub fn with_backend_authority(
        backend: Box<dyn DriverBackend>,
        database_name: impl Into<String>,
        connection_authority: DatabaseConnectionAuthority,
    ) -> Self {
        Self {
            backend,
            connection_authority,
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
            connection_authority: DatabaseConnectionAuthority::for_typedb_address(address),
            database_name: database.to_string(),
        })
    }

    /// Connect to a TypeDB server with an explicit typed TLS policy.
    ///
    /// Unlike the released [`Self::connect_with_options`] adapter, this entry
    /// point can select a custom root CA and preserves typed pre-I/O TLS
    /// configuration failures for language bindings.
    #[cfg(feature = "typedb")]
    pub async fn connect_secure_with_options(
        address: &str,
        database: &str,
        username: &str,
        password: &str,
        options: super::real_driver::SecureConnectOptions,
    ) -> super::real_driver::SecureResult<Self> {
        let backend =
            super::real_driver::RealBackend::connect_secure(address, username, password, options)
                .await?;
        Ok(Self {
            backend: Box::new(backend),
            connection_authority: DatabaseConnectionAuthority::for_typedb_address(address),
            database_name: database.to_string(),
        })
    }

    /// Connect with trust material prepared before credential resolution.
    #[cfg(feature = "typedb")]
    #[doc(hidden)]
    pub async fn connect_prepared_secure_with_options(
        address: &str,
        database: &str,
        username: &str,
        password: &str,
        options: super::real_driver::PreparedSecureConnectOptions,
    ) -> super::real_driver::SecureResult<Self> {
        let backend = super::real_driver::RealBackend::connect_prepared_secure(
            address, username, password, options,
        )
        .await?;
        Ok(Self {
            backend: Box::new(backend),
            connection_authority: DatabaseConnectionAuthority::for_typedb_address(address),
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

    /// Open a bounded read-capable transaction whose schema cannot change
    /// until the transaction closes.
    ///
    /// This V2-only admission seam is intentionally distinct from
    /// [`Self::read_transaction`]. Backends that cannot prove schema exclusion
    /// reject it instead of approximating the guarantee with observations.
    #[doc(hidden)]
    pub async fn schema_fenced_read_transaction(
        &self,
        timeout: Duration,
    ) -> Result<(Transaction, String)> {
        let fenced = self
            .backend
            .open_schema_fenced_read_transaction(&self.database_name, timeout)
            .await?;
        let (transaction, schema_text) = fenced.into_parts();
        Ok((Transaction::new(transaction, TxType::Write), schema_text))
    }

    /// Open an owned schema transaction.
    ///
    /// Migration coordinators use this form to execute bounded assertions and
    /// an ordered schema statement group before one explicit commit.
    pub async fn schema_transaction(&self) -> Result<Transaction> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, TxType::Schema)
            .await?;
        Ok(Transaction::new(tx, TxType::Schema))
    }

    /// Create a shared [`TransactionContext`] for grouping operations.
    pub async fn transaction_context(&self, tx_type: TxType) -> Result<TransactionContext> {
        let capabilities = self.backend.match_capabilities();
        let tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        Ok(TransactionContext::new(tx, tx_type, capabilities))
    }

    /// Execute one validated selected-row request in an owned read transaction.
    pub async fn execute_match(
        &self,
        registry: &DescriptorRegistry,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult> {
        self.execute_match_with_limits(registry, validated, MatchExecutionLimits::default())
            .await
    }

    /// Execute one validated selected-row request with caller-tightened limits.
    pub async fn execute_match_with_limits(
        &self,
        registry: &DescriptorRegistry,
        validated: &ValidatedMatchRequest,
        limits: MatchExecutionLimits,
    ) -> Result<ValidatedMatchResult> {
        SelectedResultExecutor::new(registry, self.backend.match_capabilities(), limits)
            .execute_compatible_owned(self, validated)
            .await
    }

    /// Execute through the retained direct V1 implementation for live parity tests.
    ///
    /// This test-only seam is intentionally unavailable in normal builds. It
    /// lets the integration corpus compare the released executor semantics
    /// with the production V1-to-V2 compatibility path without changing which
    /// path public callers use.
    #[cfg(feature = "integration-tests")]
    #[doc(hidden)]
    pub async fn execute_match_v1_legacy_for_live_test(
        &self,
        registry: &DescriptorRegistry,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult> {
        let registry = registry.owned_registry_snapshot()?;
        SelectedResultExecutor::new(
            &registry,
            self.backend.match_capabilities(),
            MatchExecutionLimits::default(),
        )
        .execute_owned(self, validated)
        .await
    }

    /// Get the database name.
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    pub(crate) fn execution_identity(&self) -> DatabaseExecutionIdentity {
        DatabaseExecutionIdentity {
            connection_authority: self.connection_authority.clone(),
            database_name: self.database_name.clone(),
        }
    }

    /// Return whether another handle carries the same opaque provider authority.
    #[must_use]
    pub fn shares_connection_authority_with(&self, other: &Self) -> bool {
        self.connection_authority == other.connection_authority
    }

    /// Check if the underlying connection is alive.
    pub fn is_connected(&self) -> bool {
        self.backend.is_open()
    }

    /// Explicitly close this database's provider connection.
    ///
    /// This is idempotent, makes provider admission terminal, and dispatches
    /// the backend's shutdown request. A backend may retain worker resources
    /// until its final shared lease is released.
    pub fn close(&self) -> Result<()> {
        self.backend.close_connection()
    }

    /// The server version detected at connect time, when known.
    ///
    /// `None` for backends without a version gate and whenever the negotiated
    /// connection path produced no authoritative server identity.
    pub fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.backend.server_version()
    }

    /// Return the shared legacy-server deprecation notice for this connection.
    ///
    /// Real TypeDB 3.8/3.10 connections and an unknown connection that
    /// negotiated the legacy band-7 fallback return the core-owned prose.
    /// Current negotiated bands, supported known versions, and custom
    /// backends return `None`.
    #[must_use]
    pub fn server_deprecation_notice(&self) -> Option<String> {
        self.backend.server_deprecation_notice()
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

    /// Whether both the connected server and the active negotiated provider
    /// support `given`-stage parameterized queries.
    ///
    /// `false` when the server predates 3.12 or its version is unknown
    /// (band-7 gRPC fallback) — callers with a per-row fallback should use
    /// it in both cases.
    pub fn supports_given_stage(&self) -> bool {
        use type_bridge_core_lib::version::{Feature, check_feature_supported};

        self.backend.supports_given_rows()
            && self
                .server_version()
                .is_some_and(|server| check_feature_supported(Feature::GivenStage, &server).is_ok())
    }

    /// Version-gate a `given`-stage query.
    ///
    /// When the detected server version predates 3.12, fail with an
    /// actionable versioned error instead of a server-side parse error.
    /// The server feature and negotiated provider transport are checked
    /// separately. A 3.12 server reached through a band-8 fallback therefore
    /// fails before opening a transaction instead of dispatching an operation
    /// the active driver cannot carry.
    pub fn check_given_stage_support(&self) -> Result<()> {
        use type_bridge_core_lib::version::{Feature, check_feature_supported};

        let Some(server) = self.server_version() else {
            return Err(crate::error::OrmError::QueryExecution(
                "given-stage support cannot be proven because the server version is unknown".into(),
            ));
        };
        check_feature_supported(Feature::GivenStage, &server)
            .map_err(crate::error::OrmError::UnsupportedVersion)?;
        if !self.backend.supports_given_rows() {
            return Err(crate::error::OrmError::QueryExecution(
                "given-stage input rows require an active band-9 provider; the connected server supports the syntax but the negotiated provider cannot transport rows"
                    .into(),
            ));
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

    /// Execute a `given`-stage TypeQL query over input rows, auto-managing
    /// the transaction lifecycle.
    ///
    /// One compiled pipeline runs over every input row; rows travel through
    /// the driver API, never the query string. Version-gated via
    /// [`Self::check_given_stage_support`] before any transaction is opened.
    #[tracing::instrument(skip(self, typeql, rows), fields(db = %self.database_name))]
    pub async fn execute_with_rows(
        &self,
        typeql: &str,
        tx_type: TxType,
        rows: GivenRowsSpec,
    ) -> Result<QueryResult> {
        self.check_given_stage_support()?;
        let mut tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        let result = match tx.query_with_rows(typeql, rows).await {
            Ok(result) => result,
            Err(error) => {
                if matches!(tx_type, TxType::Write | TxType::Schema) {
                    let _ = tx.rollback().await;
                }
                let _ = tx.close().await;
                return Err(error);
            }
        };
        if matches!(tx_type, TxType::Write | TxType::Schema)
            && let Err(error) = tx.commit().await
        {
            let _ = tx.close().await;
            return Err(error);
        }
        tx.close().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_authority_is_opaque_redacted_and_exact() {
        const SENTINEL: &str = "TB_AUTHORITY_SECRET_31d7";
        let first = DatabaseConnectionAuthority::for_typedb_address("provider.example:1729");
        let same = DatabaseConnectionAuthority::for_typedb_address("provider.example:1729");
        let different = DatabaseConnectionAuthority::for_typedb_address("provider.example:1730");

        assert_eq!(first, same);
        assert_ne!(first, different);
        let rendered = format!("{first:?}");
        assert_eq!(rendered, "DatabaseConnectionAuthority([REDACTED])");
        assert!(!rendered.contains(SENTINEL));
        assert!(!rendered.contains("provider.example"));

        let unsafe_address = format!("admin:{SENTINEL}@provider.example:1729");
        let unsafe_first = DatabaseConnectionAuthority::for_typedb_address(&unsafe_address);
        let unsafe_second = DatabaseConnectionAuthority::for_typedb_address(&unsafe_address);
        assert_ne!(unsafe_first, unsafe_second);
        assert!(!format!("{unsafe_first:?}").contains(SENTINEL));

        let isolated = DatabaseConnectionAuthority::isolated();
        assert_eq!(isolated, isolated.clone());
        assert_ne!(isolated, DatabaseConnectionAuthority::isolated());
    }
}
