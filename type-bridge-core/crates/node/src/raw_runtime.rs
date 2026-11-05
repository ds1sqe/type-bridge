//! Raw-query lifecycle and lossless JavaScript value marshalling.

use crate::proxy_connection_runtime::settle;
use crate::{NodeProxyConnection, NodeRustDatabase};
use napi::{
    Property, PropertyAttributes,
    bindgen_prelude::{BigInt, Env, JsObjectValue, JsValue, Object, ToNapiValue, Unknown},
};
use napi_derive::napi;
use std::sync::Arc;
use type_bridge_orm::session::raw::{RawQueryError, RawTransaction};
use type_bridge_orm::{CommitFailureCertainty, ProviderRuntimeOwner, TxType};

fn raw_error(error: RawQueryError) -> napi::Error {
    let outcome = error.commit_outcome.map(|outcome| match outcome {
        CommitFailureCertainty::DefinitelyAborted => "definitely_aborted",
        CommitFailureCertainty::Unknown => "unknown",
    });
    napi::Error::from_reason(format!(
        "typebridge.raw:{}",
        serde_json::json!({
            "code":error.code, "message":error.message, "status":error.status,
            "providerCode":error.provider_code, "commitOutcome":outcome,
        })
    ))
}

struct RawJson(serde_json::Value);

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
enum RawNumber {
    Number(f64),
    BigInt(BigInt),
}
fn raw_number(value: serde_json::Number) -> RawNumber {
    if let Some(value) = value.as_i64() {
        return if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value) {
            RawNumber::Number(value as f64)
        } else {
            RawNumber::BigInt(BigInt::from(value))
        };
    }
    if let Some(value) = value.as_u64() {
        return if value <= MAX_SAFE_INTEGER as u64 {
            RawNumber::Number(value as f64)
        } else {
            RawNumber::BigInt(BigInt::from(value))
        };
    }
    let token = value.to_string();
    if token.contains(['.', 'e', 'E']) {
        return RawNumber::Number(token.parse().expect("JSON numeric token is a float"));
    }
    // serde_json may retain arbitrary-precision integer tokens. Convert decimal
    // digits to N-API words without routing them through a lossy float or JS code.
    let sign_bit = token.starts_with('-');
    let digits = token.trim_start_matches('-').bytes();
    let mut words = vec![0_u64];
    for digit in digits {
        let mut carry = u128::from(digit - b'0');
        for word in &mut words {
            let value = u128::from(*word) * 10 + carry;
            *word = value as u64;
            carry = value >> 64;
        }
        if carry != 0 {
            words.push(carry as u64);
        }
    }
    RawNumber::BigInt(BigInt { sign_bit, words })
}

impl ToNapiValue for RawJson {
    unsafe fn to_napi_value(
        env: napi::sys::napi_env,
        value: Self,
    ) -> napi::Result<napi::sys::napi_value> {
        // Every value is built in the supplied live environment; no JS callbacks
        // or setters participate in converting provider JSON to native values.
        unsafe {
            match value.0 {
                serde_json::Value::Null => <Option<bool>>::to_napi_value(env, None),
                serde_json::Value::Bool(value) => bool::to_napi_value(env, value),
                serde_json::Value::String(value) => String::to_napi_value(env, value),
                serde_json::Value::Number(value) => match raw_number(value) {
                    RawNumber::Number(value) => f64::to_napi_value(env, value),
                    RawNumber::BigInt(value) => BigInt::to_napi_value(env, value),
                },
                serde_json::Value::Array(values) => {
                    Vec::<RawJson>::to_napi_value(env, values.into_iter().map(RawJson).collect())
                }
                serde_json::Value::Object(values) => {
                    let environment = Env::from_raw(env);
                    let mut object = Object::new(&environment)?;
                    for (name, value) in values {
                        let value = Unknown::from_raw_unchecked(
                            env,
                            RawJson::to_napi_value(env, RawJson(value))?,
                        );
                        object.define_properties(&[Property::new()
                            .with_name(&environment, name)?
                            .with_value(&value)
                            .with_property_attributes(PropertyAttributes::default())])?;
                    }
                    Ok(object.raw())
                }
            }
        }
    }
}

async fn execute_one(mut transaction: RawTransaction, query: String) -> napi::Result<RawJson> {
    let result: napi::Result<RawJson> = async {
        let bytes = transaction.execute_query(&query).await.map_err(raw_error)?;
        let answer = RawJson(transaction.decode_answer(&bytes).map_err(raw_error)?);
        transaction.commit().await.map_err(raw_error)?;
        Ok(answer)
    }
    .await;
    let closed = transaction.close().await.map_err(raw_error);
    let answer = result?;
    closed?;
    Ok(answer)
}

