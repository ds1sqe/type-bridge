//! Generated-projection and query runtime for the Node binding.
//!
//! The addon exposes connection, transaction, verified projection, and Query
//! V2 handles. Arbitrary descriptor registration, handwritten dynamic managers,
//! parser/generator entry points, and descriptor-marshalling functions are not
//! part of this boundary.

#![allow(missing_docs)]

#[cfg(feature = "contract-test-adapter")]
mod contract_test_adapter;
mod match_runtime;
mod query_v2_builder_runtime;
mod query_v2_model_remote_runtime;
pub mod query_v2_runtime;
mod runtime_projection;

#[cfg(feature = "contract-test-adapter")]
pub use contract_test_adapter::round_trip_contract_foundation;

pub use match_runtime::{
    NodeMatchBindingHandle, NodeMatchFieldHandle, NodeMatchOrderHandle, NodeMatchPredicateHandle,
    NodeMatchQueryHandle, NodeMatchRoleHandle, NodeMatchSelectionHandle, NodeMatchSessionHandle,
    NodeMatchShapeHandle, NodeValidatedMatchResultHandle, NodeValidatedThingHandle,
};
pub use query_v2_model_remote_runtime::{
    NodePendingRemoteModelQuery, NodeRemoteModelQueryContext, query_v2_prepare_remote_model_count,
    query_v2_prepare_remote_model_exists, query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_reduce, query_v2_prepare_remote_model_reduce_by_field,
    query_v2_prepare_remote_model_reduce_by_fields, query_v2_prepare_remote_model_rows,
    query_v2_remote_model_context,
};
pub use runtime_projection::{NodeProjectedModelManager, NodeRuntimeProjection};

use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::{Map, Value};
use type_bridge_core_lib::version as core_version;
#[cfg(test)]
use type_bridge_orm::_descriptor::{EntityDescriptor, RelationDescriptor};
#[cfg(test)]
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    AttributeValue, OrmError, ProviderRuntimeOwner, TransactionContext, TxType, ValueType,
};

/// Private registry fixture used by native match-runtime unit tests only.
///
/// Generated JavaScript obtains a registry exclusively through an installed
/// `NodeRuntimeProjection`; this type carries no N-API export.
#[cfg(test)]
pub(crate) struct NodeDescriptorRegistry {
    inner: Arc<DescriptorRegistry>,
}

#[cfg(test)]
impl NodeDescriptorRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(DescriptorRegistry::new()),
        }
    }

    pub(crate) fn register_entity_json(&self, descriptor_json: String) -> Result<String> {
        let descriptor: EntityDescriptor =
            serde_json::from_str(&descriptor_json).map_err(invalid_json_error("entity"))?;
        let registered = self
            .inner
            .register_entity(descriptor)
            .map_err(napi_orm_error)?;
        serde_json::to_string(registered.as_ref()).map_err(json_serialize_error)
    }

    pub(crate) fn register_relation_json(&self, descriptor_json: String) -> Result<String> {
        let descriptor: RelationDescriptor =
            serde_json::from_str(&descriptor_json).map_err(invalid_json_error("relation"))?;
        let registered = self
            .inner
            .register_relation(descriptor)
            .map_err(napi_orm_error)?;
        serde_json::to_string(registered.as_ref()).map_err(json_serialize_error)
    }

    pub(crate) fn shared_registry(&self) -> Arc<DescriptorRegistry> {
        Arc::clone(&self.inner)
    }
}

/// JavaScript-facing Rust database handle backed by the shared ORM runtime.
#[napi]
pub struct NodeRustDatabase {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<ProviderRuntimeOwner>,
}

impl NodeRustDatabase {
    pub(crate) fn handles(&self) -> (Arc<type_bridge_orm::Database>, Arc<ProviderRuntimeOwner>) {
        (Arc::clone(&self.db), Arc::clone(&self.runtime))
    }
}

#[napi]
impl NodeRustDatabase {
    #[napi(js_name = "isConnected")]
    pub fn is_connected(&self) -> bool {
        self.db.is_connected()
    }

    #[napi(js_name = "close")]
    pub fn close(&self) -> Result<()> {
        self.db.close().map_err(napi_orm_error)
    }

    #[napi(js_name = "databaseName")]
    pub fn database_name(&self) -> String {
        self.db.database_name().to_string()
    }

