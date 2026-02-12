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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn display_config_error() {
        let e = PipelineError::Config("bad config".into());
        assert_eq!(e.to_string(), "Configuration error: bad config");
    }

    #[test]
    fn display_connection_error() {
        let e = PipelineError::Connection("refused".into());
        assert_eq!(e.to_string(), "TypeDB connection error: refused");
    }

    #[test]
    fn display_query_execution_error() {
        let e = PipelineError::QueryExecution("syntax error".into());
        assert_eq!(e.to_string(), "Query execution error: syntax error");
    }

    #[test]
    fn display_validation_error() {
        let e = PipelineError::Validation("invalid type".into());
        assert_eq!(e.to_string(), "Validation error: invalid type");
    }

    #[test]
    fn display_parse_error() {
        let e = PipelineError::Parse("unexpected token".into());
        assert_eq!(e.to_string(), "Parse error: unexpected token");
    }

    #[test]
    fn display_schema_error() {
        let e = PipelineError::Schema("file not found".into());
        assert_eq!(e.to_string(), "Schema error: file not found");
    }

    #[test]
    fn display_interceptor_error() {
        let e = PipelineError::Interceptor("access denied".into());
        assert_eq!(e.to_string(), "Interceptor error: access denied");
    }

    #[test]
    fn display_internal_error() {
        let e = PipelineError::Internal("unexpected".into());
        assert_eq!(e.to_string(), "Internal error: unexpected");
    }

    #[test]
    fn debug_format() {
        let e = PipelineError::Config("test".into());
        let debug = format!("{:?}", e);
        assert!(debug.contains("Config"));
    }
}
