//! Released schemas are immutable input data, not emitters.

#[test]
fn released_control_schemas_parse_through_the_official_adapter() {
    for source in [
        type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL,
        type_bridge_schema_compat::LEGACY_LEDGER_SCHEMA_TYPEQL,
    ] {
        type_bridge_core_lib::__internal::parse(source).expect("released schema input");
    }
}
