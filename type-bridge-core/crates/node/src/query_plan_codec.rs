//! Primitive-only bridge to the shared staged plan codec.
use napi::bindgen_prelude::{Buffer, Env, Unknown};
use napi_derive::napi;
use type_bridge_contract::query_plan::decode_query_plan;
use type_bridge_contract::schema::decode_declared_schema;

#[napi(object)]
pub struct QueryPlanEncoding {
    pub bytes: Buffer,
    pub fingerprint: String,
}

/// Validate canonical staged bytes under a declared schema and return bytes and digest.
#[napi(js_name = "_query_plan_round_trip")]
pub fn _query_plan_round_trip(
    env: Env,
    plan_bytes: Unknown,
    schema_bytes: Unknown,
) -> napi::Result<QueryPlanEncoding> {
    let plan_bytes = crate::plan_runtime::non_shared_buffer(&env, plan_bytes)?;
    let schema_bytes = crate::plan_runtime::non_shared_buffer(&env, schema_bytes)?;
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
