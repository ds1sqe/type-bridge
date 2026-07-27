//! Canonical TypeDB decimal parsing and semantic comparison.
//!
//! The implementation moved into the dependency-bottom contract crate. These
//! re-exports preserve the released `type_bridge_core_lib::decimal` paths.

pub use type_bridge_contract::decimal::{CanonicalDecimal, parse_decimal};
