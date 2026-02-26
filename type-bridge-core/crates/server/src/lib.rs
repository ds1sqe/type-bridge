#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! # type-bridge-server
//!
//! Transport-agnostic query pipeline for TypeDB with composable interceptors.
//!
//! This crate provides both a library and a standalone binary for executing
//! TypeQL queries through a structured pipeline: **validate → intercept →
//! compile → execute → intercept**.
//!
//! ## Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `typedb` | yes | TypeDB backend via [`TypeDBClient`](typedb::TypeDBClient) |
//! | `axum-transport` | yes | HTTP server with `/query`, `/query/validate`, `/health`, `/schema` endpoints |
//!
//! Disable defaults with `--no-default-features` to use the core pipeline as
//! a library without any transport or backend.
//!
//! ## Library usage
//!
//! ```rust,ignore
//! use type_bridge_server::pipeline::PipelineBuilder;
//! use type_bridge_server::schema_source::InMemorySchemaSource;
//!
//! let pipeline = PipelineBuilder::new(my_executor)
//!     .with_schema_source(InMemorySchemaSource::new(tql_schema))
//!     .with_default_database("my_db")
//!     .with_interceptor(my_audit_log)
//!     .build()?;
//!
//! let output = pipeline.execute_query(input).await?;
//! ```
//!
//! ## Extension points
//!
//! - **[`QueryExecutor`](executor::QueryExecutor)** — implement to use a
//!   non-TypeDB backend or a mock.
//! - **[`Interceptor`](interceptor::Interceptor)** — implement to add
//!   cross-cutting concerns (audit, auth, rate limiting).
//! - **[`SchemaSource`](schema_source::SchemaSource)** — implement to load
//!   TypeQL schemas from custom sources.

pub mod config;
pub mod error;
pub mod executor;
pub mod interceptor;
pub mod pipeline;
pub mod schema_source;

#[cfg(feature = "axum-transport")]
pub mod transport;

#[cfg(feature = "typedb")]
pub mod typedb;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod test_helpers;
