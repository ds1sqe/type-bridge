#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(missing_docs)]

//! Staged TypeDB transactions with composable host policy hooks.
//! The default build serves `/v3/staged`, `/v3/capabilities`, and `/health`.

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub mod readme_doctests {}

/// Standalone server configuration and bounded material loading.
pub mod config;
/// Errors returned by the query pipeline and transports.
pub mod error;
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
