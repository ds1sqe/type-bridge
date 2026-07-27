use std::ffi::OsString;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;

#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt as _;
use cap_fs_ext::{
    DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _, OpenOptionsMaybeDirExt as _,
};
use cap_std::ambient_authority;
#[cfg(windows)]
use cap_std::fs::OpenOptionsExt as _;
use cap_std::fs::{Dir, OpenOptions};
use serde::Deserialize;
use type_bridge_core_lib::version as core_version;

const MAX_TLS_MATERIAL_BYTES: u64 = 1024 * 1024;
const MAX_SERVER_CONFIG_BYTES: u64 = 1024 * 1024;
#[cfg(feature = "v2-query")]
const MAX_DECLARED_SCHEMA_BYTES: usize = type_bridge_contract::limits::MAX_CANONICAL_BYTES;

#[derive(Clone, Copy)]
enum RuntimeConfigParseErrorKind {
    Syntax,
    ValueShape,
    UnknownSecurityKey,
}

struct RuntimeConfigParseError {
    kind: RuntimeConfigParseErrorKind,
    location: Option<(usize, usize)>,
}

impl RuntimeConfigParseError {
    fn from_toml(kind: RuntimeConfigParseErrorKind, content: &str, error: toml::de::Error) -> Self {
        // Extract only numeric source coordinates. The TOML error owns the
        // original document and its Display text may reproduce a complete
        // secret-bearing line, so neither is retained as a source.
        let location = error
            .span()
            .and_then(|span| source_line_column(content, span.start));
        Self { kind, location }
    }

    const fn unknown_security_key() -> Self {
        Self {
            kind: RuntimeConfigParseErrorKind::UnknownSecurityKey,
            location: None,
        }
    }
}

impl fmt::Display for RuntimeConfigParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            RuntimeConfigParseErrorKind::Syntax => "TOML syntax",
            RuntimeConfigParseErrorKind::ValueShape => "value shape",
            RuntimeConfigParseErrorKind::UnknownSecurityKey => "unknown security-sensitive key",
        };
        write!(formatter, "server configuration is invalid ({reason})")?;
        if let Some((line, column)) = self.location {
            write!(formatter, " at line {line}, column {column}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for RuntimeConfigParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for RuntimeConfigParseError {}

fn source_line_column(content: &str, byte_offset: usize) -> Option<(usize, usize)> {
    if byte_offset > content.len() || !content.is_char_boundary(byte_offset) {
        return None;
    }
    let prefix = &content[..byte_offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, tail)| tail)
        .chars()
        .count()
        + 1;
    Some((line, column))
}

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

/// Standalone-server configuration with additive secure and V2 settings.
///
/// [`ServerConfig`] remains the released plaintext configuration surface so
/// downstream exhaustive struct literals and patterns keep compiling. The
/// standalone binary parses this projection instead; its private wire types
/// accept the same TOML tables while keeping new fields out of the released
/// structs.
pub struct RuntimeServerConfig {
    pub server: ServerSection,
    pub typedb: SecureTypeDBSection,
    pub schema: SchemaSection,
    pub interceptors: InterceptorsSection,
    pub logging: LoggingSection,
    pub inbound_tls: Option<InboundTlsSection>,
    pub v2: V2Section,
}

/// TypeDB connection settings for the standalone server's secure entry point.
///
/// The credential-bearing V1 projection is deliberately not publicly
/// reachable from a loaded runtime config:
///
/// ```compile_fail
/// use type_bridge_server::config::SecureTypeDBSection;
///
/// fn accidentally_log_raw_connection(config: &SecureTypeDBSection) {
///     let _ = format!("{:?}", config.connection);
/// }
/// ```
pub struct SecureTypeDBSection {
    /// Released connection fields, preserved as one exact V1 projection.
    pub(crate) connection: TypeDBSection,
    /// Validated transport mode used by both HTTP discovery and gRPC.
    pub tls_mode: OutboundTlsMode,
    // Relative custom roots are captured while the configuration directory
    // handle is retained. Transport preparation consumes these exact bytes
    // instead of reopening the diagnostic path below.
    #[cfg_attr(not(feature = "typedb"), allow(dead_code))]
    pub(crate) custom_root_ca_snapshot: Option<CapturedConfiguredMaterial>,
    // Runtime-only storage avoids changing the public V2 configuration
    // projection while binding the declared schema to this exact load.
    #[cfg(feature = "v2-query")]
    pub(crate) v2_declared_schema_snapshot: Option<CapturedConfiguredMaterial>,
}

impl fmt::Debug for RuntimeServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeServerConfig")
            .field("server", &self.server)
            .field("typedb", &self.typedb)
            .field("schema", &self.schema)
            .field("interceptors", &self.interceptors)
            .field("logging", &self.logging)
            .field("inbound_tls", &self.inbound_tls)
            .field("v2", &self.v2)
            .finish()
    }
}

impl fmt::Debug for SecureTypeDBSection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureTypeDBSection")
            .field("address", &"[REDACTED]")
            .field("database", &self.connection.database)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("http_port", &self.connection.http_port)
            .field("server_version", &self.connection.server_version)
            .field("tls_mode", &self.tls_mode)
            .field("custom_root_ca_snapshot", &self.custom_root_ca_snapshot)
            .finish()
    }
}

impl SecureTypeDBSection {
    /// Construct secure connection settings from an independently owned TLS
    /// policy. Custom-root paths are captured during transport preparation.
    #[must_use]
    pub fn new(connection: TypeDBSection, tls_mode: OutboundTlsMode) -> Self {
        Self {
            connection,
            tls_mode,
            custom_root_ca_snapshot: None,
            #[cfg(feature = "v2-query")]
            v2_declared_schema_snapshot: None,
        }
    }

    /// Return the non-secret database name selected by this runtime config.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.connection.database
    }
}

/// Authority mode for the standalone V2 query executor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum V2AuthorityMode {
    /// Require the complete managed migration-control partition and singleton.
    #[default]
    Managed,
    /// Require no V2 or legacy migration controls and bind to this database.
    QueryOnly,
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
    /// Exact authority contract; defaults to `managed`.
    #[serde(default)]
    pub authority_mode: V2AuthorityMode,
}

#[derive(Debug, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

/// Inbound server identity loaded and validated before binding a listener.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundTlsSection {
    #[serde(rename = "cert-path")]
    pub cert_path: PathBuf,
    #[serde(rename = "key-path")]
    pub key_path: PathBuf,
    #[serde(skip)]
    prepared: Option<PreparedInboundTlsMaterial>,
}

#[derive(Clone)]
pub(crate) struct CapturedConfiguredMaterial {
    pub(crate) path: PathBuf,
    pub(crate) bytes: Arc<[u8]>,
}

impl fmt::Debug for CapturedConfiguredMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedConfiguredMaterial")
            .field("path", &self.path)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[derive(Clone, Default)]
struct PreparedInboundTlsMaterial {
    certificate: Option<CapturedConfiguredMaterial>,
    private_key: Option<CapturedConfiguredMaterial>,
}

impl fmt::Debug for PreparedInboundTlsMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedInboundTlsMaterial")
            .field(
                "certificate_bytes",
                &self
                    .certificate
                    .as_ref()
                    .map(|material| material.bytes.len()),
            )
            .field(
                "private_key_bytes",
                &self
                    .private_key
                    .as_ref()
                    .map(|material| material.bytes.len()),
            )
            .finish()
    }
}

impl InboundTlsSection {
    /// Construct an inbound identity whose absolute or caller-owned paths are
    /// read when the transport identity is loaded.
    #[must_use]
    pub fn from_paths(cert_path: PathBuf, key_path: PathBuf) -> Self {
        Self {
            cert_path,
            key_path,
            prepared: None,
        }
    }
}

