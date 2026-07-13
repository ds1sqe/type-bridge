//! Session layer for TypeDB connectivity.
//!
//! Provides [`Database`] for connecting to TypeDB, [`Transaction`] for
//! individual operations, and [`TransactionContext`] for grouping multiple
//! operations into a shared transaction.

pub mod backend;
pub mod context;
pub mod database;
pub mod transaction;

#[cfg(feature = "typedb")]
pub mod real_driver;

pub use backend::{GivenRowsSpec, GivenValue, TxType};
pub use context::TransactionContext;
pub use database::Database;
pub use transaction::Transaction;

#[cfg(feature = "typedb")]
pub use real_driver::ConnectOptions;
#[cfg(feature = "typedb")]
pub use real_driver::embedded_driver_versions;
#[cfg(feature = "typedb")]
pub use real_driver::ensure_database_exists;
