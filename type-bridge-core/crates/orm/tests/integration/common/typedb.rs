use std::env;

fn parse_http_port(value: &str) -> Result<u16, String> {
    let port = value.parse::<u16>().map_err(|error| {
        format!("TYPEDB_HTTP_PORT must be an integer from 1 through 65535, got {value:?}: {error}")
    })?;
    if port == 0 {
        return Err(format!(
            "TYPEDB_HTTP_PORT must be an integer from 1 through 65535, got {value:?}"
        ));
    }
    Ok(port)
}

pub fn connect_options_from_env() -> type_bridge_orm::ConnectOptions {
    let mut options = type_bridge_orm::ConnectOptions::default();
    options.http_port = match env::var("TYPEDB_HTTP_PORT") {
        Ok(value) => parse_http_port(&value).unwrap_or_else(|error| panic!("{error}")),
        Err(env::VarError::NotPresent) => options.http_port,
        Err(env::VarError::NotUnicode(_)) => {
            panic!("TYPEDB_HTTP_PORT must contain valid UTF-8 decimal digits")
        }
    };
    options
}

pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    context: &str,
) {
    // Route through the orm seam so the test setup itself is band-dispatched;
    // a raw driver here would pin the helper to one protocol band.
    type_bridge_orm::ensure_database_exists(
        address,
        database,
        username,
        password,
        connect_options_from_env(),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("{context}: failed to ensure database {database} at {address}: {error}")
    });
}

#[cfg(test)]
mod tests {
    use super::parse_http_port;

    #[test]
    fn http_port_parser_accepts_valid_remapped_port() {
        assert_eq!(parse_http_port("32881"), Ok(32881));
    }

    #[test]
    fn http_port_parser_rejects_invalid_values_clearly() {
        for value in ["0", "70000", "not-a-port"] {
            let error = parse_http_port(value).expect_err("invalid port must fail");
            assert!(error.contains("TYPEDB_HTTP_PORT"), "{error}");
            assert!(error.contains(value), "{error}");
        }
    }
}