#[cfg(feature = "axum-transport")]
impl InboundTlsSection {
    /// Load and cross-check the certificate chain and private key before bind.
    pub async fn load(
        &self,
    ) -> Result<axum_server::tls_rustls::RustlsConfig, Box<dyn std::error::Error>> {
        fn read_bounded(path: &Path, field: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            use std::path::Component;

            use cap_fs_ext::{
                DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _,
                OpenOptionsMaybeDirExt as _, OpenOptionsSyncExt as _,
            };

            if !path.is_absolute() {
                return Err(format!("{field} must be an absolute resolved path").into());
            }
            let anchor = path
                .ancestors()
                .last()
                .ok_or_else(|| format!("{field} has no filesystem anchor"))?;
            let relative = path
                .strip_prefix(anchor)
                .map_err(|_| format!("{field} is not beneath its filesystem anchor"))?;
            let components = relative
                .components()
                .map(|component| match component {
                    Component::Normal(name) => Ok(name),
                    _ => Err(format!("{field} contains an invalid path component")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (name, parents) = components
                .split_last()
                .ok_or_else(|| format!("{field} has no file name"))?;
            let mut directory =
                cap_std::fs::Dir::open_ambient_dir(anchor, cap_std::ambient_authority())
                    .map_err(|error| format!("cannot read {field}: {error}"))?;
            for parent in parents {
                directory = directory
                    .open_dir_nofollow(parent)
                    .map_err(|error| format!("cannot read {field}: {error}"))?;
            }
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No).nonblock(true);
            // A directory must open (Windows needs backup semantics for
            // that) so the regular-file classification below comes from the
            // opened handle's metadata, exactly as it does on Unix.
            options.maybe_dir(true);
            let mut file = directory
                .open_with(name, &options)
                .map(cap_std::fs::File::into_std)
                .map_err(|error| format!("cannot read {field}: {error}"))?;
            let metadata = file
                .metadata()
                .map_err(|error| format!("cannot inspect {field}: {error}"))?;
            if !metadata.is_file() {
                return Err(format!("{field} must name a regular file").into());
            }
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_TLS_MATERIAL_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("cannot read {field}: {error}"))?;
            if bytes.is_empty()
                || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_TLS_MATERIAL_BYTES
            {
                return Err(
                    format!("{field} must be a non-empty file no larger than 1 MiB").into(),
                );
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|error| format!("cannot reread {field}: {error}"))?;
            let mut verification_bytes = Vec::new();
            (&mut file)
                .take(MAX_TLS_MATERIAL_BYTES + 1)
                .read_to_end(&mut verification_bytes)
                .map_err(|error| format!("cannot reread {field}: {error}"))?;
            let after = file
                .metadata()
                .map_err(|error| format!("cannot inspect {field}: {error}"))?;
            let timestamps_match = match (metadata.modified(), after.modified()) {
                (Ok(before), Ok(after)) => before == after,
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if metadata.len() != after.len()
                || metadata.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                || bytes != verification_bytes
                || !timestamps_match
            {
                return Err(format!("{field} changed while it was being read").into());
            }
            Ok(bytes)
        }

        fn captured_bytes(
            material: Option<&CapturedConfiguredMaterial>,
            current_path: &Path,
            field: &str,
        ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
            let Some(material) = material else {
                return Ok(None);
            };
            if material.path != current_path {
                return Err(format!(
                    "{field} changed after its relative material was captured; reload the configuration"
                )
                .into());
            }
            Ok(Some(material.bytes.to_vec()))
        }

        let certificate = match captured_bytes(
            self.prepared
                .as_ref()
                .and_then(|prepared| prepared.certificate.as_ref()),
            &self.cert_path,
            "server.tls.cert-path",
        )? {
            Some(bytes) => bytes,
            None => read_bounded(&self.cert_path, "server.tls.cert-path")?,
        };
        let private_key = match captured_bytes(
            self.prepared
                .as_ref()
                .and_then(|prepared| prepared.private_key.as_ref()),
            &self.key_path,
            "server.tls.key-path",
        )? {
            Some(bytes) => bytes,
            None => read_bounded(&self.key_path, "server.tls.key-path")?,
        };
        axum_server::tls_rustls::RustlsConfig::from_pem(certificate, private_key)
            .await
            .map_err(|error| format!("invalid server.tls identity: {error}").into())
    }
}

#[derive(Deserialize)]
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

impl fmt::Debug for TypeDBSection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypeDBSection")
            .field("address", &"[REDACTED]")
            .field("database", &self.database)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("http_port", &self.http_port)
            .field("server_version", &self.server_version)
            .finish()
    }
}

/// Validated outbound TLS policy independent of any driver implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundTlsMode {
    Disabled,
    NativeRoots,
    CustomRootCa(PathBuf),
}

#[derive(Debug, Deserialize)]
struct RuntimeServerConfigWire {
    server: RuntimeServerSectionWire,
    typedb: RuntimeTypeDBSectionWire,
    #[serde(default)]
    schema: RuntimeSchemaSectionWire,
    #[serde(default)]
    interceptors: RuntimeInterceptorsSectionWire,
    #[serde(default)]
    logging: RuntimeLoggingSectionWire,
    #[serde(default)]
    v2: RuntimeV2SectionWire,
}

#[derive(Debug, Deserialize)]
struct RuntimeServerSectionWire {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    tls: Option<InboundTlsSection>,
}

#[derive(Deserialize)]
struct RuntimeTypeDBSectionWire {
    address: String,
    database: String,
    #[serde(default = "default_username")]
    username: String,
    #[serde(default = "default_password")]
    password: String,
    #[serde(default = "default_http_port")]
    http_port: u16,
    #[serde(default)]
    server_version: Option<String>,
    #[serde(default)]
    tls: Option<bool>,
    #[serde(default, rename = "tls-root-ca")]
    tls_root_ca: Option<PathBuf>,
}

impl fmt::Debug for RuntimeTypeDBSectionWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeTypeDBSectionWire")
            .field("address", &"[REDACTED]")
            .field("database", &self.database)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("http_port", &self.http_port)
            .field("server_version", &self.server_version)
            .field("tls", &self.tls)
            .field("tls_root_ca", &self.tls_root_ca)
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
struct RuntimeSchemaSectionWire {
    #[serde(default)]
    source_file: String,
}

