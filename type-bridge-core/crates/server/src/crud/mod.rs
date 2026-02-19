//! CRUD endpoints for entity and relation types.
//!
//! Provides REST-style HTTP endpoints that build TypeQL queries dynamically
//! from request parameters, using [`TypeSchema`](type_bridge_core_lib::schema::TypeSchema)
//! for validation. All operations flow through the [`QueryPipeline`](crate::pipeline::QueryPipeline),
//! getting validation, interceptor processing, and audit logging for free.
//!
//! # Routes
//!
//! | Method | Path | Handler |
//! |--------|------|---------|
//! | POST | `/entities/{type_name}` | Insert a new entity |
//! | GET | `/entities/{type_name}` | List entities with optional filters |
//! | GET | `/entities/{type_name}/{iid}` | Fetch entity by IID |
//! | PUT | `/entities/{type_name}/{iid}` | Update entity attributes |
//! | DELETE | `/entities/{type_name}/{iid}` | Delete entity by IID |
//! | POST | `/relations/{type_name}` | Insert a new relation |
//! | GET | `/relations/{type_name}` | List relations |
//! | DELETE | `/relations/{type_name}/{iid}` | Delete relation by IID |

pub mod builder;
#[cfg(feature = "axum-transport")]
pub mod handlers;
pub mod types;
