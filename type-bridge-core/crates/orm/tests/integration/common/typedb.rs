use typedb_driver::{Credentials, DriverOptions, TypeDBDriver};

pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    context: &str,
) {
    let options = DriverOptions::new(false, None)
        .unwrap_or_else(|error| panic!("{context}: TypeDB driver options failed: {error}"));
    let driver = TypeDBDriver::new(address, Credentials::new(username, password), options)
        .await
        .unwrap_or_else(|error| {
            panic!("{context}: failed to connect to TypeDB at {address}: {error}")
        });

    let databases = driver.databases();
    let exists = databases
        .contains(database)
        .await
        .unwrap_or_else(|error| panic!("{context}: database lookup failed: {error}"));
    if !exists {
        databases
            .create(database)
            .await
            .unwrap_or_else(|error| panic!("{context}: database create failed: {error}"));
    }
}
