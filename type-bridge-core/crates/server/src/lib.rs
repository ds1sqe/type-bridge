#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(missing_docs)]

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
//! | `v2-query` | yes | Additive prepared `/v2/capabilities` and `/v2/query` routes |
//!
//! Disable defaults with `--no-default-features` to use the core pipeline as
//! a library without any transport or backend.
//!
//! ## Library usage
//!
//! This example is ignored because the executor and interceptor values are
//! application-defined extension points rather than crate-provided fixtures.
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

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub mod readme_doctests {}

/// Standalone server configuration and bounded material loading.
pub mod config;
/// Errors returned by the query pipeline and transports.
pub mod error;
/// Provider-independent query execution interfaces.
pub mod executor;
/// Request, response, audit, and CRUD interceptor APIs.
pub mod interceptor;
/// The transport-independent query execution pipeline.
pub mod pipeline;
/// Schema-loading interfaces and built-in sources.
pub mod schema_source;

#[cfg(feature = "axum-transport")]
/// Axum HTTP request and response transports.
pub mod transport;

#[cfg(feature = "typedb")]
/// TypeDB-backed query executor implementation.
pub mod typedb;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_helpers;