#[derive(Debug, Default, Deserialize)]
struct RuntimeInterceptorsSectionWire {
    #[serde(default)]
    enabled: Vec<String>,
    #[serde(default, rename = "audit-log")]
    audit_log: Option<RuntimeAuditLogConfigWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeAuditLogConfigWire {
    #[serde(default = "default_audit_output")]
    output: String,
    #[serde(default)]
    file_path: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeLoggingSectionWire {
    #[serde(default = "default_log_level")]
    level: String,
    #[serde(default = "default_log_format")]
    format: String,
}

impl Default for RuntimeLoggingSectionWire {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeV2SectionWire {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    declared_schema_file: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    authority_mode: V2AuthorityMode,
}

impl From<RuntimeSchemaSectionWire> for SchemaSection {
    fn from(wire: RuntimeSchemaSectionWire) -> Self {
        Self {
            source_file: wire.source_file,
        }
    }
}

impl From<RuntimeInterceptorsSectionWire> for InterceptorsSection {
    fn from(wire: RuntimeInterceptorsSectionWire) -> Self {
        Self {
            enabled: wire.enabled,
            audit_log: wire.audit_log.map(Into::into),
        }
    }
}

impl From<RuntimeAuditLogConfigWire> for AuditLogConfig {
    fn from(wire: RuntimeAuditLogConfigWire) -> Self {
        Self {
            output: wire.output,
            file_path: wire.file_path,
        }
    }
}

impl From<RuntimeLoggingSectionWire> for LoggingSection {
    fn from(wire: RuntimeLoggingSectionWire) -> Self {
        Self {
            level: wire.level,
            format: wire.format,
        }
    }
}

impl From<RuntimeV2SectionWire> for V2Section {
    fn from(wire: RuntimeV2SectionWire) -> Self {
        Self {
            enabled: wire.enabled,
            declared_schema_file: wire.declared_schema_file,
            scope: wire.scope,
            profile: wire.profile,
            authority_mode: wire.authority_mode,
        }
    }
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
    /// Load the released plaintext server configuration.
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

impl RuntimeServerConfig {
    /// Load the standalone runtime projection, including secure and V2 TOML.
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_file_with_env(path, |name| std::env::var(name).ok())
    }

    /// Return the immutable V2 declared-schema bytes captured by [`Self::from_file`].
    ///
    /// A caller that mutates the public diagnostic path after loading must
    /// reload the complete configuration instead of pairing it with stale
    /// schema authority.
    #[cfg(feature = "v2-query")]
    pub fn v2_declared_schema_bytes(&self) -> Result<Option<&[u8]>, &'static str> {
        let Some(material) = &self.typedb.v2_declared_schema_snapshot else {
            return Ok(None);
        };
        if material.path != Path::new(&self.v2.declared_schema_file) {
            return Err(
                "v2.declared_schema_file changed after its bytes were captured; reload the configuration",
            );
        }
        Ok(Some(material.bytes.as_ref()))
    }

    fn from_file_with_env<F>(path: &str, mut get_env: F) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        Self::from_file_with_env_after_probes(path, &mut get_env, || {}, || {})
    }

    #[cfg(test)]
    fn from_file_with_env_after_probe<F, H>(
        path: &str,
        mut get_env: F,
        after_probe: H,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Option<String>,
        H: FnOnce(),
    {
        Self::from_file_with_env_after_probes(path, &mut get_env, after_probe, || {})
    }

    fn from_file_with_env_after_probes<F, H, I>(
        path: &str,
        mut get_env: F,
        after_probe: H,
        after_authorized_read: I,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Option<String>,
        H: FnOnce(),
        I: FnOnce(),
    {
        let (mut wire, relative_authority) = load_runtime_wire(path, after_probe)?;
        // Tests use this seam to prove that every relative authority-bearing
        // runtime file is opened through the already-retained directory even
        // if its ambient name is replaced after the authoritative read.
        after_authorized_read();
        if let Some(address) = get_env("TYPEDB_ADDRESS") {
            wire.typedb.address = address;
        }
        if let Some(database) = get_env("TYPEDB_DATABASE") {
            wire.typedb.database = database;
        }
        if let Some(username) = get_env("TYPEDB_USERNAME") {
            wire.typedb.username = username;
        }
        if let Some(password) = get_env("TYPEDB_PASSWORD") {
            wire.typedb.password = password;
        }
        if let Some(raw) = get_env("TYPEDB_HTTP_PORT") {
            wire.typedb.http_port = raw.parse::<u16>().map_err(|_| {
                format!("TYPEDB_HTTP_PORT must be a valid port number (0–65535), got {raw:?}")
            })?;
        }
        if let Some(server_version) = get_env("TYPEDB_SERVER_VERSION") {
            wire.typedb.server_version = Some(server_version);
        }

        // Reject contradictions before resolving or opening any configured
        // trust or identity path.
        let tls_mode = outbound_tls_mode(wire.typedb.tls, wire.typedb.tls_root_ca.take())?;
        let mut custom_root_ca_snapshot = None;
        let tls_mode = match tls_mode {
            OutboundTlsMode::CustomRootCa(path) => {
                let resolved = resolve_configured_file(
                    relative_authority.as_ref(),
                    &path,
                    "typedb.tls-root-ca",
                )?;
                custom_root_ca_snapshot =
                    resolved.snapshot.map(|bytes| CapturedConfiguredMaterial {
                        path: resolved.path.clone(),
                        bytes,
                    });
                OutboundTlsMode::CustomRootCa(resolved.path)
            }
            other => other,
        };
        if let Some(tls) = &mut wire.server.tls {
            let certificate = resolve_configured_file(
                relative_authority.as_ref(),
                &tls.cert_path,
                "server.tls.cert-path",
            )?;
            let private_key = resolve_configured_file(
                relative_authority.as_ref(),
                &tls.key_path,
                "server.tls.key-path",
            )?;
            tls.cert_path = certificate.path;
            tls.key_path = private_key.path;
            if certificate.snapshot.is_some() || private_key.snapshot.is_some() {
                tls.prepared = Some(PreparedInboundTlsMaterial {
                    certificate: certificate
                        .snapshot
                        .map(|bytes| CapturedConfiguredMaterial {
                            path: tls.cert_path.clone(),
                            bytes,
                        }),
                    private_key: private_key
                        .snapshot
                        .map(|bytes| CapturedConfiguredMaterial {
                            path: tls.key_path.clone(),
                            bytes,
                        }),
                });
            }
        }

        #[cfg(feature = "v2-query")]
        let v2_declared_schema_snapshot = if wire.v2.enabled
            && !wire.v2.declared_schema_file.is_empty()
        {
            let configured_path = Path::new(&wire.v2.declared_schema_file);
            Some(CapturedConfiguredMaterial {
                path: configured_path.to_path_buf(),
                bytes: capture_declared_schema_file(relative_authority.as_ref(), configured_path)?,
            })
        } else {
            None
        };

        Ok(Self {
            server: ServerSection {
                host: wire.server.host,
                port: wire.server.port,
            },
            typedb: SecureTypeDBSection {
                connection: TypeDBSection {
                    address: wire.typedb.address,
                    database: wire.typedb.database,
                    username: wire.typedb.username,
                    password: wire.typedb.password,
                    http_port: wire.typedb.http_port,
                    server_version: wire.typedb.server_version,
                },
                tls_mode,
                custom_root_ca_snapshot,
                #[cfg(feature = "v2-query")]
                v2_declared_schema_snapshot,
            },
            schema: wire.schema.into(),
            interceptors: wire.interceptors.into(),
            logging: wire.logging.into(),
            inbound_tls: wire.server.tls,
            v2: wire.v2.into(),
        })
    }
}

fn read_runtime_config_path(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    #[cfg(windows)]
    {
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        // Backup semantics let a directory path open so the regular-file
        // classification comes from the opened handle's metadata, exactly
        // as it does on Unix.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.share_mode(FILE_SHARE_READ);
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("cannot read server configuration: {error}"))?;
    read_runtime_config_handle(file)
}

fn read_runtime_config_handle(file: std::fs::File) -> Result<String, Box<dyn std::error::Error>> {
    read_runtime_config_handle_with_hooks(file, || {}, || {})
}

#[cfg(test)]
fn read_runtime_config_handle_after_inspect<H>(
    file: std::fs::File,
    after_inspect: H,
) -> Result<String, Box<dyn std::error::Error>>
where
    H: FnOnce(),
{
    read_runtime_config_handle_with_hooks(file, after_inspect, || {})
}

fn read_runtime_config_handle_with_hooks<H, I>(
    mut file: std::fs::File,
    after_inspect: H,
    after_first_read: I,
) -> Result<String, Box<dyn std::error::Error>>
where
    H: FnOnce(),
    I: FnOnce(),
{
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect server configuration: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_SERVER_CONFIG_BYTES {
        return Err("server configuration must name a regular file no larger than 1 MiB".into());
    }
    after_inspect();
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_SERVER_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read server configuration: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SERVER_CONFIG_BYTES {
        return Err("server configuration must be no larger than 1 MiB".into());
    }
    after_first_read();
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("cannot reread server configuration: {error}"))?;
    let mut verification_bytes = Vec::new();
    (&mut file)
        .take(MAX_SERVER_CONFIG_BYTES + 1)
        .read_to_end(&mut verification_bytes)
        .map_err(|error| format!("cannot reread server configuration: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot inspect server configuration: {error}"))?;
    let timestamps_match = match (metadata.modified(), after.modified()) {
        (Ok(before), Ok(after)) => before == after,
        (Err(_), Err(_)) => true,
        _ => false,
    };
    if metadata.len() != after.len()
        || metadata.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || bytes != verification_bytes
        || !timestamps_match
    {
        return Err("server configuration changed while it was being read".into());
    }
    String::from_utf8(bytes).map_err(|_| "server configuration must be valid UTF-8".into())
}

fn load_runtime_wire<H>(
    path: &str,
    after_probe: H,
) -> Result<(RuntimeServerConfigWire, Option<RelativeConfigAuthority>), Box<dyn std::error::Error>>
where
    H: FnOnce(),
{
    let content = read_runtime_config_path(Path::new(path))?;
    let wire = parse_runtime_wire(&content)?;
    // Preserve fail-fast contradiction handling: a relative path must not
    // cause any additional pathname lookup when the policy itself is invalid.
    outbound_tls_mode(wire.typedb.tls, wire.typedb.tls_root_ca.clone())?;
    after_probe();

    if !wire_uses_relative_runtime_paths(&wire) {
        return Ok((wire, None));
    }

    // The preliminary bytes only determine whether compatibility permits a
    // single read. Once relative security material is present, resolve the
    // configuration identity first and use bytes read through that resolved
    // path. This prevents a configuration symlink swap from pairing one
    // target's policy with another target's base directory.
    let resolved_config = Path::new(path).canonicalize()?;
    let authority = RelativeConfigAuthority::open(&resolved_config)?;
    let content = authority.read_config()?;
    let wire = parse_runtime_wire(&content)?;
    outbound_tls_mode(wire.typedb.tls, wire.typedb.tls_root_ca.clone())?;
    let relative_authority = if wire_uses_relative_runtime_paths(&wire) {
        Some(authority)
    } else {
        None
    };
    Ok((wire, relative_authority))
}

struct RelativeConfigAuthority {
    directory: Dir,
    ancestors: Vec<Dir>,
    config_name: OsString,
    display_base: PathBuf,
}

impl RelativeConfigAuthority {
    fn open(resolved_config: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let display_base = resolved_config
            .parent()
            .ok_or("server configuration path has no parent directory")?
            .to_path_buf();
        let config_name = resolved_config
            .file_name()
            .map(OsString::from)
            .ok_or("server configuration path has no file name")?;
        // Open every absolute component from a retained root handle. Keeping
        // the complete lineage makes `../` relative paths stable too: a later
        // rename cannot cause `open_parent_dir` to discover a different
        // ambient parent.
        let mut components = display_base.components();
        let mut anchor = PathBuf::new();
        let mut saw_prefix = false;
        let mut saw_root = false;
        loop {
            match components.clone().next() {
                Some(Component::Prefix(prefix)) if !saw_prefix && !saw_root => {
                    anchor.push(prefix.as_os_str());
                    saw_prefix = true;
                    let _ = components.next();
                }
                Some(Component::RootDir) if !saw_root => {
                    anchor.push(Component::RootDir.as_os_str());
                    saw_root = true;
                    let _ = components.next();
                }
                _ => break,
            }
        }
        if !saw_root {
            return Err("resolved server configuration directory is not absolute".into());
        }
        let mut directory = Dir::open_ambient_dir(&anchor, ambient_authority())?;
        let mut ancestors = Vec::new();
        for component in components {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => {
                    let child = directory.open_dir_nofollow(name)?;
                    ancestors.push(directory);
                    directory = child;
                }
                Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                    return Err(
                        "resolved server configuration directory has an invalid component".into(),
                    );
                }
            }
        }
        Ok(Self {
            directory,
            ancestors,
            config_name,
            display_base,
        })
    }

