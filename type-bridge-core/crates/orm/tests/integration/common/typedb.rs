pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    context: &str,
) {
    // Route through the orm seam so the test setup itself is band-dispatched;
    // a raw driver here would pin the helper to one protocol band.
    type_bridge_orm::ensure_database_exists(address, database, username, password)
        .await
        .unwrap_or_else(|error| {
            panic!("{context}: failed to ensure database {database} at {address}: {error}")
        });
}
