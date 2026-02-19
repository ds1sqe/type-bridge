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

pub use backend::TxType;
pub use context::TransactionContext;
pub use database::Database;
pub use transaction::Transaction;
