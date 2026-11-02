/// HTTP error responses and health mount.
pub mod http;

#[cfg(feature = "v2-query")]
/// Shared staged executor state and replay protection.
pub mod staged_state;

#[cfg(feature = "v2-query")]
/// Staged V3 transactions over one executor identity.
pub mod v3;
