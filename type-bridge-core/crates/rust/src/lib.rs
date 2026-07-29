//! Public Rust client library for TypeBridge.
//!
//! Provides top-level [`Database`], [`ConnectionOptions`], [`SchemaPackage`],
//! and owner-branded schema/model primitives.

#![forbid(unsafe_code)]

extern crate self as type_bridge;

pub mod __codegen;
pub mod aggregate;
#[allow(dead_code)]
mod entity_codec;
mod entity_manager;
pub mod error;
pub mod model;
mod query;
#[allow(dead_code)]
mod relation_codec;
mod relation_manager;
mod remote;
pub mod schema;
pub mod session;
mod transaction;
pub mod value;

pub use entity_manager::{EntityManager, EntitySubtypeManager};
pub use error::{Error, ModelValidationPhase, Result};
pub use query::{
    Binding, BoundField, BoundRole, Collected, Exact, GroupedQuery, NamedSelection, Order,
    OrderedOperand, Page, PageOptions, Predicate, Query, QueryOperand, QuerySession, RowsOptions,
    Selectable, SelectedRowSpec, SelectedShape, SelectedSlot, SelectionMode, SingularSelectedShape,
    Subtypes,
};
pub use relation_manager::{RelationManager, RelationSubtypeManager};
pub use remote::{
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits, RemoteQueryTransport,
};
pub use schema::{Schema, SchemaPackage, Unbound};
pub use session::{ConnectionOptions, Database};
pub use transaction::{
    ReadTransaction, TransactionEntityManager, TransactionRelationManager, WriteTransaction,
};
pub use type_bridge_orm_derive::SelectedRow;
