//! Session layer for TypeDB connectivity.
//!
//! Provides [`Database`] for connecting to TypeDB, [`Transaction`] for
//! individual operations, and [`TransactionContext`] for grouping multiple
//! operations into a shared transaction.

pub mod backend;
pub mod context;
pub mod database;
pub mod legacy_writer;
pub mod transaction;

#[cfg(feature = "typedb")]
pub mod real_driver;

pub use backend::{GivenRowsSpec, GivenValue, TxType};
pub use context::TransactionContext;
pub use database::{Database, DatabaseConnectionAuthority};
pub use legacy_writer::{require_legacy_writer_open, require_legacy_writer_open_in_transaction};
pub use transaction::Transaction;

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
