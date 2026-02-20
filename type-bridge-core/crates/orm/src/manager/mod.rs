//! Entity and relation managers with supporting modules.
//!
//! The [`EntityManager`] and [`RelationManager`] provide typed CRUD
//! operations. Query building and result hydration are handled by
//! internal helpers.

pub mod entity_manager;
pub mod hydration;
pub mod query_builder;
pub mod relation_manager;

pub use entity_manager::EntityManager;
pub use relation_manager::RelationManager;
