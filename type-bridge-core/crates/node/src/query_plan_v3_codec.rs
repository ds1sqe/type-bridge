//! Primitive-only bridge to the shared V3 plan codec.
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::schema::decode_declared_schema;

#[napi(object)]
pub struct QueryPlanV3Encoding {
    pub bytes: Buffer,
    pub fingerprint: String,
}

/// Validate canonical V3 bytes under a declared schema and return bytes and digest.
#[napi(js_name = "_query_plan_v3_round_trip")]
pub fn _query_plan_v3_round_trip(
    plan_bytes: Buffer,
    schema_bytes: Buffer,
) -> napi::Result<QueryPlanV3Encoding> {
    let schema = decode_declared_schema(&schema_bytes)
        .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
    let plan = decode_query_plan_v3(&plan_bytes, &schema)
        .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
    Ok(QueryPlanV3Encoding {
        bytes: plan
            .canonical_bytes()
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?
            .into(),
        fingerprint: plan
            .fingerprint()
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?
            .as_fingerprint()
            .digest()
            .to_hex(),
    })
}