    #[napi(js_name = "databaseExists")]
    pub fn database_exists(&self) -> Result<bool> {
        self.runtime
            .block_on(self.db.database_exists())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "createDatabase")]
    pub fn create_database(&self) -> Result<()> {
        self.runtime
            .block_on(self.db.create_database())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "deleteDatabase")]
    pub fn delete_database(&self) -> Result<()> {
        self.runtime
            .block_on(self.db.delete_database())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "resetDatabase")]
    pub fn reset_database(&self) -> Result<()> {
        self.runtime
            .block_on(async {
                if self.db.database_exists().await? {
                    self.db.delete_database().await?;
                }
                self.db.create_database().await
            })
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "transaction")]
    pub fn transaction(
        &self,
        transaction_type: Option<String>,
    ) -> Result<NodeRustTransactionContext> {
        let tx_type = parse_tx_type(transaction_type.as_deref().unwrap_or("read"))?;
        let context = self
            .runtime
            .block_on(self.db.transaction_context(tx_type))
            .map_err(napi_orm_error)?;
        Ok(NodeRustTransactionContext {
            context,
            runtime: Arc::clone(&self.runtime),
        })
    }
}

/// JavaScript-facing Rust transaction context.
#[napi]
pub struct NodeRustTransactionContext {
    context: TransactionContext,
    runtime: Arc<ProviderRuntimeOwner>,
}

impl NodeRustTransactionContext {
    pub(crate) fn handles(&self) -> (TransactionContext, Arc<ProviderRuntimeOwner>) {
        (self.context.clone(), Arc::clone(&self.runtime))
    }
}

#[napi]
impl NodeRustTransactionContext {
    #[napi(js_name = "queryJson")]
    pub fn query_json(&self, query: String) -> Result<String> {
        let result = self
            .runtime
            .block_on(self.context.query(&query))
            .map_err(napi_orm_error)?;
        query_result_to_json(result)
    }

    #[napi(js_name = "commit")]
    pub fn commit(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.commit())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "rollback")]
    pub fn rollback(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.rollback())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "close")]
    pub fn close(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.close())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "transactionType")]
    pub fn transaction_type(&self) -> String {
        tx_type_name(self.context.tx_type()).to_string()
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
#[napi(js_name = "ensureRustDatabase")]
#[allow(
    clippy::too_many_arguments,
    reason = "the stable N-API connection function has explicit transport fields"
)]
pub fn ensure_rust_database(
    address: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
    http_port: Option<u32>,
    server_version: Option<String>,
    tls_enabled: Option<bool>,
    tls_root_ca: Option<String>,
) -> Result<()> {
    let options = napi_secure_connect_options(http_port, server_version, tls_enabled, tls_root_ca)?;
    let prepared = options
        .prepare_transport()
        .map_err(napi_secure_connect_error)?;
    let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create Tokio runtime: {error}"),
        )
    })?;
    let username = username.unwrap_or_else(|| "admin".to_string());
    let password = password.unwrap_or_else(|| "password".to_string());
    runtime
        .block_on(type_bridge_orm::ensure_database_exists_prepared_secure(
            &address, &database, &username, &password, prepared,
        ))
        .map_err(napi_secure_connect_error)
}

/// Connect to TypeDB through the shared Rust session layer.
#[napi(js_name = "connectRustDatabase")]
#[allow(
    clippy::too_many_arguments,
    reason = "the stable N-API connection function has explicit transport fields"
)]
pub fn connect_rust_database(
    address: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
    http_port: Option<u32>,
    server_version: Option<String>,
    tls_enabled: Option<bool>,
    tls_root_ca: Option<String>,
) -> Result<NodeRustDatabase> {
    let options = napi_secure_connect_options(http_port, server_version, tls_enabled, tls_root_ca)?;
    let prepared = options
        .prepare_transport()
        .map_err(napi_secure_connect_error)?;
    let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create Tokio runtime: {error}"),
        )
    })?;
    let username = username.unwrap_or_else(|| "admin".to_string());
    let password = password.unwrap_or_else(|| "password".to_string());
    let db = runtime
        .block_on(
            type_bridge_orm::Database::connect_prepared_secure_with_options(
                &address, &database, &username, &password, prepared,
            ),
        )
        .map_err(napi_secure_connect_error)?;

    Ok(NodeRustDatabase {
        db: Arc::new(db),
        runtime,
    })
}