    fn read_config(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        // A directory must open (Windows needs backup semantics for that)
        // so the regular-file classification comes from the opened handle's
        // metadata, exactly as it does on Unix.
        options.maybe_dir(true);
        #[cfg(unix)]
        options.nonblock(true);
        #[cfg(windows)]
        {
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            options.share_mode(FILE_SHARE_READ);
        }
        let file = self
            .directory
            .open_with(Path::new(&self.config_name), &options)
            .map(cap_std::fs::File::into_std)?;
        read_runtime_config_handle(file)
    }

    fn open_relative_file_nofollow(
        &self,
        path: &Path,
        field: &str,
    ) -> Result<std::fs::File, Box<dyn std::error::Error>> {
        if path.as_os_str().is_empty() || path.is_absolute() {
            return Err(format!("{field} must be a non-empty relative path").into());
        }
        let file_name = path
            .file_name()
            .map(OsString::from)
            .ok_or_else(|| format!("{field} must name a file"))?;
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        let mut directory = self
            .directory
            .try_clone()
            .map_err(|error| format!("cannot resolve {field}: {error}"))?;
        let mut ancestors = self
            .ancestors
            .iter()
            .map(Dir::try_clone)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot resolve {field}: {error}"))?;
        for component in parent.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    directory = ancestors.pop().ok_or_else(|| {
                        format!("cannot resolve {field}: path escapes the filesystem root")
                    })?;
                }
                Component::Normal(name) => {
                    let child = directory
                        .open_dir_nofollow(name)
                        .map_err(|error| format!("cannot resolve {field}: {error}"))?;
                    ancestors.push(directory);
                    directory = child;
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(format!("{field} contains an invalid path component").into());
                }
            }
        }

        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        // A directory must open (Windows needs backup semantics for that)
        // so the regular-file classification comes from the opened handle's
        // metadata, exactly as it does on Unix.
        options.maybe_dir(true);
        #[cfg(unix)]
        options.nonblock(true);
        #[cfg(windows)]
        {
            // Permit only concurrent readers while the bounded snapshot is
            // captured; replacement and writes remain denied on Windows.
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            options.share_mode(FILE_SHARE_READ);
        }
        directory
            .open_with(Path::new(&file_name), &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|error| format!("cannot resolve {field}: {error}").into())
    }

    fn capture_relative_file(
        &self,
        path: &Path,
        field: &str,
    ) -> Result<ResolvedConfiguredFile, Box<dyn std::error::Error>> {
        let mut file = self.open_relative_file_nofollow(path, field)?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect {field}: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("{field} must name a regular file").into());
        }
        if metadata.len() == 0 || metadata.len() > MAX_TLS_MATERIAL_BYTES {
            return Err(format!("{field} must be a non-empty file no larger than 1 MiB").into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_TLS_MATERIAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {field}: {error}"))?;
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_TLS_MATERIAL_BYTES
        {
            return Err(format!("{field} must be a non-empty file no larger than 1 MiB").into());
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("cannot reread {field}: {error}"))?;
        let mut verification_bytes = Vec::new();
        (&mut file)
            .take(MAX_TLS_MATERIAL_BYTES + 1)
            .read_to_end(&mut verification_bytes)
            .map_err(|error| format!("cannot reread {field}: {error}"))?;
        let after = file
            .metadata()
            .map_err(|error| format!("cannot inspect {field}: {error}"))?;
        let timestamps_match = match (metadata.modified(), after.modified()) {
            (Ok(before), Ok(after)) => before == after,
            (Err(_), Err(_)) => true,
            _ => false,
        };
        if metadata.len() != after.len()
            || metadata.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            || bytes != verification_bytes
            || !timestamps_match
        {
            return Err(format!("{field} changed while it was being read").into());
        }
        Ok(ResolvedConfiguredFile {
            // This path remains a diagnostic projection only. Consumers use
            // `snapshot` for relative files, so an ambient parent replacement
            // cannot redirect a later TLS read through this name.
            path: self.display_base.join(path),
            snapshot: Some(bytes.into()),
        })
    }

    #[cfg(feature = "v2-query")]
    fn capture_relative_declared_schema(
        &self,
        path: &Path,
    ) -> Result<Arc<[u8]>, Box<dyn std::error::Error>> {
        let file = self.open_relative_file_nofollow(path, "v2.declared_schema_file")?;
        capture_declared_schema_handle(file)
    }
}

#[cfg(feature = "v2-query")]
fn capture_declared_schema_file(
    relative_authority: Option<&RelativeConfigAuthority>,
    path: &Path,
) -> Result<Arc<[u8]>, Box<dyn std::error::Error>> {
    if path.is_absolute() {
        return capture_absolute_declared_schema_file(path);
    }
    relative_authority
        .ok_or(
            "cannot resolve relative v2.declared_schema_file without the configuration directory",
        )?
        .capture_relative_declared_schema(path)
}

