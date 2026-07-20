use serde::Deserialize;
use type_bridge_core_lib::version as core_version;

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub typedb: TypeDBSection,
    #[serde(default)]
    pub schema: SchemaSection,
    #[serde(default)]
    pub interceptors: InterceptorsSection,
    #[serde(default)]
    pub logging: LoggingSection,
    #[serde(default)]
    pub v2: V2Section,
}

/// Optional versioned V2 query surface served beside the V1 routes.
///
/// Requires a binary built with `--features v2-query`; enabling it on a
/// build without the feature aborts startup instead of silently serving
/// 404s.
#[derive(Debug, Default, Deserialize)]
pub struct V2Section {
    /// Serve `/v2/query` and `/v2/capabilities`.
    #[serde(default)]
    pub enabled: bool,
    /// Path to canonical declared-schema bytes (a generated
    /// `declared-schema.json`) V2 plans validate against.
    #[serde(default)]
    pub declared_schema_file: String,
    /// Exclusive managed scope identity plans bind against.
    #[serde(default)]
    pub scope: String,
    /// Semantic profile identifier, e.g. `typedb-3.12.1/v1`.
    #[serde(default)]
    pub profile: String,
}

#[derive(Debug, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct TypeDBSection {
    pub address: String,
    pub database: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default = "default_password")]
    pub password: String,
    /// Port of the TypeDB HTTP API on the same host as `address`; the
    /// connect-time version gate probes `/v1/version` here.
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    /// Exact TypeDB server version to validate when the HTTP API is disabled
    /// or unreachable. When set, the connect-time gate skips HTTP probing.
    #[serde(default)]
    pub server_version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SchemaSection {
    #[serde(default)]
    pub source_file: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct InterceptorsSection {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default, rename = "audit-log")]
    pub audit_log: Option<AuditLogConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuditLogConfig {
    #[serde(default = "default_audit_output")]
    pub output: String,
    #[serde(default)]
    pub file_path: String,
}

#[derive(Debug, Deserialize)]
pub struct LoggingSection {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_http_port() -> u16 {
    core_version::DEFAULT_HTTP_PORT
}

fn default_username() -> String {
    "admin".to_string()
}

fn default_password() -> String {
    "password".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

fn default_audit_output() -> String {
    "stdout".to_string()
}

impl ServerConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_file_with_env(path, |name| std::env::var(name).ok())
    }

