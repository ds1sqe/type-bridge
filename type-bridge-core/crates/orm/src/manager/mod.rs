//! Entity manager and supporting modules.
//!
//! The [`EntityManager`] provides typed CRUD operations. Query building and
//! result hydration are handled by internal helpers.

pub mod entity_manager;
pub mod hydration;
pub mod query_builder;

pub use entity_manager::EntityManager;