#[napi]
pub struct NodeRawTransaction {
    inner: Arc<tokio::sync::Mutex<RawTransaction>>,
    runtime: Arc<ProviderRuntimeOwner>,
}
impl NodeRawTransaction {
    fn new(inner: RawTransaction, runtime: Arc<ProviderRuntimeOwner>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(inner)),
            runtime,
        }
    }
}
#[napi]
impl NodeRawTransaction {
    #[napi(js_name = "executeQuery", ts_return_type = "Promise<unknown>")]
    pub fn execute_query(&self, env: Env, query: String) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(&env, &self.runtime, async move {
            let mut transaction = inner.lock().await;
            let bytes = transaction.execute_query(&query).await.map_err(raw_error)?;
            transaction
                .decode_answer(&bytes)
                .map(RawJson)
                .map_err(raw_error)
        })
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn commit(&self, env: Env) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(&env, &self.runtime, async move {
            inner.lock().await.commit().await.map_err(raw_error)
        })
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn rollback(&self, env: Env) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(&env, &self.runtime, async move {
            inner.lock().await.rollback().await.map_err(raw_error)
        })
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn close(&self, env: Env) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(&env, &self.runtime, async move {
            inner.lock().await.close().await.map_err(raw_error)
        })
    }
}

#[napi]
impl NodeRustDatabase {
    #[napi(
        js_name = "rawTransaction",
        ts_return_type = "Promise<NodeRawTransaction>"
    )]
    pub fn raw_transaction(&self, env: Env, kind: String) -> napi::Result<Object<'static>> {
        let kind = crate::match_runtime::parse_enum::<TxType>(&kind)?;
        let (database, runtime) = self.handles();
        let owner = Arc::clone(&runtime);
        settle(&env, &runtime, async move {
            database
                .raw_transaction(kind)
                .await
                .map(|inner| NodeRawTransaction::new(inner, owner))
                .map_err(raw_error)
        })
    }
    #[napi(js_name = "executeQuery", ts_return_type = "Promise<unknown>")]
    pub fn execute_query(
        &self,
        env: Env,
        query: String,
        kind: String,
    ) -> napi::Result<Object<'static>> {
        let kind = crate::match_runtime::parse_enum::<TxType>(&kind)?;
        let (database, runtime) = self.handles();
        settle(&env, &runtime, async move {
            execute_one(
                database.raw_transaction(kind).await.map_err(raw_error)?,
                query,
            )
            .await
        })
    }
}

#[napi]
impl NodeProxyConnection {
    #[napi(js_name = "rawTransaction")]
    pub fn raw_transaction(&self, kind: String) -> napi::Result<NodeRawTransaction> {
        let kind = crate::match_runtime::parse_enum::<TxType>(&kind)?;
        let (connection, runtime) = self.handles();
        Ok(NodeRawTransaction::new(
            connection.raw_transaction(kind).map_err(raw_error)?,
            runtime,
        ))
    }
    #[napi(js_name = "executeQuery", ts_return_type = "Promise<unknown>")]
    pub fn execute_query(
        &self,
        env: Env,
        query: String,
        kind: String,
    ) -> napi::Result<Object<'static>> {
        let kind = crate::match_runtime::parse_enum::<TxType>(&kind)?;
        let (connection, runtime) = self.handles();
        settle(&env, &runtime, async move {
            execute_one(connection.raw_transaction(kind).map_err(raw_error)?, query).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_integer_conversion_is_lossless_and_safe_values_stay_numbers() {
        for value in [i64::MIN, i64::MAX, 9_007_199_254_740_993] {
            let RawNumber::BigInt(result) = raw_number(value.into()) else {
                panic!("unsafe integer rounded")
            };
            assert_eq!(result.sign_bit, value < 0);
            assert_eq!(result.words, vec![value.unsigned_abs()]);
        }
        for value in [-MAX_SAFE_INTEGER, 0, 7, MAX_SAFE_INTEGER] {
            let RawNumber::Number(result) = raw_number(value.into()) else {
                panic!("safe integer became bigint")
            };
            assert_eq!(result, value as f64);
        }
        let RawNumber::Number(value) = raw_number(serde_json::Number::from_f64(1.25).unwrap())
        else {
            panic!("double became bigint")
        };
        assert_eq!(value, 1.25);
    }
}
