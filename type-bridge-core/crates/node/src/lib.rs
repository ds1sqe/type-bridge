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
mod migration_catalog_runtime;
mod query_v2_builder_runtime;
mod query_v2_model_remote_runtime;
pub mod query_v2_runtime;
mod runtime_projection;

#[cfg(feature = "contract-test-adapter")]
pub use contract_test_adapter::{
    ProjectionRecordingFixture, new_projection_recording_authority, round_trip_contract_foundation,
};

pub use match_runtime::{
    NodeMatchBindingHandle, NodeMatchFieldHandle, NodeMatchFunctionArgumentHandle,
    NodeMatchFunctionCallHandle, NodeMatchFunctionHandle, NodeMatchFunctionValueHandle,
    NodeMatchOrderHandle, NodeMatchPredicateHandle, NodeMatchQueryHandle, NodeMatchRoleHandle,
    NodeMatchSelectionHandle, NodeMatchSessionHandle, NodeMatchShapeHandle, NodeQueryCancellation,
    NodeQueryExecutionResources, NodeValidatedMatchResultHandle, NodeValidatedThingHandle,
};
pub use migration_catalog_runtime::{
    NodeMigrationApprovalBuilder, NodeMigrationApprovalSet, NodeMigrationCatalog,
    NodeMigrationExecutionReport, NodeMigrationHistoryEntry, NodeMigrationIdentity,
    NodeMigrationPlan, NodeMigrationPreview, NodeMigrationPreviewEntry,
    NodeMigrationVerificationFinding, NodeMigrationVerificationReport, open_migration_catalog,
};
pub use query_v2_model_remote_runtime::{
    NodePendingRemoteModelQuery, NodeRemoteModelQueryContext, query_v2_prepare_remote_model_count,
    query_v2_prepare_remote_model_exists, query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_reduce, query_v2_prepare_remote_model_reduce_by_field,
    query_v2_prepare_remote_model_reduce_by_fields, query_v2_prepare_remote_model_rows,
    query_v2_remote_model_context, query_v2_remote_model_context_with_resources,
};
pub use runtime_projection::{NodeProjectedModelManager, NodeRuntimeProjection};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    managed_scope_id: Option<type_bridge_contract::managed_scope::ManagedScopeId>,
}

impl NodeRustDatabase {
    pub(crate) fn from_handles(
        db: Arc<type_bridge_orm::Database>,
        runtime: Arc<ProviderRuntimeOwner>,
        managed_scope_id: Option<type_bridge_contract::managed_scope::ManagedScopeId>,
    ) -> Self {
        Self {
            db,
            runtime,
            managed_scope_id,
        }
    }

    pub(crate) fn handles(&self) -> (Arc<type_bridge_orm::Database>, Arc<ProviderRuntimeOwner>) {
        (Arc::clone(&self.db), Arc::clone(&self.runtime))
    }

    fn pair_administrator(
        &self,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator> {
        let scope = self.managed_scope_id.clone().ok_or_else(|| {
            Error::new(
                Status::InvalidArg,
                "managed database administration requires verified generated schema authority",
            )
        })?;
        type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator::from_managed_database(
            Arc::clone(&self.db),
            scope,
        )
        .map_err(napi_administration_error)
    }
}

#[napi]
pub struct NodeManagedDatabaseDeletionPlan {
    inner: Option<type_bridge_schema_migration_typedb::ManagedDatabasePairDeletionPlan>,
    runtime: Arc<ProviderRuntimeOwner>,
}

#[napi]
impl NodeManagedDatabaseDeletionPlan {
    #[napi(js_name = "inspectedState")]
    pub fn inspected_state(&self) -> Result<String> {
        self.inner
            .as_ref()
            .map(|plan| node_pair_state(plan.inspected_state()).to_owned())
            .ok_or_else(|| Error::new(Status::InvalidArg, "database deletion plan is closed"))
    }

    #[napi(js_name = "execute")]
    pub fn execute(&mut self) -> Result<String> {
        let plan = self
            .inner
            .take()
            .ok_or_else(|| Error::new(Status::InvalidArg, "database deletion plan is closed"))?;
        self.runtime
            .block_on(plan.execute())
            .map(node_delete_outcome)
            .map(str::to_owned)
            .map_err(napi_administration_error)
    }

    #[napi(js_name = "close")]
    pub fn close(&mut self) {
        self.inner = None;
    }
}

fn node_pair_state(
    state: type_bridge_schema_migration_typedb::ManagedDatabasePairState,
) -> &'static str {
    match state {
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::Absent => "absent",
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::StandaloneManaged => {
            "standalone_managed"
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedPair => "owned_pair",
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedJournalOrphan => {
            "owned_journal_orphan"
        }
    }
}

fn node_delete_outcome(
    outcome: type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome,
) -> &'static str {
    match outcome {
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::AlreadyAbsent => {
            "already_absent"
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedStandaloneManaged => "deleted_standalone_managed",
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedPair => {
            "deleted_owned_pair"
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedJournalOrphan => "deleted_owned_journal_orphan",
    }
}

fn napi_administration_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    Error::new(
        Status::GenericFailure,
        format!("database administration failed [{}]", error.code().as_str()),
    )
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
        self.create_database_outcome().map(|_| ())
    }

