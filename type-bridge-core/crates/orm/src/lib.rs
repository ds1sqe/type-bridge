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
pub mod match_request;
pub mod migration_assertion;
pub mod provider_runtime;
pub mod query;
pub mod query_v2;
#[cfg(any(test, feature = "integration-tests"))]
mod query_v2_adapter;
#[cfg(test)]
mod query_v2_adapter_tests;
pub mod query_v2_prepared;
pub mod query_v2_remote;
pub mod registry;
pub mod relation;
pub mod runtime_projection;
pub mod schema;
pub mod session;
pub mod value;

/// Test-only bridge for exercising the crate-internal V1-to-V2 adapter from
/// the live integration-test crate.
///
/// This module exists only when the explicitly test-only `integration-tests`
/// feature is enabled. The adapter itself remains private and is absent from
/// normal/release builds.
#[cfg(feature = "integration-tests")]
#[doc(hidden)]
pub mod integration_test_support {
    use type_bridge_contract::diagnostic::Diagnostic;
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery};

    use crate::match_request::ValidatedMatchRequest;
    use crate::query_v2_adapter::adapt_match_request;
    use crate::registry::DescriptorRegistry;

    /// Adapt a validated V1 request for the live parity gate.
    pub fn adapt_match_request_for_live_test(
        validated: &ValidatedMatchRequest,
        registry: &DescriptorRegistry,
        context: &MigrationAssertionValidationContext<'_>,
        limits: StructuralLimits,
    ) -> Result<(ValidatedQuery, QueryOperation), Diagnostic> {
        let adapted = adapt_match_request(validated, registry, context, limits)?;
        Ok((adapted.validated().clone(), adapted.operation()))
    }
}

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
pub use error::{ClassifiedCommitError, CommitFailureCertainty, OrmError, Result};
pub use expr::{Agg, AggResult, Expr, GroupByResult, SortDir};
pub use field_ref::{FieldRef, RolePlayerFieldRef, RoleRef};
pub use filter::Filter;
pub use hooks::{
    CrudOperation, HookContext, HookError, HookRunner, LifecycleHook, PreHookResult, TypeKind,
};
pub use manager::{DynamicEntityManager, DynamicRelationManager, EntityManager, RelationManager};
pub use match_request::*;
pub use provider_runtime::ProviderRuntimeOwner;
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use registry::DescriptorRegistry;
pub use relation::{RoleInfo, RolePlayerRef, TypeBridgeRelation};
pub use runtime_projection::InstalledRuntimeProjection;
pub use schema::{SchemaDiff, SchemaInfo, SchemaManager};
pub use session::backend::AnswerCancellation;
#[cfg(feature = "typedb")]
pub use session::embedded_driver_versions;
#[cfg(feature = "typedb")]
pub use session::{
    ConnectOptions, PreparedSecureConnectOptions, SecureConnectError, SecureConnectOptions,
    SecureResult, TlsMode,
};
pub use session::{
    Database, DatabaseConnectionAuthority, GivenRowsSpec, GivenValue, Transaction,
    TransactionContext, TxType, require_legacy_writer_open,
    require_legacy_writer_open_in_transaction,
};
#[cfg(feature = "typedb")]
pub use session::{
    database_exists, database_exists_prepared_secure, database_exists_secure,
    delete_database_prepared_secure, delete_database_secure, ensure_database_exists,
    ensure_database_exists_prepared_secure, ensure_database_exists_secure,
};
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
