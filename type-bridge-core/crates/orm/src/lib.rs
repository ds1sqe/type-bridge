//! Async ORM for TypeDB built on `type-bridge-core-lib`.
//!
//! This crate provides:
//!
//! - **[`TypeBridgeEntity`]** trait for mapping Rust structs to TypeDB entity types
//! - **[`TypeBridgeRelation`]** trait for mapping Rust structs to TypeDB relation types
//! - **[`TypeBridgeAttribute`]** trait and [`define_attribute!`] macro for attribute types
//! - **[`EntityManager`]** / **[`RelationManager`]** for typed CRUD operations
//! - **[`DescriptorRegistry`]** plus dynamic managers for runtime schemas
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
//!
//! # Runtime descriptors
//!
//! Runtime descriptors are the shared Rust substrate for generated schemas and
//! language bindings. They are registered without a database and then used to
//! construct dynamic managers:
//!
//! ```ignore
//! use type_bridge_orm::{
//!     DescriptorRegistry, DynamicEntityManager, EntityDescriptor,
//!     OwnedAttributeDescriptor, ValueType,
//! };
//!
//! let registry = DescriptorRegistry::new();
//! let person = registry.register_entity(EntityDescriptor {
//!     type_name: "person".into(),
//!     is_abstract: false,
//!     parent_type: None,
//!     owned_attributes: vec![OwnedAttributeDescriptor {
//!         field_name: "name".into(),
//!         attr_name: "name".into(),
//!         value_type: ValueType::String,
//!         annotations: vec![],
//!         is_optional: false,
//!         is_ordered: false,
//!     }],
//! })?;
//!
//! let manager = DynamicEntityManager::new(&db, person);
//! ```

pub mod attribute;
pub mod codegen;
pub mod descriptor;
pub mod dynamic;
pub mod entity;
pub mod error;
pub mod expr;
pub mod field_ref;
pub mod filter;
pub mod hooks;
pub mod manager;
pub mod query;
pub mod registry;
pub mod relation;
pub mod schema;
pub mod session;
pub mod value;

// Re-exports for convenient access
pub use attribute::{TypeBridgeAttribute, ValueType};
pub use descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
    TypeDescriptorRef,
};
pub use dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp, DynamicEntityRow, DynamicExpr,
    DynamicRelationRow, DynamicRolePlayer, DynamicRolePlayerInput, DynamicSort,
};
pub use entity::{Annotation, OwnedAttributeInfo, TypeBridgeEntity};
pub use error::{OrmError, Result};
pub use expr::{Agg, AggResult, Expr, GroupByResult, SortDir};
pub use field_ref::{FieldRef, RolePlayerFieldRef, RoleRef};
pub use filter::Filter;
pub use hooks::{
    CrudOperation, HookContext, HookError, HookRunner, LifecycleHook, PreHookResult, TypeKind,
};
pub use manager::{DynamicEntityManager, DynamicRelationManager, EntityManager, RelationManager};
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use registry::DescriptorRegistry;
pub use relation::{RoleInfo, RolePlayerRef, TypeBridgeRelation};
pub use schema::{SchemaDiff, SchemaInfo, SchemaManager};
#[cfg(feature = "typedb")]
pub use session::ensure_database_exists;
#[cfg(feature = "typedb")]
pub use session::embedded_driver_versions;
pub use session::{Database, Transaction, TransactionContext, TxType};
pub use value::AttributeValue;

// Re-export derive macros when the `derive` feature is enabled.
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeAttribute as DeriveAttribute;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeEntity as DeriveEntity;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::TypeBridgeRelation as DeriveRelation;
#[cfg(feature = "derive")]
pub use type_bridge_orm_derive::include_schema;
