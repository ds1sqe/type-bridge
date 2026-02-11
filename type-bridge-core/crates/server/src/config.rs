use serde::Deserialize;

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
}

#[derive(Debug, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields used once TypeDB driver is integrated
pub struct TypeDBSection {
    pub address: String,
    pub database: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default = "default_password")]
    pub password: String,
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
        let content = std::fs::read_to_string(path)?;
        let config: ServerConfig = toml::from_str(&content)?;
        Ok(config)
    }
}
