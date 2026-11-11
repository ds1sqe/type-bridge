/// HTTP error responses and health mount.
pub mod http;

/// Shared staged executor state and replay protection.
pub mod staged_state;

/// Staged transactions over one executor identity.
pub mod staged;

/// Opt-in raw TypeQL transport with host admission hooks.
#[cfg(feature = "typedb")]
pub mod raw;