#[cfg(feature = "v2-query")]
fn capture_absolute_declared_schema_file(
    path: &Path,
) -> Result<Arc<[u8]>, Box<dyn std::error::Error>> {
    let field = "v2.declared_schema_file";
    let anchor = path
        .ancestors()
        .last()
        .ok_or_else(|| format!("{field} has no filesystem anchor"))?;
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| format!("{field} is not beneath its filesystem anchor"))?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(format!("{field} contains an invalid path component")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (name, parents) = components
        .split_last()
        .ok_or_else(|| format!("{field} has no file name"))?;
    let mut directory = Dir::open_ambient_dir(anchor, ambient_authority())
        .map_err(|error| format!("cannot resolve {field}: {error}"))?;
    for parent in parents {
        directory = directory
            .open_dir_nofollow(parent)
            .map_err(|error| format!("cannot resolve {field}: {error}"))?;
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    // A directory must open (Windows needs backup semantics for that) so
    // the regular-file classification comes from the opened handle's
    // metadata, exactly as it does on Unix.
    options.maybe_dir(true);
    #[cfg(unix)]
    options.nonblock(true);
    #[cfg(windows)]
    {
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    let file = directory
        .open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|error| format!("cannot resolve {field}: {error}"))?;
    capture_declared_schema_handle(file)
}

#[cfg(feature = "v2-query")]
fn capture_declared_schema_handle(
    mut file: std::fs::File,
) -> Result<Arc<[u8]>, Box<dyn std::error::Error>> {
    let field = "v2.declared_schema_file";
    let ceiling = u64::try_from(MAX_DECLARED_SCHEMA_BYTES)
        .map_err(|_| "canonical declared-schema byte ceiling is not representable")?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {field}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{field} must name a regular file").into());
    }
    if metadata.len() == 0 || metadata.len() > ceiling {
        return Err(format!("{field} must be a non-empty file no larger than 16 MiB").into());
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(ceiling + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {field}: {error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_DECLARED_SCHEMA_BYTES {
        return Err(format!("{field} must be a non-empty file no larger than 16 MiB").into());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("cannot reread {field}: {error}"))?;
    let mut verification_bytes = Vec::new();
    (&mut file)
        .take(ceiling + 1)
        .read_to_end(&mut verification_bytes)
        .map_err(|error| format!("cannot reread {field}: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot inspect {field}: {error}"))?;
    let timestamps_match = match (metadata.modified(), after.modified()) {
        (Ok(before), Ok(after)) => before == after,
        (Err(_), Err(_)) => true,
        _ => false,
    };
    if metadata.len() != after.len()
        || metadata.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || bytes != verification_bytes
        || !timestamps_match
    {
        return Err(format!("{field} changed while it was being read").into());
    }
    Ok(bytes.into())
}

fn parse_runtime_wire(
    content: &str,
) -> Result<RuntimeServerConfigWire, Box<dyn std::error::Error>> {
    reject_ambiguous_security_keys(content)?;
    toml::from_str(content).map_err(|error| {
        RuntimeConfigParseError::from_toml(RuntimeConfigParseErrorKind::ValueShape, content, error)
            .into()
    })
}

fn wire_uses_relative_runtime_paths(wire: &RuntimeServerConfigWire) -> bool {
    let uses_relative_tls_path = wire
        .typedb
        .tls_root_ca
        .as_deref()
        .is_some_and(|path| !path.is_absolute())
        || wire
            .server
            .tls
            .as_ref()
            .is_some_and(|tls| !tls.cert_path.is_absolute() || !tls.key_path.is_absolute());
    #[cfg(feature = "v2-query")]
    {
        uses_relative_tls_path
            || (wire.v2.enabled
                && !wire.v2.declared_schema_file.is_empty()
                && !Path::new(&wire.v2.declared_schema_file).is_absolute())
    }
    #[cfg(not(feature = "v2-query"))]
    {
        uses_relative_tls_path
    }
}

/// Preserve the released parser's tolerance for extension keys without
/// allowing a misspelled TLS option to be silently ignored as plaintext.
fn reject_ambiguous_security_keys(content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let document: toml::Value = toml::from_str(content).map_err(|error| {
        RuntimeConfigParseError::from_toml(RuntimeConfigParseErrorKind::Syntax, content, error)
    })?;
    let Some(root) = document.as_table() else {
        return Ok(());
    };
    for key in root.keys() {
        let path = [key.clone()];
        if security_shaped_key(key) && !documented_security_path(&path) {
            return Err(RuntimeConfigParseError::unknown_security_key().into());
        }
    }
    for namespace in ["server", "typedb"] {
        let Some(table) = root.get(namespace).and_then(toml::Value::as_table) else {
            continue;
        };
        for key in table.keys() {
            let path = [namespace.to_owned(), key.clone()];
            if security_shaped_key(key) && !documented_security_path(&path) {
                return Err(RuntimeConfigParseError::unknown_security_key().into());
            }
        }
    }
    Ok(())
}

fn documented_security_path(path: &[String]) -> bool {
    matches!(
        path,
        [server, tls]
            if server == "server" && tls == "tls"
    ) || matches!(
        path,
        [server, tls, field]
            if server == "server"
                && tls == "tls"
                && matches!(field.as_str(), "cert-path" | "key-path")
    ) || matches!(
        path,
        [typedb, field]
            if typedb == "typedb" && matches!(field.as_str(), "tls" | "tls-root-ca")
    )
}

fn security_shaped_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.starts_with("tls")
        || normalized.ends_with("tls")
        || normalized.starts_with("ssl")
        || normalized.ends_with("ssl")
        || normalized.starts_with("https")
        || normalized.ends_with("https")
        || matches!(
            normalized.as_str(),
            "cafile"
                | "capath"
                | "rootca"
                | "rootcapath"
                | "truststore"
                | "truststorepath"
                | "certfile"
                | "certificatepath"
                | "certpath"
                | "clientcert"
                | "clientcertpath"
                | "keyfile"
                | "keypath"
                | "privatekey"
                | "privatekeypath"
        )
}

impl OutboundTlsMode {
    /// Return whether this policy requires TLS on every outbound transport.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

fn outbound_tls_mode(
    tls: Option<bool>,
    root: Option<PathBuf>,
) -> Result<OutboundTlsMode, Box<dyn std::error::Error>> {
    match (tls, root) {
        (None | Some(false), None) => Ok(OutboundTlsMode::Disabled),
        (Some(true), None) => Ok(OutboundTlsMode::NativeRoots),
        (Some(true), Some(path)) => Ok(OutboundTlsMode::CustomRootCa(path)),
        (Some(false), Some(_)) => Err("typedb.tls-root-ca contradicts typedb.tls = false".into()),
        (None, Some(_)) => Err("typedb.tls-root-ca requires explicit typedb.tls = true".into()),
    }
}

struct ResolvedConfiguredFile {
    path: PathBuf,
    snapshot: Option<Arc<[u8]>>,
}

fn capture_tls_material_handle(
    mut file: std::fs::File,
    field: &str,
) -> Result<Arc<[u8]>, Box<dyn std::error::Error>> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {field}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{field} must name a regular file").into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_TLS_MATERIAL_BYTES {
        return Err(format!("{field} must be a non-empty file no larger than 1 MiB").into());
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_TLS_MATERIAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {field}: {error}"))?;
    if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_TLS_MATERIAL_BYTES {
        return Err(format!("{field} must be a non-empty file no larger than 1 MiB").into());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("cannot reread {field}: {error}"))?;
    let mut verification_bytes = Vec::new();
    (&mut file)
        .take(MAX_TLS_MATERIAL_BYTES + 1)
        .read_to_end(&mut verification_bytes)
        .map_err(|error| format!("cannot reread {field}: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot inspect {field}: {error}"))?;
    let timestamps_match = match (metadata.modified(), after.modified()) {
        (Ok(before), Ok(after)) => before == after,
        (Err(_), Err(_)) => true,
        _ => false,
    };
    if metadata.len() != after.len()
        || metadata.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || bytes != verification_bytes
        || !timestamps_match
    {
        return Err(format!("{field} changed while it was being read").into());
    }
    Ok(bytes.into())
}

fn capture_absolute_configured_file(
    path: &Path,
    field: &str,
) -> Result<ResolvedConfiguredFile, Box<dyn std::error::Error>> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(format!("{field} must be a non-empty absolute path").into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    #[cfg(windows)]
    {
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        // Backup semantics let a directory path open so the regular-file
        // classification comes from the opened handle's metadata, exactly
        // as it does on Unix.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.share_mode(FILE_SHARE_READ);
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("cannot resolve {field}: {error}"))?;
    let snapshot = capture_tls_material_handle(file, field)?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {field}: {error}"))?;
    Ok(ResolvedConfiguredFile {
        path: canonical,
        snapshot: Some(snapshot),
    })
}

fn resolve_configured_file(
    relative_authority: Option<&RelativeConfigAuthority>,
    path: &Path,
    field: &str,
) -> Result<ResolvedConfiguredFile, Box<dyn std::error::Error>> {
    if path.is_absolute() {
        return capture_absolute_configured_file(path, field);
    }
    let authority = relative_authority.ok_or_else(|| {
        format!("cannot resolve relative {field} without the configuration directory")
    })?;
    authority.capture_relative_file(path, field)
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

        let config =
            RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert!(!config.v2.enabled);
        assert!(config.v2.declared_schema_file.is_empty());
        assert_eq!(config.v2.authority_mode, V2AuthorityMode::Managed);
        assert_eq!(config.typedb.tls_mode, OutboundTlsMode::Disabled);
        assert!(config.inbound_tls.is_none());
    }

    #[test]
    fn v2_section_parses_when_configured() {
        let config_text = format!(
            "{MINIMAL_CONFIG}\n[v2]\nenabled = true\n\
             declared_schema_file = \"declared-schema.json\"\n\
             scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n\
             authority_mode = \"query_only\"\n"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, config_text).unwrap();
        #[cfg(feature = "v2-query")]
        std::fs::write(dir.path().join("declared-schema.json"), "captured schema").unwrap();

        let config =
            RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert!(config.v2.enabled);
        assert_eq!(config.v2.declared_schema_file, "declared-schema.json");
        assert_eq!(config.v2.scope, "prod");
        assert_eq!(config.v2.profile, "typedb-3.12.1/v1");
        assert_eq!(config.v2.authority_mode, V2AuthorityMode::QueryOnly);
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn relative_v2_declared_schema_is_config_relative_and_snapshot_stable() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("schemas");
        std::fs::create_dir(&schemas).unwrap();
        let declared_path = schemas.join("declared-schema.json");
        let original = b"captured declared schema";
        std::fs::write(&declared_path, original).unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                "{MINIMAL_CONFIG}\n[schema]\nsource_file = \"released-schema.tql\"\n\
                 [v2]\nenabled = true\ndeclared_schema_file = \"schemas/declared-schema.json\"\n\
                 scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n"
            ),
        )
        .unwrap();

        let mut config =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .unwrap();
        assert_eq!(
            config.v2.declared_schema_file,
            "schemas/declared-schema.json"
        );
        assert_eq!(config.schema.source_file, "released-schema.tql");
        assert_eq!(
            config.v2_declared_schema_bytes().unwrap(),
            Some(original.as_slice())
        );

        std::fs::write(&declared_path, "ambient replacement").unwrap();
        assert_eq!(
            config.v2_declared_schema_bytes().unwrap(),
            Some(original.as_slice()),
            "post-load replacement must not change V2 schema authority"
        );

        config.v2.declared_schema_file = "other.json".to_owned();
        let error = config
            .v2_declared_schema_bytes()
            .expect_err("a mutated public path must not detach the captured bytes");
        assert!(error.contains("changed after"), "{error}");
    }

    #[cfg(all(feature = "v2-query", unix))]
    #[test]
    fn relative_v2_declared_schema_uses_retained_base_after_ambient_parent_swap() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("active");
        let retained = dir.path().join("retained");
        std::fs::create_dir_all(active.join("schemas")).unwrap();
        let original = b"original declared schema";
        std::fs::write(active.join("schemas/declared-schema.json"), original).unwrap();
        let config_text = format!(
            "{MINIMAL_CONFIG}\n[v2]\nenabled = true\n\
             declared_schema_file = \"schemas/declared-schema.json\"\n\
             scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n"
        );
        let config_path = active.join("server.toml");
        std::fs::write(&config_path, &config_text).unwrap();
        let active_for_swap = active.clone();
        let retained_for_swap = retained.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probes(
            config_path.to_str().unwrap(),
            |_| None,
            || {},
            move || {
                std::fs::rename(&active_for_swap, &retained_for_swap).unwrap();
                std::fs::create_dir_all(active_for_swap.join("schemas")).unwrap();
                std::fs::write(
                    active_for_swap.join("schemas/declared-schema.json"),
                    "replacement declared schema",
                )
                .unwrap();
                std::fs::write(active_for_swap.join("server.toml"), config_text).unwrap();
            },
        )
        .unwrap();

        assert_eq!(
            config.v2_declared_schema_bytes().unwrap(),
            Some(original.as_slice()),
            "ambient parent replacement must not redirect the retained config authority"
        );
        assert_eq!(
            std::fs::read(active.join("schemas/declared-schema.json")).unwrap(),
            b"replacement declared schema"
        );
    }

    #[cfg(all(feature = "v2-query", unix))]
    #[test]
    fn absolute_v2_declared_schema_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.json");
        let declared_path = dir.path().join("declared-schema.json");
        std::fs::write(&target, "target schema").unwrap();
        symlink(&target, &declared_path).unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                "{MINIMAL_CONFIG}\n[v2]\nenabled = true\ndeclared_schema_file = {:?}\n\
                 scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n",
                declared_path.to_str().unwrap()
            ),
        )
        .unwrap();

        let error =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .expect_err("the final V2 schema component must not follow symlinks");
        assert!(
            error.to_string().contains("v2.declared_schema_file"),
            "{error}"
        );
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn non_regular_v2_declared_schema_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("declared-schema.json")).unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                "{MINIMAL_CONFIG}\n[v2]\nenabled = true\n\
                 declared_schema_file = \"declared-schema.json\"\n\
                 scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n"
            ),
        )
        .unwrap();

        let error =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .expect_err("a V2 schema directory must fail during config loading");
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn oversized_v2_declared_schema_is_rejected_at_the_canonical_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let declared_path = dir.path().join("declared-schema.json");
        let file = std::fs::File::create(&declared_path).unwrap();
        file.set_len(u64::try_from(MAX_DECLARED_SCHEMA_BYTES).unwrap() + 1)
            .unwrap();
        // Close the writable fixture handle before loading: the capture
        // opens the file denying concurrent writers, so a live writer
        // handle on Windows is a sharing violation, not a size failure.
        drop(file);
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                "{MINIMAL_CONFIG}\n[v2]\nenabled = true\n\
                 declared_schema_file = \"declared-schema.json\"\n\
                 scope = \"prod\"\nprofile = \"typedb-3.12.1/v1\"\n"
            ),
        )
        .unwrap();

        let error =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .expect_err("an oversized V2 schema must fail during config loading");
        assert!(
            error.to_string().contains("no larger than 16 MiB"),
            "{error}"
        );
    }

    #[test]
    fn runtime_wire_preserves_released_unknown_field_tolerance() {
        let cases = [
            (
                "root",
                r#"unexpected = true
[server]
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "server",
                r#"[server]
vendor_extension = true
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "typedb",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
vendor_extension = true
"#,
            ),
            (
                "schema",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[schema]
source-file = "schema.tql"
"#,
            ),
            (
                "interceptors",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enable = ["audit-log"]
"#,
            ),
            (
                "audit-log",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[interceptors]
enabled = ["audit-log"]
[interceptors.audit-log]
file-path = "audit.jsonl"
"#,
            ),
            (
                "logging",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[logging]
log-level = "debug"
"#,
            ),
        ];

        for (level, source) in cases {
            toml::from_str::<RuntimeServerConfigWire>(source).unwrap_or_else(|error| {
                panic!("released {level} extension key regressed: {error}")
            });
        }
    }

    #[test]
    fn new_security_sections_remain_closed_to_unknown_keys() {
        let cases = [
            r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[v2]
enable = true
"#,
            r#"[server]
[server.tls]
cert-path = "server.pem"
key-path = "server.key"
certificate-path = "ignored.pem"
[typedb]
address = "localhost:1729"
database = "db"
"#,
        ];
        for source in cases {
            let error = toml::from_str::<RuntimeServerConfigWire>(source)
                .expect_err("new security-sensitive sections must fail closed");
            assert!(error.to_string().contains("unknown field"), "{error}");
        }
    }

    #[test]
    fn ambiguous_tls_aliases_cannot_silently_select_plaintext() {
        let cases = [
            (
                "server tls alias",
                r#"[server]
tls-enabled = true
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "typedb tls alias",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
tls_enabled = true
"#,
            ),
            (
                "root CA alias",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
ca-file = "root.pem"
"#,
            ),
            (
                "hyphenated top-level table",
                r#"[server-tls]
enabled = true
[server]
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "underscored top-level table",
                r#"[server_tls]
enabled = true
[server]
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "top-level TLS table",
                r#"[tls]
enabled = true
[server]
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
            (
                "nested SSL table",
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
[typedb.ssl]
enabled = true
"#,
            ),
            (
                "top-level SSL flag",
                r#"ssl = true
[server]
[typedb]
address = "localhost:1729"
database = "db"
"#,
            ),
        ];

        let dir = tempfile::tempdir().unwrap();
        for (index, (name, source)) in cases.into_iter().enumerate() {
            let path = dir.path().join(format!("ambiguous-{index}.toml"));
            std::fs::write(&path, source).unwrap();
            let error = RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None)
                .unwrap_err();
            assert!(
                error.to_string().contains("security-sensitive"),
                "{name} was not rejected by the downgrade guard: {error}"
            );
        }
    }

    #[test]
    fn runtime_toml_errors_and_debug_never_retain_secret_source_values() {
        const SENTINEL: &str = "TB_SECRET_SENTINEL_7b4f";

        let syntax = format!(
            "[server]\n[typedb]\naddress = \"localhost:1729\"\ndatabase = \"db\"\npassword = \"{SENTINEL}\n"
        );
        let wrong_password = format!(
            "[server]\n[typedb]\naddress = \"localhost:1729\"\ndatabase = \"db\"\npassword = [\"{SENTINEL}\"]\n"
        );
        let wrong_username = format!(
            "[server]\n[typedb]\naddress = \"localhost:1729\"\ndatabase = \"db\"\nusername = {{ secret = \"{SENTINEL}\" }}\n"
        );
        let unknown_security_key = format!(
            "[server]\n[typedb]\naddress = \"localhost:1729\"\ndatabase = \"db\"\ntls-{SENTINEL} = true\n"
        );

        for (name, source) in [
            ("syntax", syntax),
            ("wrong password type", wrong_password),
            ("wrong username type", wrong_username),
            ("unknown security key", unknown_security_key),
        ] {
            let error = parse_runtime_wire(&source).expect_err(name);
            let rendered = format!("{error}\n{error:?}");
            assert!(!rendered.contains(SENTINEL), "{name}: {rendered}");
            assert!(
                error.source().is_none(),
                "{name} retained a source error that could expose TOML bytes"
            );
            assert!(
                rendered.contains("server configuration is invalid"),
                "{name}: {rendered}"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            format!(
                "[server]\n[typedb]\naddress = \"admin:{SENTINEL}@localhost:1729\"\ndatabase = \"db\"\nusername = \"{SENTINEL}\"\npassword = \"{SENTINEL}\"\n"
            ),
        )
        .unwrap();
        let config = RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None)
            .expect("valid runtime config");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains(SENTINEL), "{rendered}");
        assert!(rendered.contains("[REDACTED]"));
        let typedb_rendered = format!("{:?}", config.typedb);
        assert!(!typedb_rendered.contains(SENTINEL), "{typedb_rendered}");
        assert!(typedb_rendered.contains("[REDACTED]"));

        let wire = parse_runtime_wire(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(!format!("{wire:?}").contains(SENTINEL));
    }

    #[test]
    fn unrelated_legacy_extension_keys_remain_tolerated() {
        let source = r#"[server]
vendor_extension = true
[typedb]
address = "localhost:1729"
database = "db"
[legacy]
keyboard_layout = "dvorak"
hassle = false
monkey = "capuchin"
uncertainty = 0.1
[legacy.transport]
key = "request-id"
cert = "third-party.pem"
private-key = "third-party.key"
trust-store = "third-party.pem"
[logging.labels]
key = "request-id"
cert = "classification"
api-key = "redacted"
cache-key = "server-v1"
"#;
        reject_ambiguous_security_keys(source).unwrap();
    }

    #[test]
    fn released_top_level_and_known_namespace_extension_keys_remain_tolerated() {
        let source = r#"api-key = "redacted"
[server]
cache-key = "server-v1"
key = "request-id"
cert = "classification"
[typedb]
address = "localhost:1729"
database = "db"
api-key = "redacted"
certificate = "extension-metadata"
"#;
        reject_ambiguous_security_keys(source).unwrap();
    }

    #[test]
    fn common_tls_typos_and_aliases_remain_downgrade_guarded() {
        for key in [
            "tls_enable",
            "enable_tls",
            "tls-root",
            "ssl-ca",
            "tls-cert-file",
            "key-path",
            "trust-store",
        ] {
            let source = format!(
                "[server]\n[typedb]\naddress = \"localhost:1729\"\ndatabase = \"db\"\n{key} = \"ignored\"\n"
            );
            let error = reject_ambiguous_security_keys(&source)
                .expect_err("TLS-shaped direct keys must not be ignored");
            assert!(
                error.to_string().contains("security-sensitive"),
                "{key}: {error}"
            );
        }
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
    fn outbound_tls_truth_table_rejects_implicit_enablement() {
        let parse =
            |tls: Option<bool>, root: Option<&str>| outbound_tls_mode(tls, root.map(PathBuf::from));
        assert_eq!(parse(None, None).unwrap(), OutboundTlsMode::Disabled);
        assert_eq!(parse(Some(false), None).unwrap(), OutboundTlsMode::Disabled,);
        assert_eq!(
            parse(Some(true), None).unwrap(),
            OutboundTlsMode::NativeRoots,
        );
        assert!(parse(None, Some("root.pem")).is_err());
        assert!(parse(Some(false), Some("root.pem")).is_err());
        assert!(matches!(
            parse(Some(true), Some("root.pem")).unwrap(),
            OutboundTlsMode::CustomRootCa(path) if path == Path::new("root.pem")
        ));
    }

    #[test]
    fn tls_paths_resolve_against_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("certs")).unwrap();
        for name in ["ca.pem", "server.pem", "server.key"] {
            std::fs::write(dir.path().join("certs").join(name), "test-only material").unwrap();
        }
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            r#"[server]
[server.tls]
cert-path = "certs/server.pem"
key-path = "certs/server.key"
[typedb]
address = "localhost:1729"
database = "db"
tls = true
tls-root-ca = "certs/ca.pem"
"#,
        )
        .unwrap();

        let config =
            RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None).unwrap();
        assert!(matches!(
            config.typedb.tls_mode,
            OutboundTlsMode::CustomRootCa(ref path) if path.is_absolute()
        ));
        let inbound = config.inbound_tls.unwrap();
        assert!(inbound.cert_path.is_absolute());
        assert!(inbound.key_path.is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn relative_tls_config_symlink_swap_cannot_mix_policy_and_base() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        for (root, database) in [(&first, "first-db"), (&second, "second-db")] {
            std::fs::write(root.join("root.pem"), database).unwrap();
            std::fs::write(
                root.join("server.toml"),
                format!(
                    r#"[server]
[typedb]
address = "localhost:1729"
database = "{database}"
tls = true
tls-root-ca = "root.pem"
"#
                ),
            )
            .unwrap();
        }
        let config_link = dir.path().join("server.toml");
        symlink(first.join("server.toml"), &config_link).unwrap();
        let replacement = second.join("server.toml");
        let link_for_swap = config_link.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probe(
            config_link.to_str().unwrap(),
            |_| None,
            move || {
                std::fs::remove_file(&link_for_swap).unwrap();
                symlink(&replacement, &link_for_swap).unwrap();
            },
        )
        .unwrap();

        assert_eq!(config.typedb.connection.database, "second-db");
        assert_eq!(
            config.typedb.tls_mode,
            OutboundTlsMode::CustomRootCa(second.join("root.pem").canonicalize().unwrap())
        );
        assert_eq!(
            config
                .typedb
                .custom_root_ca_snapshot
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"second-db".as_slice())
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_tls_parent_swap_after_authorized_read_uses_one_directory_identity() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("active");
        let retained = dir.path().join("retained");
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("root.pem"), "original root").unwrap();
        std::fs::write(active.join("server.pem"), "original certificate").unwrap();
        std::fs::write(active.join("server.key"), "original private key").unwrap();
        let config_path = active.join("server.toml");
        std::fs::write(
            &config_path,
            r#"[server]
[server.tls]
cert-path = "server.pem"
key-path = "server.key"
[typedb]
address = "localhost:1729"
database = "original-db"
tls = true
tls-root-ca = "root.pem"
"#,
        )
        .unwrap();
        let active_for_swap = active.clone();
        let retained_for_swap = retained.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probes(
            config_path.to_str().unwrap(),
            |_| None,
            || {},
            move || {
                std::fs::rename(&active_for_swap, &retained_for_swap).unwrap();
                std::fs::create_dir(&active_for_swap).unwrap();
                std::fs::write(active_for_swap.join("root.pem"), "replacement root").unwrap();
                std::fs::write(
                    active_for_swap.join("server.pem"),
                    "replacement certificate",
                )
                .unwrap();
                std::fs::write(
                    active_for_swap.join("server.key"),
                    "replacement private key",
                )
                .unwrap();
            },
        )
        .unwrap();

        assert_eq!(config.typedb.connection.database, "original-db");
        assert_eq!(
            config
                .typedb
                .custom_root_ca_snapshot
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original root".as_slice())
        );
        let inbound = config.inbound_tls.unwrap();
        let prepared = inbound.prepared.unwrap();
        assert_eq!(
            prepared
                .certificate
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original certificate".as_slice())
        );
        assert_eq!(
            prepared
                .private_key
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original private key".as_slice())
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_relative_tls_uses_the_retained_ancestor_after_parent_swap() {
        let dir = tempfile::tempdir().unwrap();
        let active_parent = dir.path().join("active-parent");
        let config_dir = active_parent.join("config");
        let retained_parent = dir.path().join("retained-parent");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(active_parent.join("root.pem"), "original ancestor root").unwrap();
        let config_path = config_dir.join("server.toml");
        std::fs::write(
            &config_path,
            r#"[server]
[typedb]
address = "localhost:1729"
database = "original-db"
tls = true
tls-root-ca = "../root.pem"
"#,
        )
        .unwrap();
        let active_for_swap = active_parent.clone();
        let retained_for_swap = retained_parent.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probes(
            config_path.to_str().unwrap(),
            |_| None,
            || {},
            move || {
                std::fs::rename(&active_for_swap, &retained_for_swap).unwrap();
                std::fs::create_dir(&active_for_swap).unwrap();
                std::fs::write(
                    active_for_swap.join("root.pem"),
                    "replacement ancestor root",
                )
                .unwrap();
            },
        )
        .unwrap();

        assert_eq!(config.typedb.connection.database, "original-db");
        assert_eq!(
            config
                .typedb
                .custom_root_ca_snapshot
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original ancestor root".as_slice())
        );
    }

    #[test]
    fn plaintext_config_does_not_require_a_post_read_path_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();
        let remove_after_read = path.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probe(
            path.to_str().unwrap(),
            |_| None,
            move || std::fs::remove_file(remove_after_read).unwrap(),
        )
        .unwrap();

        assert_eq!(config.typedb.connection.database, "mydb");
        assert_eq!(config.typedb.tls_mode, OutboundTlsMode::Disabled);
    }

    #[test]
    fn absolute_tls_paths_do_not_require_a_post_read_config_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.pem");
        std::fs::write(&root, "root material").unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            format!(
                r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
tls = true
tls-root-ca = {root:?}
"#,
                root = root.to_str().unwrap()
            ),
        )
        .unwrap();
        let remove_after_read = path.clone();

        let config = RuntimeServerConfig::from_file_with_env_after_probe(
            path.to_str().unwrap(),
            |_| None,
            move || std::fs::remove_file(remove_after_read).unwrap(),
        )
        .unwrap();

        assert_eq!(
            config.typedb.tls_mode,
            OutboundTlsMode::CustomRootCa(root.canonicalize().unwrap())
        );
        assert_eq!(
            config
                .typedb
                .custom_root_ca_snapshot
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"root material".as_slice())
        );
    }

    #[test]
    fn absolute_tls_material_survives_parent_replacement_after_config_load() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("active");
        let retained = dir.path().join("retained");
        std::fs::create_dir(&active).unwrap();
        let root = active.join("root.pem");
        let certificate = active.join("server.pem");
        let private_key = active.join("server.key");
        std::fs::write(&root, "original root").unwrap();
        std::fs::write(&certificate, "original certificate").unwrap();
        std::fs::write(&private_key, "original private key").unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"[server]
