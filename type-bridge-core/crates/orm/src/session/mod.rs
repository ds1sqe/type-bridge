//! Session layer for TypeDB connectivity.
//!
//! Provides [`Database`] for connecting to TypeDB, [`Transaction`] for
//! staged plans, and [`TransactionContext`] for grouping operations in a
//! caller-owned transaction.

pub mod backend;
pub mod context;
pub mod database;
#[cfg(feature = "typedb")]
mod direct_connection;
#[cfg(feature = "internal-eager")]
#[doc(hidden)]
pub mod eager;
#[cfg(not(feature = "internal-eager"))]
pub(crate) mod eager;
mod legacy_writer;
mod proxy_connection;
pub mod raw;
pub mod staged;
pub(crate) mod staged_proxy;
mod tls_policy;
pub mod transaction;

#[cfg(feature = "typedb")]
pub mod real_driver;

pub use backend::{GivenColumnType, GivenRowsSpec, GivenValue, TxType};
pub use context::TransactionContext;
#[doc(hidden)]
pub use context::TransactionContextState;
pub use database::{
    Database, DatabaseConnectionAuthority, DatabaseCreateOutcome, DatabaseDeleteOutcome,
};
#[cfg(feature = "typedb")]
pub use direct_connection::DirectConnectionPolicy;
pub use proxy_connection::{ProxyEndpoint, ProxyHttpTransport};
pub use staged::Transaction;
pub use staged_proxy::{StagedExchangeFn, StagedTransport};
pub use tls_policy::TlsPolicy;
pub use transaction::OwnedTransaction;

#[cfg(feature = "typedb")]
pub use real_driver::embedded_driver_versions;
#[cfg(feature = "typedb")]
pub use real_driver::{
    ConnectOptions, PreparedSecureConnectOptions, SecureConnectError, SecureConnectOptions,
    SecureResult, TlsMode,
};
#[cfg(feature = "typedb")]
pub use real_driver::{
    database_exists, database_exists_prepared_secure, database_exists_secure,
    delete_database_prepared_secure, delete_database_secure, ensure_database_exists,
    ensure_database_exists_prepared_secure, ensure_database_exists_secure,
};