pub(crate) fn attribute_value_from_js(
    value: &Value,
    expected_type: Option<ValueType>,
) -> Result<AttributeValue> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::from_reason("Attribute value must be an object"))?;
    let value_type_name = required_string(object, "value_type")?;
    let value_type = match value_type_name.as_str() {
        "datetime_tz" => Some(ValueType::DateTimeTz),
        value_type => ValueType::parse(value_type),
    }
    .ok_or_else(|| {
        Error::from_reason(format!(
            "value_type must be one of the TypeDB value types, got '{value_type_name}'"
        ))
    })?;
    if let Some(expected_type) = expected_type
        && value_type != expected_type
    {
        return Err(Error::from_reason(format!(
            "Expected {} attribute value, got {}",
            expected_type.as_str(),
            value_type.as_str()
        )));
    }
    let raw = object
        .get("value")
        .ok_or_else(|| Error::from_reason("Attribute value missing value"))?;

    match value_type {
        ValueType::String => Ok(AttributeValue::String(required_value_string(
            raw, "string",
        )?)),
        ValueType::Long => Ok(AttributeValue::Long(long_from_js(raw)?)),
        ValueType::Double => raw
            .as_f64()
            .map(AttributeValue::Double)
            .ok_or_else(|| Error::from_reason("double value must be a number")),
        ValueType::Boolean => raw
            .as_bool()
            .map(AttributeValue::Boolean)
            .ok_or_else(|| Error::from_reason("boolean value must be a boolean")),
        ValueType::Date => Ok(AttributeValue::Date(required_value_string(raw, "date")?)),
        ValueType::DateTime => Ok(AttributeValue::DateTime(required_value_string(
            raw, "datetime",
        )?)),
        ValueType::DateTimeTz => Ok(AttributeValue::DateTimeTZ(required_value_string(
            raw,
            "datetime-tz",
        )?)),
        ValueType::Decimal => Ok(AttributeValue::Decimal(required_value_string(
            raw, "decimal",
        )?)),
        ValueType::Duration => Ok(AttributeValue::Duration(required_value_string(
            raw, "duration",
        )?)),
    }
}

pub(crate) fn attribute_value_to_json(value: &AttributeValue) -> Value {
    match value {
        AttributeValue::String(value) => serde_json::json!({ "String": value }),
        AttributeValue::Long(value) => serde_json::json!({ "Long": value.to_string() }),
        AttributeValue::Double(value) => serde_json::json!({ "Double": value }),
        AttributeValue::Boolean(value) => serde_json::json!({ "Boolean": value }),
        AttributeValue::Date(value) => serde_json::json!({ "Date": value }),
        AttributeValue::DateTime(value) => serde_json::json!({ "DateTime": value }),
        AttributeValue::DateTimeTZ(value) => serde_json::json!({ "DateTimeTZ": value }),
        AttributeValue::Decimal(value) => serde_json::json!({ "Decimal": value }),
        AttributeValue::Duration(value) => serde_json::json!({ "Duration": value }),
    }
}

fn long_from_js(value: &Value) -> Result<i64> {
    let value = value.as_str().ok_or_else(|| {
        Error::from_reason("long value must be a string produced from TypeScript bigint")
    })?;
    value
        .parse::<i64>()
        .map_err(|error| Error::from_reason(format!("Invalid i64 long value '{value}': {error}")))
}

fn required_value_string(value: &Value, value_type: &str) -> Result<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| Error::from_reason(format!("{value_type} value must be a string")))
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::from_reason(format!("Missing or invalid string field '{key}'")))
}

#[cfg(test)]
fn invalid_json_error(kind: &'static str) -> impl Fn(serde_json::Error) -> napi::Error {
    move |error| Error::from_reason(format!("Invalid {kind} descriptor JSON: {error}"))
}

fn json_serialize_error(error: serde_json::Error) -> napi::Error {
    Error::from_reason(format!("Failed to serialize Node runtime JSON: {error}"))
}

fn query_result_to_json(result: QueryResult) -> Result<String> {
    let values = match result {
        QueryResult::Ok => Vec::new(),
        QueryResult::Documents(values) | QueryResult::Rows(values) => values,
    };
    serde_json::to_string(&values).map_err(json_serialize_error)
}

fn parse_tx_type(value: &str) -> Result<TxType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read" => Ok(TxType::Read),
        "write" => Ok(TxType::Write),
        "schema" => Ok(TxType::Schema),
        other => Err(Error::new(
            Status::InvalidArg,
            format!("transaction_type must be 'read', 'write', or 'schema', got {other:?}"),
        )),
    }
}

