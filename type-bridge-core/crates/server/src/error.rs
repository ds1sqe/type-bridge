#[derive(Debug, thiserror::Error)]
/// Errors surfaced by configuration, validation, execution, and interception.
pub enum PipelineError {
    /// Server configuration could not be loaded or validated.
    #[error("Configuration error: {0}")]
    Config(String),
    /// The configured TypeDB endpoint could not be reached.
    #[error("TypeDB connection error: {0}")]
    Connection(String),
    /// The connected TypeDB server version is outside the supported matrix.
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(#[from] type_bridge_core_lib::version::VersionError),
    /// Provider execution rejected or failed the query.
    #[error("Query execution error: {0}")]
    QueryExecution(String),
    /// Static query validation failed.
    #[error("Validation error: {0}")]
    Validation(String),
    /// Query text or wire input could not be parsed.
    #[error("Parse error: {0}")]
    Parse(String),
    /// The configured schema could not be loaded or decoded.
    #[error("Schema error: {0}")]
    Schema(String),
    /// An interceptor rejected or failed the request.
    #[error("Interceptor error: {0}")]
    Interceptor(String),
    /// An unexpected internal invariant failed.
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

    #[test]
    fn display_unsupported_version_error() {
        use type_bridge_core_lib::version::{Version, VersionError};
        let inner = VersionError::Unsupported {
            component: "server",
            found: Version::new(2, 28, 0),
        };
        let e = PipelineError::UnsupportedVersion(inner);
        let msg = e.to_string();
        assert!(msg.contains("Unsupported version"), "missing prefix: {msg}");
        assert!(msg.contains("2.28.0"), "missing detected version: {msg}");
    }
}
