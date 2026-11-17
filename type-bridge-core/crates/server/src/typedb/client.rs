use super::real_driver::{prepare_secure_connect_options, sanitize_prepared_connect_error};
use crate::config::SecureTypeDBSection;
use crate::error::PipelineError;
use type_bridge_typedb_runtime::PreparedSecureConnectOptions;

/// One immutable server connection identity bound to one prepared transport.
///
/// The fields are intentionally private: a caller cannot pair trust material,
/// HTTP discovery settings, or a selected driver band with a different host or
/// credential set after preparation.
#[doc(hidden)]
pub struct PreparedSecureTypeDBConnection {
    address: String,

    database: String,
    username: String,
    password: String,
    options: PreparedSecureConnectOptions,
}

impl PreparedSecureTypeDBConnection {
    /// Connect the raw runtime through the same captured transport identity.
    pub async fn connect_raw_runtime(
        &self,
    ) -> Result<type_bridge_typedb_runtime::TypeDBRuntime, PipelineError> {
        type_bridge_typedb_runtime::TypeDBRuntime::connect_prepared_secure(
            &self.address,
            &self.username,
            &self.password,
            self.options.clone(),
        )
        .await
        .map_err(sanitize_prepared_connect_error)
    }

    /// Connect the ORM authority through this exact prepared identity.
    #[doc(hidden)]
    pub async fn connect_database(&self) -> Result<type_bridge_orm::Database, PipelineError> {
        type_bridge_orm::Database::connect_prepared_secure_with_options(
            &self.address,
            &self.database,
            &self.username,
            &self.password,
            self.options.clone(),
        )
        .await
        .map_err(sanitize_prepared_connect_error)
    }
}

/// Constructor for one captured server transport identity.
pub struct TypeDBClient;

impl TypeDBClient {
    /// Validate and snapshot one secure transport policy for reuse.
    pub fn prepare_secure_transport(
        config: &SecureTypeDBSection,
    ) -> Result<PreparedSecureTypeDBConnection, PipelineError> {
        // Resolve trust before retaining any credential-bearing connection
        // identity, then bind both into one opaque value.
        let options = prepare_secure_connect_options(config)?;
        let connection = &config.connection;
        Ok(PreparedSecureTypeDBConnection {
            address: connection.address.clone(),

            database: connection.database.clone(),
            username: connection.username.clone(),
            password: connection.password.clone(),
            options,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OutboundTlsMode, TypeDBSection};
    use std::error::Error as _;

    #[tokio::test]
    async fn staged_and_raw_connectors_do_not_expose_provider_identity_or_credentials() {
        const ADDRESS: &str = "admin:TB_ADDRESS_SECRET@provider.invalid:1729";
        const USERNAME: &str = "TB_USERNAME_SECRET";
        const PASSWORD: &str = "TB_PASSWORD_SECRET";
        let config = SecureTypeDBSection::new(
            TypeDBSection {
                address: ADDRESS.to_owned(),
                database: "db".to_owned(),
                username: USERNAME.to_owned(),
                password: PASSWORD.to_owned(),
                http_port: 8000,
                server_version: Some("3.12.1".to_owned()),
            },
            OutboundTlsMode::Disabled,
        );
        let prepared = TypeDBClient::prepare_secure_transport(&config).unwrap();
        let staged = prepared
            .connect_database()
            .await
            .err()
            .expect("invalid address");
        let raw = prepared
            .connect_raw_runtime()
            .await
            .err()
            .expect("invalid address");
        for error in [staged, raw] {
            let rendered = format!("{error}\n{error:?}");
            for secret in [ADDRESS, USERNAME, PASSWORD] {
                assert!(!rendered.contains(secret), "{secret}: {rendered}");
            }
            assert!(error.source().is_none());
        }
    }
}