fn tx_type_name(tx_type: TxType) -> &'static str {
    match tx_type {
        TxType::Read => "read",
        TxType::Write => "write",
        TxType::Schema => "schema",
    }
}

fn napi_secure_connect_options(
    http_port: Option<u32>,
    server_version: Option<String>,
    tls_enabled: Option<bool>,
    tls_root_ca: Option<String>,
) -> Result<type_bridge_orm::SecureConnectOptions> {
    let tls_mode = match (tls_enabled, tls_root_ca) {
        (None | Some(false), None) => type_bridge_orm::TlsMode::Disabled,
        (Some(true), None) => type_bridge_orm::TlsMode::NativeRoots,
        (Some(true), Some(path)) if path.is_empty() => {
            return Err(Error::new(
                Status::InvalidArg,
                "tlsRootCa must not be empty".to_string(),
            ));
        }
        (Some(true), Some(path)) => type_bridge_orm::TlsMode::CustomRootCa(PathBuf::from(path)),
        (Some(false), Some(_)) => {
            return Err(Error::new(
                Status::InvalidArg,
                "tlsRootCa contradicts explicit tlsEnabled=false".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(Error::new(
                Status::InvalidArg,
                "tlsRootCa requires explicit tlsEnabled=true".to_string(),
            ));
        }
    };
    let mut options = type_bridge_orm::SecureConnectOptions {
        tls_mode,
        ..type_bridge_orm::SecureConnectOptions::default()
    };
    if let Some(port) = http_port {
        options.http_port = u16::try_from(port).map_err(|_| {
            Error::new(
                Status::InvalidArg,
                format!("httpPort {port} is out of the valid port range (0–65535)"),
            )
        })?;
    }
    if let Some(version) = server_version {
        options.server_version =
            Some(version.parse::<core_version::Version>().map_err(|error| {
                Error::new(
                    Status::InvalidArg,
                    format!("serverVersion must be a TypeDB semantic version: {error}"),
                )
            })?);
    }
    Ok(options)
}

fn napi_secure_connect_error(error: type_bridge_orm::SecureConnectError) -> napi::Error {
    let status = if error.configuration_code().is_some() {
        Status::InvalidArg
    } else {
        Status::GenericFailure
    };
    Error::new(status, error.to_string())
}

pub(crate) fn napi_orm_error(error: OrmError) -> napi::Error {
    match error {
        OrmError::Match(error) => match_runtime::napi_match_error(error),
        OrmError::DescriptorValidation { .. }
        | OrmError::DescriptorConflict { .. }
        | OrmError::InvalidFilter(_)
        | OrmError::Compilation(_) => Error::new(Status::InvalidArg, error.to_string()),
        OrmError::DescriptorNotFound(_) | OrmError::NotFound(_) => {
            Error::new(Status::GenericFailure, error.to_string())
        }
        OrmError::Connection(_) | OrmError::QueryExecution(_) | OrmError::Transaction(_) => {
            Error::new(Status::GenericFailure, error.to_string())
        }
        _ => Error::new(Status::GenericFailure, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_tls_inputs_follow_the_canonical_truth_table() {
        assert!(matches!(
            napi_secure_connect_options(None, None, None, None)
                .unwrap()
                .tls_mode,
            type_bridge_orm::TlsMode::Disabled
        ));
        assert!(matches!(
            napi_secure_connect_options(None, None, Some(false), None)
                .unwrap()
                .tls_mode,
            type_bridge_orm::TlsMode::Disabled
        ));
        assert!(matches!(
            napi_secure_connect_options(None, None, Some(true), None)
                .unwrap()
                .tls_mode,
            type_bridge_orm::TlsMode::NativeRoots
        ));
        assert!(matches!(
            napi_secure_connect_options(None, None, Some(true), Some("ca.pem".into()))
                .unwrap()
                .tls_mode,
            type_bridge_orm::TlsMode::CustomRootCa(path)
                if path == std::path::Path::new("ca.pem")
        ));
        assert!(
            napi_secure_connect_options(None, None, Some(false), Some("ca.pem".into())).is_err()
        );
        assert!(napi_secure_connect_options(None, None, None, Some("ca.pem".into())).is_err());
        assert!(napi_secure_connect_options(None, None, Some(true), Some(String::new())).is_err());
    }
}
