//! Semantic guard between the v1 ledger schema and its frozen rendering.
//!
//! `type_bridge_schema_compat::LEGACY_LEDGER_SCHEMA_TYPEQL` pins the exact
//! released rendering so offline crates can recognize the ledger without a
//! provider dependency. Keep those released bytes frozen while comparing
//! newly generated schema through the official AST. A semantic change is
//! a contract break, not a reason to update the constant.

use std::collections::BTreeSet;

use type_bridge_migration::migration_state_schema;
use type_bridge_schema_compat::{LEGACY_LEDGER_SCHEMA_TYPEQL, is_legacy_ledger_label};

#[test]
fn the_frozen_rendering_matches_the_canonical_state_schema() {
    let rendered = migration_state_schema()
        .to_typeql()
        .expect("canonical state schema renders");
    assert!(
        type_bridge_core_lib::__internal::equivalent(&rendered, LEGACY_LEDGER_SCHEMA_TYPEQL,)
            .unwrap()
    );
}

#[test]
fn the_frozen_label_predicate_matches_the_canonical_label_set() {
    let schema = migration_state_schema();
    let canonical: BTreeSet<&str> = schema
        .entities
        .keys()
        .chain(schema.relations.keys())
        .chain(schema.attributes.keys())
        .map(String::as_str)
        .collect();
    for label in &canonical {
        assert!(
            is_legacy_ledger_label(label),
            "missing frozen label {label}"
        );
    }
    assert!(!is_legacy_ledger_label("person"));
    assert!(!is_legacy_ledger_label("type_bridge_migration_custom"));
}
