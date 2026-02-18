use type_bridge_core_lib::schema::TypeSchema;

use crate::error::PipelineError;

/// Source of a TypeDB schema for the query pipeline.
///
/// Implement this trait to load a schema from any source: a file on disk,
/// an in-memory string, a remote URL, or even directly from TypeDB.
///
/// # Example
///
/// ```rust,ignore
/// use type_bridge_server::{SchemaSource, PipelineError};
/// use type_bridge_core_lib::schema::TypeSchema;
///
/// struct RemoteSchemaSource { url: String }
///
/// impl SchemaSource for RemoteSchemaSource {
///     fn load(&self) -> Result<TypeSchema, PipelineError> {
///         let content = fetch_schema(&self.url)?;
///         TypeSchema::from_typeql(&content)
///             .map_err(|e| PipelineError::Schema(e.to_string()))
///     }
/// }
/// ```
pub trait SchemaSource: Send + Sync {
    /// Load and parse the schema, returning a `TypeSchema`.
    fn load(&self) -> Result<TypeSchema, PipelineError>;
}

/// Load a schema from a TypeQL file on disk.
pub struct FileSchemaSource {
    path: String,
}

impl FileSchemaSource {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl SchemaSource for FileSchemaSource {
    fn load(&self) -> Result<TypeSchema, PipelineError> {
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| PipelineError::Schema(format!("Failed to read schema file '{}': {}", self.path, e)))?;
        TypeSchema::from_typeql(&content)
            .map_err(parse_schema_error)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_schema_error(e: impl std::fmt::Display) -> PipelineError {
    PipelineError::Schema(format!("Failed to parse schema: {}", e))
}

/// Load a schema from an in-memory TypeQL string.
///
/// Useful for testing or when the schema is embedded in the application.
pub struct InMemorySchemaSource {
    typeql: String,
}

impl InMemorySchemaSource {
    pub fn new(typeql: impl Into<String>) -> Self {
        Self { typeql: typeql.into() }
    }
}

impl SchemaSource for InMemorySchemaSource {
    fn load(&self) -> Result<TypeSchema, PipelineError> {
        TypeSchema::from_typeql(&self.typeql)
            .map_err(parse_schema_error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const VALID_SCHEMA: &str = r#"
define
    attribute name, value string;
    entity person, owns name;
"#;

    const COMPLEX_SCHEMA: &str = r#"
define
    attribute name, value string;
    attribute age, value long;
    attribute email, value string;
    entity person,
        owns name @key,
        owns age,
        owns email;
    entity employee sub person;
"#;

    // --- FileSchemaSource tests ---

    #[test]
    fn file_schema_source_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.tql");
        std::fs::write(&path, VALID_SCHEMA).unwrap();

        let source = FileSchemaSource::new(path.to_str().unwrap());
        let schema = source.load().unwrap();
        assert!(schema.entities.contains_key("person"));
    }

    #[test]
    fn file_schema_source_missing_file() {
        let source = FileSchemaSource::new("/nonexistent/schema.tql");
        let err = source.load().unwrap_err();
        assert!(matches!(&err, PipelineError::Schema(msg) if msg.contains("Failed to read schema file")));
    }

    #[test]
    fn file_schema_source_invalid_typeql() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.tql");
        std::fs::write(&path, "this is not valid typeql").unwrap();

        let source = FileSchemaSource::new(path.to_str().unwrap());
        let err = source.load().unwrap_err();
        assert!(matches!(&err, PipelineError::Schema(msg) if msg.contains("Failed to parse schema")));
    }

    #[test]
    fn file_schema_source_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.tql");
        std::fs::write(&path, "").unwrap();

        let source = FileSchemaSource::new(path.to_str().unwrap());
        // Empty input parses to an empty schema (no entities/relations/attributes)
        let schema = source.load().unwrap();
        assert!(schema.entities.is_empty());
        assert!(schema.relations.is_empty());
        assert!(schema.attributes.is_empty());
    }

    #[test]
    fn file_schema_source_error_contains_path() {
        let source = FileSchemaSource::new("/some/specific/path.tql");
        let err = source.load().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/some/specific/path.tql"), "Error should contain the file path: {msg}");
    }

    // --- InMemorySchemaSource tests ---

    #[test]
    fn in_memory_schema_source_valid() {
        let source = InMemorySchemaSource::new(VALID_SCHEMA);
        let schema = source.load().unwrap();
        assert!(schema.entities.contains_key("person"));
    }

    #[test]
    fn in_memory_schema_source_invalid() {
        let source = InMemorySchemaSource::new("not valid typeql at all");
        let err = source.load().unwrap_err();
        assert!(matches!(&err, PipelineError::Schema(msg) if msg.contains("Failed to parse schema")));
    }

    #[test]
    fn in_memory_schema_source_empty() {
        let source = InMemorySchemaSource::new("");
        // Empty input parses to an empty schema
        let schema = source.load().unwrap();
        assert!(schema.entities.is_empty());
    }

    #[test]
    fn in_memory_schema_source_complex_schema() {
        let source = InMemorySchemaSource::new(COMPLEX_SCHEMA);
        let schema = source.load().unwrap();
        assert!(schema.entities.contains_key("person"));
        assert!(schema.entities.contains_key("employee"));
        assert!(schema.attributes.contains_key("name"));
        assert!(schema.attributes.contains_key("age"));
        assert!(schema.attributes.contains_key("email"));
    }

    // --- Constructor tests ---

    #[test]
    fn file_schema_source_new_stores_path() {
        let source = FileSchemaSource::new("/my/path.tql");
        assert_eq!(source.path, "/my/path.tql");
    }

    #[test]
    fn in_memory_schema_source_new_stores_typeql() {
        let source = InMemorySchemaSource::new("define attribute x, value string;");
        assert_eq!(source.typeql, "define attribute x, value string;");
    }

    // --- Trait object safety ---

    #[test]
    fn schema_source_trait_object_safety() {
        let source: Box<dyn SchemaSource> = Box::new(InMemorySchemaSource::new(VALID_SCHEMA));
        let schema = source.load().unwrap();
        assert!(schema.entities.contains_key("person"));
    }
}
