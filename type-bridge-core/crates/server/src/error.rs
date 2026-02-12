#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("TypeDB connection error: {0}")]
    Connection(String),
    #[error("Query execution error: {0}")]
    QueryExecution(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Schema error: {0}")]
    Schema(String),
    #[error("Interceptor error: {0}")]
    Interceptor(String),
    #[error("Internal error: {0}")]
    Internal(String),
}
