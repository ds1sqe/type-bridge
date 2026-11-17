//! Shared identifier validation for the nominal TypeScript IID facade.

/// Validate an IID without rewriting its provider spelling.
#[napi_derive::napi]
pub fn validate_iid(value: String) -> napi::Result<String> {
    type_bridge_contract::id::Iid::new(value)
        .map(|iid| iid.as_str().to_owned())
        .map_err(|error| crate::plan_runtime::napi_error(&error))
}
