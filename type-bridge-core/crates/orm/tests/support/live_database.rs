//! Fresh live databases, with an optional runner-owned name prefix.

pub fn name(stem: &str) -> String {
    let prefix =
        std::env::var("TYPE_BRIDGE_RUST_INTG_DATABASE").unwrap_or_else(|_| "type_bridge".into());
    format!(
        "{prefix}_{stem}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos()
    )
}

#[test]
fn live_database_names_are_fresh_and_keep_the_runner_prefix() {
    let first = name("probe");
    let second = name("probe");
    assert_ne!(first, second);
    let prefix =
        std::env::var("TYPE_BRIDGE_RUST_INTG_DATABASE").unwrap_or_else(|_| "type_bridge".into());
    assert!(first.starts_with(&format!("{prefix}_probe_")));
}
