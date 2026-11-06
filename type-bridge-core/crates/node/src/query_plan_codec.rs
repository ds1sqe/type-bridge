//! Primitive-only bridge to the shared V3 plan codec.
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use type_bridge_contract::query_plan::decode_query_plan;
use type_bridge_contract::schema::decode_declared_schema;

#[napi(object)]
pub struct QueryPlanEncoding {
    pub bytes: Buffer,
    pub fingerprint: String,
}

/// Validate canonical V3 bytes under a declared schema and return bytes and digest.
#[napi(js_name = "_query_plan_round_trip")]
pub fn _query_plan_round_trip(
    plan_bytes: Buffer,
    schema_bytes: Buffer,
) -> napi::Result<QueryPlanEncoding> {
    let schema = decode_declared_schema(&schema_bytes)
        .map_err(|error| crate::plan_runtime::napi_error(&error))?;
    let plan = decode_query_plan(&plan_bytes, &schema)
        .map_err(|error| crate::plan_runtime::napi_error(&error))?;
    Ok(QueryPlanEncoding {
        bytes: plan
            .canonical_bytes()
            .map_err(|error| crate::plan_runtime::napi_error(&error))?
            .into(),
        fingerprint: plan
            .fingerprint()
            .map_err(|error| crate::plan_runtime::napi_error(&error))?
            .as_fingerprint()
            .digest()
            .to_hex(),
    })
}
