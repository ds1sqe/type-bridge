//! External-crate source compatibility for the released server config API.

use type_bridge_server::config::{
    InterceptorsSection, LoggingSection, SchemaSection, ServerConfig, ServerSection, TypeDBSection,
};

#[test]
fn released_struct_literals_and_exhaustive_patterns_still_compile() {
    let config = ServerConfig {
        server: ServerSection {
            host: "127.0.0.1".to_owned(),
            port: 8080,
        },
        typedb: TypeDBSection {
            address: "localhost:1729".to_owned(),
            database: "test".to_owned(),
            username: "admin".to_owned(),
            password: "password".to_owned(),
            http_port: 8000,
            server_version: None,
        },
        schema: SchemaSection::default(),
        interceptors: InterceptorsSection::default(),
        logging: LoggingSection::default(),
    };

    let ServerConfig {
        server,
        typedb,
        schema,
        interceptors,
        logging,
    } = config;
    let ServerSection { host, port } = server;
    let TypeDBSection {
        address,
        database,
        username,
        password,
        http_port,
        server_version,
    } = typedb;

    assert_eq!(host, "127.0.0.1");
    assert_eq!(port, 8080);
    assert_eq!(address, "localhost:1729");
    assert_eq!(database, "test");
    assert_eq!(username, "admin");
    assert_eq!(password, "password");
    assert_eq!(http_port, 8000);
    assert!(server_version.is_none());
    assert!(schema.source_file.is_empty());
    assert!(interceptors.enabled.is_empty());
    assert_eq!(logging.level, "info");
}

#[test]
fn released_debug_trait_redacts_connection_credentials() {
    const SENTINEL: &str = "public-config-secret";
    let config = ServerConfig {
        server: ServerSection {
            host: "127.0.0.1".to_owned(),
            port: 8080,
        },
        typedb: TypeDBSection {
            address: format!("admin:{SENTINEL}@localhost:1729"),
            database: "test".to_owned(),
            username: SENTINEL.to_owned(),
            password: SENTINEL.to_owned(),
            http_port: 8000,
            server_version: None,
        },
        schema: SchemaSection::default(),
        interceptors: InterceptorsSection::default(),
        logging: LoggingSection::default(),
    };

    let rendered = format!("{config:?}");
    assert!(!rendered.contains(SENTINEL), "{rendered}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(rendered.contains("test"));
}

#[cfg(feature = "typedb")]
#[test]
fn released_connect_signature_still_accepts_plaintext_section() {
    fn assert_future(config: &TypeDBSection) {
        let future = type_bridge_server::typedb::TypeDBClient::connect(config);
        drop(future);
    }

    let _signature: for<'a> fn(&'a TypeDBSection) = assert_future;
}