    #[napi(js_name = "createDatabaseOutcome")]
    pub fn create_database_outcome(&self) -> Result<String> {
        if self.managed_scope_id.is_some() {
            return self
                .runtime
                .block_on(self.pair_administrator()?.create_database_outcome())
                .map(|outcome| match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created => "created".to_owned(),
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::AlreadyExists => "already_exists".to_owned(),
                })
                .map_err(napi_administration_error);
        }
        self.runtime
            .block_on(self.db.create_database_outcome())
            .map(|outcome| match outcome {
                type_bridge_orm::session::DatabaseCreateOutcome::Created => "created".to_owned(),
                type_bridge_orm::session::DatabaseCreateOutcome::AlreadyExists => {
                    "already_exists".to_owned()
                }
            })
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "deleteDatabase")]
    pub fn delete_database(&self) -> Result<()> {
        if self.managed_scope_id.is_some() {
            return Err(Error::new(
                Status::InvalidArg,
                "managed database deletion requires planDatabaseDelete() and explicit plan execution",
            ));
        }
        self.runtime
            .block_on(self.db.delete_database())
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "deleteDatabaseOutcome")]
    pub fn delete_database_outcome(&self) -> Result<String> {
        if self.managed_scope_id.is_some() {
            return Err(Error::new(
                Status::InvalidArg,
                "managed database deletion requires planDatabaseDelete() and explicit plan execution",
            ));
        }
        self.runtime
            .block_on(self.db.delete_database_outcome())
            .map(|outcome| match outcome {
                type_bridge_orm::session::DatabaseDeleteOutcome::Deleted => "deleted".to_owned(),
                type_bridge_orm::session::DatabaseDeleteOutcome::AlreadyAbsent => {
                    "already_absent".to_owned()
                }
            })
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "resetDatabase")]
    pub fn reset_database(&self) -> Result<()> {
        if self.managed_scope_id.is_some() {
            return Err(Error::new(
                Status::InvalidArg,
                "managed database reset is not a recovery-safe pair operation",
            ));
        }
        self.runtime
            .block_on(async {
                if self.db.database_exists().await? {
                    self.db.delete_database().await?;
                }
                self.db.create_database().await
            })
            .map_err(napi_orm_error)
    }

    #[napi(js_name = "inspectDatabasePair")]
    pub fn inspect_database_pair(&self) -> Result<String> {
        self.runtime
            .block_on(self.pair_administrator()?.inspect())
            .map(node_pair_state)
            .map(str::to_owned)
            .map_err(napi_administration_error)
    }

    #[napi(js_name = "planDatabaseDelete")]
    pub fn plan_database_delete(&self) -> Result<NodeManagedDatabaseDeletionPlan> {
        let inner = self
            .runtime
            .block_on(self.pair_administrator()?.plan_delete())
            .map_err(napi_administration_error)?;
        Ok(NodeManagedDatabaseDeletionPlan {
            inner: Some(inner),
            runtime: Arc::clone(&self.runtime),
        })
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
            successor_batch_invoked: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// JavaScript-facing Rust transaction context.
#[napi]
pub struct NodeRustTransactionContext {
    context: TransactionContext,
    runtime: Arc<ProviderRuntimeOwner>,
    successor_batch_invoked: Arc<AtomicBool>,
}

impl NodeRustTransactionContext {
    pub(crate) fn handles(&self) -> (TransactionContext, Arc<ProviderRuntimeOwner>) {
        (self.context.clone(), Arc::clone(&self.runtime))
    }

    pub(crate) fn successor_batch_marker(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.successor_batch_invoked)
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
        if self.successor_batch_invoked.load(Ordering::Acquire) {
            self.runtime
                .block_on(self.context.commit_sdk())
                .map_err(match_runtime::napi_sdk_diagnostic)
        } else {
            self.runtime
                .block_on(self.context.commit())
                .map_err(napi_orm_error)
        }
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
        managed_scope_id: None,
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
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use type_bridge_orm::error::OrmError;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};

    use super::*;

    struct AdministrationBackend {
        databases: Arc<Mutex<BTreeSet<String>>>,
    }

    impl DriverBackend for AdministrationBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Err(OrmError::Connection("unexpected transaction".into())) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn database_exists(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<bool, OrmError>> {
            let exists = self.databases.lock().unwrap().contains(database);
            Box::pin(async move { Ok(exists) })
        }

        fn create_database(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.databases.lock().unwrap().insert(database.to_owned());
            Box::pin(async { Ok(()) })
        }

        fn delete_database(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.databases.lock().unwrap().remove(database);
            Box::pin(async { Ok(()) })
        }

        fn schema_text(
            &self,
            _database: &str,
        ) -> BoxFuture<'_, std::result::Result<String, OrmError>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    #[test]
    fn node_generated_administration_is_pair_aware_and_plan_owned() {
        let databases = Arc::new(Mutex::new(BTreeSet::new()));
        let database = NodeRustDatabase {
            db: Arc::new(type_bridge_orm::Database::with_backend(
                Box::new(AdministrationBackend {
                    databases: Arc::clone(&databases),
                }),
                "app",
            )),
            runtime: Arc::new(ProviderRuntimeOwner::new().unwrap()),
            managed_scope_id: Some(
                type_bridge_contract::managed_scope::ManagedScopeId::new("generated-scope")
                    .unwrap(),
            ),
        };

        assert_eq!(database.create_database_outcome().unwrap(), "created");
        assert_eq!(
            database.inspect_database_pair().unwrap(),
            "standalone_managed"
        );
        assert!(database.delete_database().is_err());
        let mut plan = database.plan_database_delete().unwrap();
        drop(database);
        assert_eq!(plan.execute().unwrap(), "deleted_standalone_managed");
        assert!(databases.lock().unwrap().is_empty());
    }

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
