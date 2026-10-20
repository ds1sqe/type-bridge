/// Released V1 Axum routes and handlers.
pub mod http;
/// Released V1 HTTP request and response models.
pub mod types;

#[cfg(feature = "v2-query")]
/// Generated-authority-backed V2 query routes and handlers.
pub mod v2;

#[cfg(feature = "v2-query")]
/// Staged V3 transactions over the V2 executor identity.
pub mod v3;