[server.tls]
cert-path = {certificate:?}
key-path = {private_key:?}
[typedb]
address = "localhost:1729"
database = "db"
tls = true
tls-root-ca = {root:?}
"#,
            ),
        )
        .unwrap();

        let config =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .unwrap();
        std::fs::rename(&active, &retained).unwrap();
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("root.pem"), "replacement root").unwrap();
        std::fs::write(active.join("server.pem"), "replacement certificate").unwrap();
        std::fs::write(active.join("server.key"), "replacement private key").unwrap();

        assert_eq!(
            config
                .typedb
                .custom_root_ca_snapshot
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original root".as_slice())
        );
        let prepared = config.inbound_tls.unwrap().prepared.unwrap();
        assert_eq!(
            prepared
                .certificate
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original certificate".as_slice())
        );
        assert_eq!(
            prepared
                .private_key
                .as_ref()
                .map(|material| material.bytes.as_ref()),
            Some(b"original private key".as_slice())
        );
    }

    #[test]
    fn outbound_root_contradiction_fails_before_file_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
tls = false
tls-root-ca = "missing.pem"
"#,
        )
        .unwrap();
        let error = RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None)
            .expect_err("contradictory policy must fail before reading the path");
        assert!(error.to_string().contains("contradicts"));
    }

    #[test]
    fn oversized_tls_material_fails_before_identity_or_provider_construction() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.pem");
        let file = std::fs::File::create(&root).unwrap();
        file.set_len(MAX_TLS_MATERIAL_BYTES + 1).unwrap();
        // Close the writable fixture handle before loading: the capture
        // opens the file denying concurrent writers, so a live writer
        // handle on Windows is a sharing violation, not a size failure.
        drop(file);
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            r#"[server]
[typedb]
address = "localhost:1729"
database = "db"
tls = true
tls-root-ca = "root.pem"
"#,
        )
        .unwrap();
        let error = RuntimeServerConfig::from_file_with_env(path.to_str().unwrap(), |_| None)
            .expect_err("oversized trust material must fail during config loading");
        assert!(error.to_string().contains("no larger than 1 MiB"));
    }

    #[tokio::test]
    async fn malformed_inbound_identity_fails_before_bind() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("server.pem");
        let key = dir.path().join("server.key");
        std::fs::write(&cert, "not a certificate").unwrap();
        std::fs::write(&key, "not a key").unwrap();
        let tls = InboundTlsSection::from_paths(cert, key);
        assert!(tls.load().await.is_err());
    }

    #[tokio::test]
    async fn inbound_identity_load_rechecks_the_open_handle_byte_limit() {
        let dir = tempfile::tempdir().unwrap();
        // load() walks parent components without following symlinks; the
        // ambient temp directory may sit behind symlinked components
        // (macOS /var), which would fail before the byte-limit check.
        let root = dir.path().canonicalize().unwrap();
        let cert = root.join("server.pem");
        let key = root.join("server.key");
        let file = std::fs::File::create(&cert).unwrap();
        file.set_len(MAX_TLS_MATERIAL_BYTES + 1).unwrap();
        std::fs::write(&key, "not a key").unwrap();
        let tls = InboundTlsSection::from_paths(cert, key);
        let error = tls
            .load()
            .await
            .expect_err("oversized identity must fail bounded");
        assert!(error.to_string().contains("no larger than 1 MiB"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn relative_inbound_identity_ignores_a_post_capture_symlink_swap() {
        use std::os::unix::fs::symlink;

        let identity = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("server.pem");
        let replacement = dir.path().join("replacement.pem");
        let key = dir.path().join("server.key");
        std::fs::write(&cert, identity.cert.pem()).unwrap();
        std::fs::write(&replacement, "attacker-controlled replacement").unwrap();
        std::fs::write(&key, identity.key_pair.serialize_pem()).unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            r#"[server]
[server.tls]
cert-path = "server.pem"
key-path = "server.key"
[typedb]
address = "localhost:1729"
database = "db"
"#,
        )
        .unwrap();
        let config =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .unwrap();
        let tls = config.inbound_tls.unwrap();

        std::fs::remove_file(&cert).unwrap();
        symlink(&replacement, &cert).unwrap();
        tls.load()
            .await
            .expect("relative identity loading must consume the captured certificate bytes");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn relative_inbound_identity_ignores_a_post_capture_parent_swap() {
        use std::os::unix::fs::symlink;

        let identity = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let identity_dir = dir.path().join("identity");
        let replacement_dir = dir.path().join("replacement");
        std::fs::create_dir(&identity_dir).unwrap();
        std::fs::create_dir(&replacement_dir).unwrap();
        std::fs::write(identity_dir.join("server.pem"), identity.cert.pem()).unwrap();
        std::fs::write(
            identity_dir.join("server.key"),
            identity.key_pair.serialize_pem(),
        )
        .unwrap();
        std::fs::write(
            replacement_dir.join("server.pem"),
            "attacker-controlled certificate",
        )
        .unwrap();
        std::fs::write(
            replacement_dir.join("server.key"),
            "attacker-controlled private key",
        )
        .unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            r#"[server]
[server.tls]
cert-path = "identity/server.pem"
key-path = "identity/server.key"
[typedb]
address = "localhost:1729"
database = "db"
"#,
        )
        .unwrap();
        let config =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .unwrap();
        let tls = config.inbound_tls.unwrap();

        std::fs::rename(&identity_dir, dir.path().join("identity-original")).unwrap();
        symlink(&replacement_dir, &identity_dir).unwrap();
        tls.load()
            .await
            .expect("relative identity loading must consume the captured identity bytes");
    }

    #[tokio::test]
    async fn relative_inbound_identity_rejects_a_mutated_diagnostic_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.pem"), "captured certificate").unwrap();
        std::fs::write(dir.path().join("server.key"), "captured private key").unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            r#"[server]
[server.tls]
cert-path = "server.pem"
key-path = "server.key"
[typedb]
address = "localhost:1729"
database = "db"
"#,
        )
        .unwrap();
        let config =
            RuntimeServerConfig::from_file_with_env(config_path.to_str().unwrap(), |_| None)
                .unwrap();
        let mut tls = config.inbound_tls.unwrap();
        tls.cert_path = dir.path().join("mutated.pem");

        let error = tls
            .load()
            .await
            .expect_err("captured certificate bytes must remain bound to their exact path");
        assert!(
            error
                .to_string()
                .contains("server.tls.cert-path changed after"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn inbound_identity_fifo_rejects_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        // load() walks parent components without following symlinks; the
        // ambient temp directory may sit behind symlinked components
        // (macOS /var), which would fail before the regular-file check.
        let root = dir.path().canonicalize().unwrap();
        let fifo = root.join("server.pem");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("POSIX mkfifo is available");
        assert!(status.success(), "mkfifo failed: {status}");
        let key = root.join("server.key");
        std::fs::write(&key, "not reached").unwrap();
        let tls = InboundTlsSection::from_paths(fifo, key);
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            sender
                .send(
                    runtime
                        .block_on(tls.load())
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                )
                .ok();
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("opening a FIFO must never block the process");
        let error = result.expect_err("a FIFO is not valid identity material");
        assert!(error.to_string().contains("must name a regular file"));
    }

    #[tokio::test]
    async fn mismatched_inbound_identity_fails_before_bind() {
        let first = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let second = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("server.pem");
        let key = dir.path().join("server.key");
        std::fs::write(&cert, first.cert.pem()).unwrap();
        std::fs::write(&key, second.key_pair.serialize_pem()).unwrap();
        let tls = InboundTlsSection::from_paths(cert, key);
        assert!(tls.load().await.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_config_fifo_rejects_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("server.toml");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("POSIX mkfifo is available");
        assert!(status.success(), "mkfifo failed: {status}");
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            sender
                .send(read_runtime_config_path(&fifo).map_err(|error| error.to_string()))
                .ok();
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("opening a configuration FIFO must never block the process");
        let error = result.expect_err("a FIFO is not a valid server configuration");
        assert!(error.contains("regular file"), "{error}");
    }

    #[test]
    fn oversized_runtime_config_is_rejected_from_same_handle_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_SERVER_CONFIG_BYTES + 1).unwrap();
        // Close the writable fixture handle before loading: the reader
        // opens the file denying concurrent writers, so a live writer
        // handle on Windows is a sharing violation, not a size failure.
        drop(file);

        let error = read_runtime_config_path(&path).unwrap_err();
        assert!(error.to_string().contains("no larger than 1 MiB"));
    }

    #[test]
    fn non_regular_runtime_config_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_runtime_config_path(dir.path()).unwrap_err();
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[test]
    fn non_regular_absolute_configured_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let error = match capture_absolute_configured_file(dir.path(), "typedb.tls-root-ca") {
            Ok(_) => panic!("a directory must not resolve as configured TLS material"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn non_regular_absolute_declared_schema_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // The capture walks parent components without following symlinks;
        // the ambient temp directory may sit behind symlinked components
        // (macOS /var), so hand it an already-physical path.
        let root = dir.path().canonicalize().unwrap();
        let error = capture_absolute_declared_schema_file(&root)
            .expect_err("a directory must not resolve as a declared schema");
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[test]
    fn runtime_config_growth_after_metadata_is_rejected_by_bounded_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let grow = file.try_clone().unwrap();

        let error = read_runtime_config_handle_after_inspect(file, move || {
            grow.set_len(MAX_SERVER_CONFIG_BYTES + 1).unwrap();
        })
        .unwrap_err();
        assert!(error.to_string().contains("no larger than 1 MiB"));
    }

    #[test]
    fn same_size_runtime_config_rewrite_between_reads_is_rejected() {
        use std::io::{Seek as _, Write as _};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        let original = MINIMAL_CONFIG.replace("mydb", "aaaa");
        let replacement = MINIMAL_CONFIG.replace("mydb", "bbbb");
        assert_eq!(original.len(), replacement.len());
        std::fs::write(&path, original).unwrap();
        let reader = std::fs::File::open(&path).unwrap();
        let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();

        let error = read_runtime_config_handle_with_hooks(
            reader,
            || {},
            move || {
                writer.seek(SeekFrom::Start(0)).unwrap();
                writer.write_all(replacement.as_bytes()).unwrap();
                writer.flush().unwrap();
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed while"), "{error}");
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
