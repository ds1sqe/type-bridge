//! Schema management: registration, generation, diffing, and synchronization.

pub mod annotations;
pub mod diff;
pub mod error;
pub mod generator;
pub mod info;
pub mod manager;

pub use diff::SchemaDiff;
pub use error::SchemaError;
pub use info::SchemaInfo;
pub use manager::SchemaManager;
