/// Generate an imported TypeQL `define` block.
///
/// `QueryCompiler`, `execute_query`, and `typedb_driver` are forbidden only as
/// executable binding policy, not as documentation.
pub fn generate_define_block() -> &'static str {
    r#"define entity person, owns name; attribute name, value string;"#
}

pub fn boundary_description() -> &'static str {
    "QueryCompiler execute_query typedb_driver are not owned here"
}
