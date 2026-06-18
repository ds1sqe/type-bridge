//! Code generator: TypeQL schema -> Rust source files.
//!
//! The generation policy lives in `type_bridge_core_lib::bindgen`; this module
//! keeps the historical `type_bridge_orm::codegen` entrypoints.

use type_bridge_core_lib::bindgen::{self, BindgenPlan};
use type_bridge_core_lib::bindgen::{
    BindgenOptions, GeneratedPackage, GeneratedRustModels, TargetLanguage,
};
use type_bridge_core_lib::schema::TypeSchema;

/// Generated Rust model source files.
pub type GeneratedModels = GeneratedRustModels;

/// Parse TypeQL and generate Rust model source code.
pub fn generate_from_typeql(input: &str) -> Result<GeneratedModels, String> {
    let plan = BindgenPlan::from_typeql(input)?;
    Ok(plan.render_rust_models())
}

/// Parse TypeQL and generate model files for any supported target language.
pub fn generate_for_target(
    input: &str,
    target: TargetLanguage,
    options: &BindgenOptions,
) -> Result<GeneratedPackage, String> {
    bindgen::generate_from_typeql(input, target, options)
}

/// Generate Rust model source from a resolved `TypeSchema`.
pub fn generate_rust_models(schema: &TypeSchema) -> GeneratedModels {
    BindgenPlan::from_schema(schema).render_rust_models()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_schema_tql() -> &'static str {
        "define\nattribute name, value string;\nattribute age, value long;\nentity person, owns name @key, owns age, plays employment:employee;"
    }

    fn relation_schema_tql() -> &'static str {
        "define\nattribute name, value string;\nattribute position, value string;\nentity person, owns name @key, plays employment:employee;\nentity company, owns name @key, plays employment:employer;\nrelation employment, relates employee, relates employer, owns position;"
    }

    #[test]
    fn generate_basic_attributes() {
        let models = generate_from_typeql(basic_schema_tql()).unwrap();
        assert!(
            models
                .attributes_rs
                .contains("define_attribute!(Age, \"age\", \"long\");")
        );
        assert!(
            models
                .attributes_rs
                .contains("define_attribute!(Name, \"name\", \"string\");")
        );
    }

    #[test]
    fn generate_basic_entity() {
        let models = generate_from_typeql(basic_schema_tql()).unwrap();
        assert!(models.entities_rs.contains("#[entity(name = \"person\")]"));
        assert!(models.entities_rs.contains("pub struct Person"));
        assert!(models.entities_rs.contains("#[field(key)]"));
    }

    #[test]
    fn generate_relation_with_roles() {
        let models = generate_from_typeql(relation_schema_tql()).unwrap();
        assert!(
            models
                .relations_rs
                .contains("#[relation(name = \"employment\")]")
        );
        assert!(models.relations_rs.contains("pub struct Employment"));
        assert!(models.relations_rs.contains("player_type = \"person\""));
        assert!(models.relations_rs.contains("player_type = \"company\""));
    }
}
