//! Async ORM for TypeDB built on `type-bridge-core-lib`.
//!
//! This crate provides:
//!
//! - **[`TypeBridgeEntity`]** trait for mapping Rust structs to TypeDB entity types
//! - **[`TypeBridgeRelation`]** trait for mapping Rust structs to TypeDB relation types
//! - **[`TypeBridgeAttribute`]** trait and [`define_attribute!`] macro for attribute types
//! - **[`EntityManager`]** / **[`RelationManager`]** for typed CRUD operations
//! - **[`Database`]** + [`Transaction`] + [`TransactionContext`] session layer
//! - **[`Filter`]** for equality-based queries
//!
//! # Quick start
//!
//! ```ignore
//! use type_bridge_orm::{
//!     define_attribute, Database, EntityManager, Filter,
//!     TypeBridgeEntity, OwnedAttributeInfo, AttributeValue,
//! };
//!
//! // Define attribute types
//! define_attribute!(Name, "name", "string");
//! define_attribute!(Age, "age", "long");
//!
//! // Define entity (manual impl; derive macros in a later phase)
//! struct Person { iid: Option<String>, name: Name, age: Age }
//! // impl TypeBridgeEntity for Person { ... }
//!
//! // CRUD operations
//! let db = Database::connect("localhost:1729", "mydb", "admin", "password").await?;
//! let manager = EntityManager::<Person>::new(&db);
//! manager.insert(&mut person).await?;
//! let people = manager.all().await?;
//! ```

pub mod attribute;
pub mod codegen;
pub mod entity;
pub mod error;
pub mod expr;
pub mod field_ref;
pub mod filter;
pub mod manager;
pub mod query;
pub mod relation;
pub mod schema;
pub mod session;
pub mod value;

// Re-exports for convenient access
pub use attribute::{TypeBridgeAttribute, ValueType};
pub use entity::{Annotation, OwnedAttributeInfo, TypeBridgeEntity};
pub use error::{OrmError, Result};
pub use expr::{Agg, AggResult, Expr, GroupByResult, SortDir};
pub use filter::Filter;
pub use manager::{EntityManager, RelationManager};
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use relation::{RoleInfo, RolePlayerRef, TypeBridgeRelation};
pub use session::{Database, Transaction, TransactionContext, TxType};
pub use schema::{SchemaDiff, SchemaInfo, SchemaManager};
pub use value::AttributeValue;
pub use field_ref::{FieldRef, RolePlayerFieldRef, RoleRef};

// Re-export derive macros when the `derive` feature is enabled.
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeAttribute as DeriveAttribute;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeEntity as DeriveEntity;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeRelation as DeriveRelation;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::include_schema;