    fn from_file_with_env<F>(path: &str, get_env: F) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let content = std::fs::read_to_string(path)?;
        let mut config: ServerConfig = toml::from_str(&content)?;
        config.apply_env_overrides_from(get_env)?;
        Ok(config)
    }

    fn apply_env_overrides_from<F>(
        &mut self,
        mut get_env: F,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        if let Some(address) = get_env("TYPEDB_ADDRESS") {
            self.typedb.address = address;
        }
        if let Some(database) = get_env("TYPEDB_DATABASE") {
            self.typedb.database = database;
        }
        if let Some(username) = get_env("TYPEDB_USERNAME") {
            self.typedb.username = username;
        }
        if let Some(password) = get_env("TYPEDB_PASSWORD") {
            self.typedb.password = password;
        }
        if let Some(raw) = get_env("TYPEDB_HTTP_PORT") {
            self.typedb.http_port = raw.parse::<u16>().map_err(|_| {
                format!("TYPEDB_HTTP_PORT must be a valid port number (0–65535), got {raw:?}")
            })?;
        }
        if let Some(server_version) = get_env("TYPEDB_SERVER_VERSION") {
            self.typedb.server_version = Some(server_version);
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const FULL_CONFIG: &str = r#"
[server]
host = "127.0.0.1"
port = 9090

[typedb]
address = "localhost:1729"
database = "mydb"
username = "root"
password = "secret"
server_version = "3.11.5"

[schema]
source_file = "schema.tql"

[interceptors]
enabled = ["audit-log"]

[interceptors.audit-log]
output = "file"
file_path = "/tmp/audit.log"

[logging]
level = "debug"
format = "text"
"#;

    const MINIMAL_CONFIG: &str = r#"
[server]

[typedb]
address = "localhost:1729"
database = "mydb"
"#;

    // --- from_file tests ---

    #[test]
    fn from_file_valid_full_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, FULL_CONFIG).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.typedb.address, "localhost:1729");
        assert_eq!(config.typedb.database, "mydb");
        assert_eq!(config.typedb.username, "root");
        assert_eq!(config.typedb.password, "secret");
        assert_eq!(config.typedb.server_version.as_deref(), Some("3.11.5"));
        assert_eq!(config.schema.source_file, "schema.tql");
        assert_eq!(config.interceptors.enabled, vec!["audit-log"]);
        let audit = config.interceptors.audit_log.unwrap();
        assert_eq!(audit.output, "file");
        assert_eq!(audit.file_path, "/tmp/audit.log");
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.format, "text");
    }

    #[test]
    fn v2_section_defaults_to_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert!(!config.v2.enabled);
        assert!(config.v2.declared_schema_file.is_empty());
    }

    #[test]
    fn v2_section_parses_when_configured() {
        let config_text = format!(
            "{MINIMAL_CONFIG}\n[v2]\nenabled = true\n\
             declared_schema_file = \"declared-schema.json\"\n\
             scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, config_text).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert!(config.v2.enabled);
        assert_eq!(config.v2.declared_schema_file, "declared-schema.json");
        assert_eq!(config.v2.scope, "prod");
        assert_eq!(config.v2.profile, "typedb-3.12.1/v1");
    }

    #[test]
    fn env_overrides_typedb_section() {
        let mut config: ServerConfig = toml::from_str(FULL_CONFIG).unwrap();
        config
            .apply_env_overrides_from(|name| match name {
                "TYPEDB_ADDRESS" => Some("typedb:1729".to_string()),
                "TYPEDB_DATABASE" => Some("docker_db".to_string()),
                "TYPEDB_USERNAME" => Some("docker_user".to_string()),
                "TYPEDB_PASSWORD" => Some("docker_pass".to_string()),
                _ => None,
            })
            .unwrap();

        assert_eq!(config.typedb.address, "typedb:1729");
        assert_eq!(config.typedb.database, "docker_db");
        assert_eq!(config.typedb.username, "docker_user");
        assert_eq!(config.typedb.password, "docker_pass");
    }

    #[test]
    fn from_file_valid_minimal_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        // Defaults should kick in
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.typedb.username, "admin");
        assert_eq!(config.typedb.password, "password");
        assert_eq!(config.typedb.server_version, None);
        assert_eq!(config.schema.source_file, "");
        assert!(config.interceptors.enabled.is_empty());
        assert!(config.interceptors.audit_log.is_none());
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.logging.format, "json");
    }

    #[test]
    fn from_file_missing_file() {
        let result = ServerConfig::from_file("/nonexistent/path/server.toml");
        assert!(result.is_err());
    }

    #[test]
    fn from_file_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not valid toml {{{}}}").unwrap();

        let result = ServerConfig::from_file(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn from_file_missing_required_typedb_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incomplete.toml");
        std::fs::write(&path, "[server]\n").unwrap();

        let result = ServerConfig::from_file(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn from_file_missing_required_typedb_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incomplete.toml");
        std::fs::write(&path, "[server]\n[typedb]\n").unwrap();

        let result = ServerConfig::from_file(path.to_str().unwrap());
        assert!(result.is_err()); // address and database are required
    }

    // --- Default function tests ---

    #[test]
    fn default_host_value() {
        assert_eq!(default_host(), "0.0.0.0");
    }

    #[test]
    fn default_port_value() {
        assert_eq!(default_port(), 8080);
    }

    #[test]
    fn default_username_value() {
        assert_eq!(default_username(), "admin");
    }

    #[test]
    fn default_password_value() {
        assert_eq!(default_password(), "password");
    }

    #[test]
    fn default_log_level_value() {
        assert_eq!(default_log_level(), "info");
    }

    #[test]
    fn default_log_format_value() {
        assert_eq!(default_log_format(), "json");
    }

    #[test]
    fn default_audit_output_value() {
        assert_eq!(default_audit_output(), "stdout");
    }

    // --- LoggingSection default ---

    #[test]
    fn logging_section_default() {
        let logging = LoggingSection::default();
        assert_eq!(logging.level, "info");
        assert_eq!(logging.format, "json");
    }

    // --- Serde deserialization edge cases ---

    #[test]
    fn server_section_custom_host_default_port() {
        let toml = r#"
[server]
host = "192.168.1.1"

[typedb]
address = "localhost:1729"
database = "db"
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.host, "192.168.1.1");
        assert_eq!(config.server.port, 8080); // default
    }

    #[test]
    fn server_section_custom_port_default_host() {
        let toml = r#"
[server]
port = 3000

[typedb]
address = "localhost:1729"
database = "db"
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.host, "0.0.0.0"); // default
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn typedb_section_custom_credentials() {
        let toml = r#"
[server]

[typedb]
address = "remote:1729"
database = "prod"
username = "superuser"
password = "hunter2"
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.typedb.username, "superuser");
        assert_eq!(config.typedb.password, "hunter2");
    }

    #[test]
    fn schema_section_default_when_missing() {
        let config: ServerConfig = toml::from_str(MINIMAL_CONFIG).unwrap();
        assert_eq!(config.schema.source_file, "");
    }

    #[test]
    fn schema_section_with_file() {
        let toml = r#"
[server]
[typedb]
address = "localhost:1729"
database = "db"
[schema]
source_file = "my_schema.tql"
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.schema.source_file, "my_schema.tql");
    }

    #[test]
    fn interceptors_enabled_empty_by_default() {
        let config: ServerConfig = toml::from_str(MINIMAL_CONFIG).unwrap();
        assert!(config.interceptors.enabled.is_empty());
        assert!(config.interceptors.audit_log.is_none());
    }

    #[test]
    fn interceptors_enabled_without_audit_config() {
        let toml = r#"
[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enabled = ["audit-log"]
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.interceptors.enabled, vec!["audit-log"]);
        assert!(config.interceptors.audit_log.is_none());
    }

    #[test]
    fn interceptors_with_audit_config() {
        let toml = r#"
[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enabled = ["audit-log"]
[interceptors.audit-log]
output = "file"
file_path = "/var/log/audit.jsonl"
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        let audit = config.interceptors.audit_log.unwrap();
        assert_eq!(audit.output, "file");
        assert_eq!(audit.file_path, "/var/log/audit.jsonl");
    }

    #[test]
    fn audit_log_config_defaults() {
        let toml = r#"
[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enabled = ["audit-log"]
[interceptors.audit-log]
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        let audit = config.interceptors.audit_log.unwrap();
        assert_eq!(audit.output, "stdout"); // default
        assert_eq!(audit.file_path, ""); // default
    }

    #[test]
    fn extra_fields_ignored() {
        let toml = r#"
[server]
host = "0.0.0.0"
unknown_field = "ignored"

[typedb]
address = "localhost:1729"
database = "db"
"#;
        // toml crate with serde ignores unknown fields by default
        let result: Result<ServerConfig, _> = toml::from_str(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn multiple_interceptors_enabled() {
        let toml = r#"
[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enabled = ["audit-log", "rate-limiter", "custom"]
"#;
        let config: ServerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.interceptors.enabled.len(), 3);
    }

    // --- TYPEDB_HTTP_PORT env override ---

    #[test]
    fn env_overrides_http_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |name| {
            if name == "TYPEDB_HTTP_PORT" {
                Some("9123".to_string())
            } else {
                None
            }
        })
        .unwrap();

        assert_eq!(config.typedb.http_port, 9123);
    }

    #[test]
    fn env_overrides_server_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();

        let config = ServerConfig::from_file_with_env(path.to_str().unwrap(), |name| {
            if name == "TYPEDB_SERVER_VERSION" {
                Some("3.10.4".to_string())
            } else {
                None
            }
        })
        .unwrap();

        assert_eq!(config.typedb.server_version.as_deref(), Some("3.10.4"));
    }

    #[test]
    fn env_invalid_http_port_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();

        let result = ServerConfig::from_file_with_env(path.to_str().unwrap(), |name| {
            if name == "TYPEDB_HTTP_PORT" {
                Some("not-a-port".to_string())
            } else {
                None
            }
        });

        assert!(
            result.is_err(),
            "invalid TYPEDB_HTTP_PORT must return an error"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("TYPEDB_HTTP_PORT"),
            "error message must mention TYPEDB_HTTP_PORT: {msg}"
        );
    }

    #[test]
    fn default_http_port_equals_ssot() {
        // Pins the server default against the core SSOT constant so any
        // divergence between the two fails at test time.
        use super::core_version;
        assert_eq!(
            default_http_port(),
            core_version::DEFAULT_HTTP_PORT,
            "server default_http_port() must equal core DEFAULT_HTTP_PORT"
        );
    }
}
