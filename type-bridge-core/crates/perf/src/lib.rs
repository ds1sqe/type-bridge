//! Private perf harness for TypeBridge.
//!
//! - [`stages`] aggregates the core's stage spans (`tb.orm.plan`,
//!   `tb.query.compile`, `tb.runtime.execute`, `tb.decode`, `tb.commit`)
//!   into exclusive nanoseconds per stage.
//! - The `type-bridge-perf-pipelines` binary runs the major pipelines
//!   through the public Rust SDK against a live TypeDB and reports raw
//!   samples; `python -m perf.harness.pipeline` turns them into a
//!   `perf-result/v1` file.
//!
//! The crate is never published and no shipped crate depends on it.

pub use type_bridge_perf_stages as stages;
