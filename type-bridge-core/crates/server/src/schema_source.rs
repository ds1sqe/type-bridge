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
            .map_err(|e| PipelineError::Schema(format!("Failed to parse schema: {}", e)))
    }
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
            .map_err(|e| PipelineError::Schema(format!("Failed to parse schema: {}", e)))
    }
}
