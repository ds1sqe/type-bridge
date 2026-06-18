//! NAPI facade over the shared type-bridge Rust ORM runtime.
//!
//! This crate intentionally owns only JavaScript boundary marshalling. The
//! descriptor registry, runtime validation, query construction, CRUD execution,
//! and hydration live in `type_bridge_orm`.

// `napi` generates public glue items that do not inherit Rustdoc comments.
#![allow(missing_docs)]

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::runtime::Runtime;
use type_bridge_core_lib::bindgen::{BindgenOptions, TargetLanguage};
use type_bridge_core_lib::schema::TypeSchema;
use type_bridge_core_lib::version as core_version;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    AttributeValue, DescriptorRegistry, DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp,
    DynamicEntityManager, DynamicEntityRow, DynamicExpr, DynamicRelationManager,
    DynamicRelationRow, DynamicRolePlayerInput, DynamicSort, EntityDescriptor, Filter, OrmError,
    OwnedAttributeDescriptor, RelationDescriptor, SchemaInfo, TransactionContext, TxType,
    TypeDescriptor, ValueType,
};

/// JSON-deserialized spec for expression-tree queries.
///
/// Carries the `{ expr, sort, limit, offset }` object a language binding passes
/// to `queryJson`. `queryCountJson`, `queryAggregateJson`, and
/// `queryGroupByAggregateJson` use only the `expr` field; `sort`/`limit`/`offset`
/// are ignored there.
#[derive(Deserialize)]
struct DynamicQuerySpecJson {
    #[serde(default)]
    expr: Vec<DynamicExprJson>,
    #[serde(default)]
    sort: Vec<DynamicSort>,
    limit: Option<u64>,
    offset: Option<u64>,
}

/// JS-shaped expression tree mirroring [`DynamicExpr`], but with comparison
/// values left as raw JSON so they decode through the binding's
/// `{ value_type, value }` convention (`attribute_value_from_js`) — preserving
/// full `long`/`decimal` precision — instead of serde's native externally-tagged
/// `AttributeValue` shape, which would cap `long` at the JS safe-integer range.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DynamicExprJson {
    Compare {
        attr_name: String,
        operator: DynamicComparisonOp,
        value: Value,
    },
    Iid {
        iid: String,
    },
    IsNull {
        attr_name: String,
        is_null: bool,
    },
    And {
        exprs: Vec<DynamicExprJson>,
    },
    Or {
        exprs: Vec<DynamicExprJson>,
    },
    Not {
        expr: Box<DynamicExprJson>,
    },
    RolePlayer {
        role_name: String,
        expr: Box<DynamicExprJson>,
    },
}

impl DynamicExprJson {
    /// Lower the JS-shaped tree into the shared `DynamicExpr`, decoding each
    /// comparison value through the binding value convention. Values are decoded
    /// without an expected type (role-player branches reference a different
    /// type's attributes whose descriptor is not in scope here); the embedded
    /// `value_type` plus TypeDB execution provide validation.
    fn into_expr(self) -> Result<DynamicExpr> {
        match self {
            Self::Compare {
                attr_name,
                operator,
                value,
            } => Ok(DynamicExpr::Compare {
                attr_name,
                operator,
                value: attribute_value_from_js(&value, None)?,
            }),
            Self::Iid { iid } => Ok(DynamicExpr::Iid { iid }),
            Self::IsNull { attr_name, is_null } => Ok(DynamicExpr::IsNull { attr_name, is_null }),
            Self::And { exprs } => Ok(DynamicExpr::And {
                exprs: exprs_into_exprs(exprs)?,
            }),
            Self::Or { exprs } => Ok(DynamicExpr::Or {
                exprs: exprs_into_exprs(exprs)?,
            }),
            Self::Not { expr } => Ok(DynamicExpr::Not {
                expr: Box::new(expr.into_expr()?),
            }),
            Self::RolePlayer { role_name, expr } => Ok(DynamicExpr::RolePlayer {
                role_name,
                expr: Box::new(expr.into_expr()?),
            }),
        }
    }
}

fn exprs_into_exprs(exprs: Vec<DynamicExprJson>) -> Result<Vec<DynamicExpr>> {
    exprs.into_iter().map(DynamicExprJson::into_expr).collect()
}

/// Decoded query-spec parts produced from a `DynamicQuerySpecJson`.
struct DecodedQuerySpec {
    exprs: Vec<DynamicExpr>,
    sort: Vec<DynamicSort>,
    limit: Option<u64>,
    offset: Option<u64>,
}

impl DynamicQuerySpecJson {
    /// Decode the spec's expression list into shared `DynamicExpr` values.
    fn exprs(self) -> Result<DecodedQuerySpec> {
        let exprs = exprs_into_exprs(self.expr)?;
        Ok(DecodedQuerySpec {
            exprs,
            sort: self.sort,
            limit: self.limit,
            offset: self.offset,
        })
    }
}

/// JavaScript-facing descriptor registry backed by `type_bridge_orm`.
#[allow(missing_docs)]
#[napi]
pub struct NodeDescriptorRegistry {
    inner: Arc<DescriptorRegistry>,
}

#[allow(missing_docs)]
#[napi]
impl NodeDescriptorRegistry {
    /// Create an empty descriptor registry.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DescriptorRegistry::new()),
        }
    }

    /// Register an entity descriptor from canonical JSON and return canonical JSON.
    #[napi(js_name = "registerEntityJson")]
    pub fn register_entity_json(&self, descriptor_json: String) -> Result<String> {
        let descriptor: EntityDescriptor =
            serde_json::from_str(&descriptor_json).map_err(invalid_json_error("entity"))?;
        let registered = self
            .inner
            .register_entity(descriptor)
            .map_err(napi_orm_error)?;
        serde_json::to_string(registered.as_ref()).map_err(json_serialize_error)
    }

    /// Register a relation descriptor from canonical JSON and return canonical JSON.
    #[napi(js_name = "registerRelationJson")]
    pub fn register_relation_json(&self, descriptor_json: String) -> Result<String> {
        let descriptor: RelationDescriptor =
            serde_json::from_str(&descriptor_json).map_err(invalid_json_error("relation"))?;
        let registered = self
            .inner
            .register_relation(descriptor)
            .map_err(napi_orm_error)?;
        serde_json::to_string(registered.as_ref()).map_err(json_serialize_error)
    }

    /// Return an entity descriptor as canonical JSON.
    #[napi(js_name = "entityJson")]
    pub fn entity_json(&self, type_name: String) -> Result<String> {
        let descriptor = self.inner.entity(&type_name).map_err(napi_orm_error)?;
        serde_json::to_string(descriptor.as_ref()).map_err(json_serialize_error)
    }

    /// Return a relation descriptor as canonical JSON.
    #[napi(js_name = "relationJson")]
    pub fn relation_json(&self, type_name: String) -> Result<String> {
        let descriptor = self.inner.relation(&type_name).map_err(napi_orm_error)?;
        serde_json::to_string(descriptor.as_ref()).map_err(json_serialize_error)
    }

    /// Return all registered descriptors as canonical JSON.
    #[napi(js_name = "snapshotJson")]
    pub fn snapshot_json(&self) -> Result<String> {
        let snapshot: Vec<TypeDescriptor> = self.inner.snapshot();
        serde_json::to_string(&snapshot).map_err(json_serialize_error)
    }

    /// Return the registered descriptors as migration-facing SchemaInfo JSON.
    #[napi(js_name = "schemaInfoJson")]
    pub fn schema_info_json(&self) -> Result<String> {
        let snapshot: Vec<TypeDescriptor> = self.inner.snapshot();
        let info = SchemaInfo::from_descriptors(&snapshot);
        serde_json::to_string(&info).map_err(json_serialize_error)
    }
}

impl Default for NodeDescriptorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// JavaScript-facing Rust database handle backed by `type_bridge_orm::Database`.
#[allow(missing_docs)]
#[napi]
pub struct NodeRustDatabase {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<Runtime>,
}

#[allow(missing_docs)]
#[napi]
impl NodeRustDatabase {
    /// Return whether the Rust database connection is open.
    #[napi(js_name = "isConnected")]
    pub fn is_connected(&self) -> bool {
        self.db.is_connected()
    }

    /// Return the database name bound to this handle.
    #[napi(js_name = "databaseName")]
    pub fn database_name(&self) -> String {
        self.db.database_name().to_string()
    }

    /// Return whether the bound database exists on the connected server.
    #[napi(js_name = "databaseExists")]
    pub fn database_exists(&self) -> Result<bool> {
        self.runtime
            .block_on(self.db.database_exists())
            .map_err(napi_orm_error)
    }

    /// Create the bound database if it does not already exist.
    #[napi(js_name = "createDatabase")]
    pub fn create_database(&self) -> Result<()> {
        self.runtime
            .block_on(self.db.create_database())
            .map_err(napi_orm_error)
    }

    /// Delete the bound database if it exists.
    #[napi(js_name = "deleteDatabase")]
    pub fn delete_database(&self) -> Result<()> {
        self.runtime
            .block_on(self.db.delete_database())
            .map_err(napi_orm_error)
    }

    /// Delete the bound database if present, then create it.
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

    /// Open a Rust-owned transaction context.
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

    /// Create a dynamic entity manager backed by this database handle.
    #[napi(js_name = "entityManagerJson")]
    pub fn entity_manager_json(&self, descriptor_json: String) -> Result<NodeDynamicEntityManager> {
        let descriptor: EntityDescriptor = serde_json::from_str(&descriptor_json)
            .map_err(invalid_json_error("entity descriptor"))?;
        Ok(NodeDynamicEntityManager {
            db: Some(Arc::clone(&self.db)),
            tx: None,
            runtime: Arc::clone(&self.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Create a dynamic relation manager backed by this database handle.
    #[napi(js_name = "relationManagerJson")]
    pub fn relation_manager_json(
        &self,
        descriptor_json: String,
    ) -> Result<NodeDynamicRelationManager> {
        let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
            .map_err(invalid_json_error("relation descriptor"))?;
        Ok(NodeDynamicRelationManager {
            db: Some(Arc::clone(&self.db)),
            tx: None,
            runtime: Arc::clone(&self.runtime),
            descriptor: Arc::new(descriptor),
        })
    }
}

/// JavaScript-facing Rust transaction context.
#[allow(missing_docs)]
#[napi]
pub struct NodeRustTransactionContext {
    context: TransactionContext,
    runtime: Arc<Runtime>,
}

#[allow(missing_docs)]
#[napi]
impl NodeRustTransactionContext {
    /// Execute a raw query in this Rust transaction and return rows as JSON.
    #[napi(js_name = "queryJson")]
    pub fn query_json(&self, query: String) -> Result<String> {
        let result = self
            .runtime
            .block_on(self.context.query(&query))
            .map_err(napi_orm_error)?;
        query_result_to_json(result)
    }

    /// Commit this Rust transaction.
    #[napi(js_name = "commit")]
    pub fn commit(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.commit())
            .map_err(napi_orm_error)
    }

    /// Roll back this Rust transaction.
    #[napi(js_name = "rollback")]
    pub fn rollback(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.rollback())
            .map_err(napi_orm_error)
    }

    /// Close this Rust transaction without committing.
    #[napi(js_name = "close")]
    pub fn close(&self) -> Result<()> {
        self.runtime
            .block_on(self.context.close())
            .map_err(napi_orm_error)
    }

    /// Return this transaction's type name.
    #[napi(js_name = "transactionType")]
    pub fn transaction_type(&self) -> String {
        tx_type_name(self.context.tx_type()).to_string()
    }

    /// Create a dynamic entity manager bound to this transaction.
    #[napi(js_name = "entityManagerJson")]
    pub fn entity_manager_json(&self, descriptor_json: String) -> Result<NodeDynamicEntityManager> {
        let descriptor: EntityDescriptor = serde_json::from_str(&descriptor_json)
            .map_err(invalid_json_error("entity descriptor"))?;
        Ok(NodeDynamicEntityManager {
            db: None,
            tx: Some(self.context.clone()),
            runtime: Arc::clone(&self.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Create a dynamic relation manager bound to this transaction.
    #[napi(js_name = "relationManagerJson")]
    pub fn relation_manager_json(
        &self,
        descriptor_json: String,
    ) -> Result<NodeDynamicRelationManager> {
        let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
            .map_err(invalid_json_error("relation descriptor"))?;
        Ok(NodeDynamicRelationManager {
            db: None,
            tx: Some(self.context.clone()),
            runtime: Arc::clone(&self.runtime),
            descriptor: Arc::new(descriptor),
        })
    }
}

/// JavaScript-facing dynamic entity manager.
#[allow(missing_docs)]
#[napi]
pub struct NodeDynamicEntityManager {
    db: Option<Arc<type_bridge_orm::Database>>,
    tx: Option<TransactionContext>,
    runtime: Arc<Runtime>,
    descriptor: Arc<EntityDescriptor>,
}

#[allow(missing_docs)]
#[napi]
impl NodeDynamicEntityManager {
    /// Insert one entity and return its IID.
    #[napi(js_name = "insertJson")]
    pub fn insert_json(&self, attributes_json: String) -> Result<String> {
        let attributes = entity_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert(&attributes))
            .map_err(napi_orm_error)
    }

    /// Insert multiple entities through the shared Rust batch API.
    #[napi(js_name = "insertManyJson")]
    pub fn insert_many_json(&self, batch_json: String) -> Result<String> {
        let batch = entity_attribute_list_from_json_string(&self.descriptor, &batch_json)?;
        let manager = self.manager()?;
        let iids = self
            .runtime
            .block_on(manager.insert_many(&batch))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&iids).map_err(json_serialize_error)
    }

    /// Put one entity and return its IID.
    #[napi(js_name = "putJson")]
    pub fn put_json(&self, attributes_json: String) -> Result<String> {
        let attributes = entity_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put(&attributes))
            .map_err(napi_orm_error)
    }

    /// Put multiple entities through the shared Rust batch API.
    #[napi(js_name = "putManyJson")]
    pub fn put_many_json(&self, batch_json: String) -> Result<String> {
        let batch = entity_attribute_list_from_json_string(&self.descriptor, &batch_json)?;
        let manager = self.manager()?;
        let iids = self
            .runtime
            .block_on(manager.put_many(&batch))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&iids).map_err(json_serialize_error)
    }

    /// Update one entity's non-key attributes.
    #[napi(js_name = "updateJson")]
    pub fn update_json(&self, attributes_json: String, iid: Option<String>) -> Result<()> {
        let attributes = entity_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.update(iid.as_deref(), &attributes))
            .map_err(napi_orm_error)
    }

    /// Fetch entities matching attribute filters and return row JSON.
    #[napi(js_name = "getJson")]
    pub fn get_json(&self, filters_json: Option<String>) -> Result<String> {
        let filters = entity_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(napi_orm_error)?;
        entity_rows_to_json(&rows)
    }

    /// Fetch one entity by TypeDB IID and return row JSON or null.
    #[napi(js_name = "getByIidJson")]
    pub fn get_by_iid_json(&self, iid: String) -> Result<String> {
        let manager = self.manager()?;
        let row = self
            .runtime
            .block_on(manager.get_by_iid(&iid))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&row.as_ref().map(entity_row_to_json)).map_err(json_serialize_error)
    }

    /// Fetch all entities for this descriptor.
    #[napi(js_name = "allJson")]
    pub fn all_json(&self) -> Result<String> {
        self.get_json(None)
    }

    /// Count entities matching attribute filters.
    #[napi(js_name = "countJson")]
    pub fn count_json(&self, filters_json: Option<String>) -> Result<String> {
        let filters = entity_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let manager = self.manager()?;
        let count = self
            .runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(napi_orm_error)?;
        Ok(count.to_string())
    }

    /// Run aggregate reductions over entities matching attribute filters.
    #[napi(js_name = "aggregateJson")]
    pub fn aggregate_json(
        &self,
        aggregates_json: String,
        filters_json: Option<String>,
    ) -> Result<String> {
        let filters = entity_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate(&filters, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Run grouped aggregate reductions over entities matching attribute filters.
    #[napi(js_name = "groupByAggregateJson")]
    pub fn group_by_aggregate_json(
        &self,
        group_fields_json: String,
        aggregates_json: String,
        filters_json: Option<String>,
    ) -> Result<String> {
        let filters = entity_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let group_fields =
            group_fields_from_json_string(&self.descriptor.owned_attributes, &group_fields_json)?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate(&filters, &group_fields, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Delete one entity by IID.
    #[napi(js_name = "deleteByIid")]
    pub fn delete_by_iid(&self, iid: String) -> Result<()> {
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.delete_by_iid(&iid))
            .map_err(napi_orm_error)
    }

    /// Fetch entities matching an expression-tree query spec.
    ///
    /// `spec_json`: `{ "expr": DynamicExpr[], "sort": DynamicSort[],
    /// "limit": number|null, "offset": number|null }`.
    #[napi(js_name = "queryJson")]
    pub fn query_json(&self, spec_json: String) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec {
            exprs,
            sort,
            limit,
            offset,
        } = spec.exprs()?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_query(&exprs, &sort, limit, offset))
            .map_err(napi_orm_error)?;
        entity_rows_to_json(&rows)
    }

    /// Count entities matching an expression-tree query spec. Only `expr` is
    /// used; `sort`/`limit`/`offset` are ignored.
    #[napi(js_name = "queryCountJson")]
    pub fn query_count_json(&self, spec_json: String) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let manager = self.manager()?;
        let count = self
            .runtime
            .block_on(manager.count_with_query(&exprs))
            .map_err(napi_orm_error)?;
        Ok(count.to_string())
    }

    /// Run aggregate reductions over entities matching an expression-tree query
    /// spec. `aggregates_json`: `DynamicAggregate[]`.
    #[napi(js_name = "queryAggregateJson")]
    pub fn query_aggregate_json(
        &self,
        spec_json: String,
        aggregates_json: String,
    ) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate_with_query(&exprs, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Run grouped aggregate reductions over entities matching an expression-tree
    /// query spec. `group_fields_json`: `string[]`; `aggregates_json`:
    /// `DynamicAggregate[]`.
    #[napi(js_name = "queryGroupByAggregateJson")]
    pub fn query_group_by_aggregate_json(
        &self,
        spec_json: String,
        group_fields_json: String,
        aggregates_json: String,
    ) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let group_fields =
            group_fields_from_json_string(&self.descriptor.owned_attributes, &group_fields_json)?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate_with_query(&exprs, &group_fields, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }
}

impl NodeDynamicEntityManager {
    fn manager(&self) -> Result<DynamicEntityManager<'_>> {
        if let Some(tx) = &self.tx {
            return Ok(DynamicEntityManager::with_transaction(
                tx.clone(),
                Arc::clone(&self.descriptor),
            ));
        }
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| Error::from_reason("Rust entity manager has no execution target"))?;
        Ok(DynamicEntityManager::new(db, Arc::clone(&self.descriptor)))
    }
}

/// JavaScript-facing dynamic relation manager.
#[allow(missing_docs)]
#[napi]
pub struct NodeDynamicRelationManager {
    db: Option<Arc<type_bridge_orm::Database>>,
    tx: Option<TransactionContext>,
    runtime: Arc<Runtime>,
    descriptor: Arc<RelationDescriptor>,
}

#[allow(missing_docs)]
#[napi]
impl NodeDynamicRelationManager {
    /// Insert one relation and return its IID.
    #[napi(js_name = "insertJson")]
    pub fn insert_json(
        &self,
        attributes_json: String,
        role_players_json: String,
    ) -> Result<String> {
        let attributes = relation_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let role_players = role_players_from_json_string(&self.descriptor, &role_players_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert(&attributes, &role_players))
            .map_err(napi_orm_error)
    }

    /// Insert multiple relations through the shared Rust batch API.
    #[napi(js_name = "insertManyJson")]
    pub fn insert_many_json(&self, batch_json: String) -> Result<String> {
        let batch = relation_write_batch_from_json_string(&self.descriptor, &batch_json)?;
        let manager = self.manager()?;
        let iids = self
            .runtime
            .block_on(manager.insert_many(&batch))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&iids).map_err(json_serialize_error)
    }

    /// Put one relation and return its IID.
    #[napi(js_name = "putJson")]
    pub fn put_json(&self, attributes_json: String, role_players_json: String) -> Result<String> {
        let attributes = relation_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let role_players = role_players_from_json_string(&self.descriptor, &role_players_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put(&attributes, &role_players))
            .map_err(napi_orm_error)
    }

    /// Put multiple relations through the shared Rust batch API.
    #[napi(js_name = "putManyJson")]
    pub fn put_many_json(&self, batch_json: String) -> Result<String> {
        let batch = relation_write_batch_from_json_string(&self.descriptor, &batch_json)?;
        let manager = self.manager()?;
        let iids = self
            .runtime
            .block_on(manager.put_many(&batch))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&iids).map_err(json_serialize_error)
    }

    /// Update one relation's non-key attributes.
    #[napi(js_name = "updateJson")]
    pub fn update_json(
        &self,
        attributes_json: String,
        role_players_json: String,
        iid: Option<String>,
    ) -> Result<()> {
        let attributes = relation_attributes_from_json_string(&self.descriptor, &attributes_json)?;
        let role_players = role_players_from_json_string(&self.descriptor, &role_players_json)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.update(iid.as_deref(), &attributes, &role_players))
            .map_err(napi_orm_error)
    }

    /// Fetch relations matching attribute filters and return row JSON.
    #[napi(js_name = "getJson")]
    pub fn get_json(&self, filters_json: Option<String>) -> Result<String> {
        let filters = relation_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(napi_orm_error)?;
        relation_rows_to_json(&rows)
    }

    /// Fetch relations matching attribute and role-player filters.
    #[napi(js_name = "getWithRolePlayersJson")]
    pub fn get_with_role_players_json(
        &self,
        filters_json: Option<String>,
        role_players_json: Option<String>,
    ) -> Result<String> {
        let filters = relation_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let role_players = match role_players_json.as_deref() {
            Some(value) => role_players_from_json_string(&self.descriptor, value)?,
            None => vec![],
        };
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_role_filters(&filters, &role_players))
            .map_err(napi_orm_error)?;
        relation_rows_to_json(&rows)
    }

    /// Fetch relation rows by TypeDB IID.
    #[napi(js_name = "getByIidJson")]
    pub fn get_by_iid_json(&self, iid: String) -> Result<String> {
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_by_iid(&iid))
            .map_err(napi_orm_error)?;
        relation_rows_to_json(&rows)
    }

    /// Fetch all relations for this descriptor.
    #[napi(js_name = "allJson")]
    pub fn all_json(&self) -> Result<String> {
        self.get_json(None)
    }

    /// Count relations matching attribute filters.
    #[napi(js_name = "countJson")]
    pub fn count_json(&self, filters_json: Option<String>) -> Result<String> {
        let filters = relation_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let manager = self.manager()?;
        let count = self
            .runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(napi_orm_error)?;
        Ok(count.to_string())
    }

    /// Run aggregate reductions over relations matching attribute filters.
    #[napi(js_name = "aggregateJson")]
    pub fn aggregate_json(
        &self,
        aggregates_json: String,
        filters_json: Option<String>,
    ) -> Result<String> {
        let filters = relation_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate(&filters, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Run grouped aggregate reductions over relations matching attribute filters.
    #[napi(js_name = "groupByAggregateJson")]
    pub fn group_by_aggregate_json(
        &self,
        group_fields_json: String,
        aggregates_json: String,
        filters_json: Option<String>,
    ) -> Result<String> {
        let filters = relation_filters_from_json_string(&self.descriptor, filters_json.as_deref())?;
        let group_fields =
            group_fields_from_json_string(&self.descriptor.owned_attributes, &group_fields_json)?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate(&filters, &group_fields, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Delete one relation by IID.
    #[napi(js_name = "deleteByIid")]
    pub fn delete_by_iid(&self, iid: String) -> Result<()> {
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.delete_by_iid(&iid))
            .map_err(napi_orm_error)
    }

    /// Fetch relations matching an expression-tree query spec.
    ///
    /// `spec_json`: `{ "expr": DynamicExpr[], "sort": DynamicSort[],
    /// "limit": number|null, "offset": number|null }`.
    #[napi(js_name = "queryJson")]
    pub fn query_json(&self, spec_json: String) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec {
            exprs,
            sort,
            limit,
            offset,
        } = spec.exprs()?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_query(&exprs, &sort, limit, offset))
            .map_err(napi_orm_error)?;
        relation_rows_to_json(&rows)
    }

    /// Count relations matching an expression-tree query spec. Only `expr` is
    /// used; `sort`/`limit`/`offset` are ignored.
    #[napi(js_name = "queryCountJson")]
    pub fn query_count_json(&self, spec_json: String) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let manager = self.manager()?;
        let count = self
            .runtime
            .block_on(manager.count_with_query(&exprs))
            .map_err(napi_orm_error)?;
        Ok(count.to_string())
    }

    /// Run aggregate reductions over relations matching an expression-tree query
    /// spec. `aggregates_json`: `DynamicAggregate[]`.
    #[napi(js_name = "queryAggregateJson")]
    pub fn query_aggregate_json(
        &self,
        spec_json: String,
        aggregates_json: String,
    ) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate_with_query(&exprs, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }

    /// Run grouped aggregate reductions over relations matching an
    /// expression-tree query spec. `group_fields_json`: `string[]`;
    /// `aggregates_json`: `DynamicAggregate[]`.
    #[napi(js_name = "queryGroupByAggregateJson")]
    pub fn query_group_by_aggregate_json(
        &self,
        spec_json: String,
        group_fields_json: String,
        aggregates_json: String,
    ) -> Result<String> {
        let spec: DynamicQuerySpecJson =
            serde_json::from_str(&spec_json).map_err(invalid_json_error("query spec"))?;
        let DecodedQuerySpec { exprs, .. } = spec.exprs()?;
        let group_fields =
            group_fields_from_json_string(&self.descriptor.owned_attributes, &group_fields_json)?;
        let aggregates =
            aggregates_from_json_string(&self.descriptor.owned_attributes, &aggregates_json)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate_with_query(&exprs, &group_fields, &aggregates))
            .map_err(napi_orm_error)?;
        serde_json::to_string(&rows).map_err(json_serialize_error)
    }
}

impl NodeDynamicRelationManager {
    fn manager(&self) -> Result<DynamicRelationManager<'_>> {
        if let Some(tx) = &self.tx {
            return Ok(DynamicRelationManager::with_transaction(
                tx.clone(),
                Arc::clone(&self.descriptor),
            ));
        }
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| Error::from_reason("Rust relation manager has no execution target"))?;
        Ok(DynamicRelationManager::new(
            db,
            Arc::clone(&self.descriptor),
        ))
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
///
/// Fails hard when TypeDB is unreachable — the caller decides whether to
/// propagate or surface the error; tests should propagate it so that a
/// missing server is a clear failure rather than a silent skip.
#[napi(js_name = "ensureRustDatabase")]
pub fn ensure_rust_database(
    address: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
    http_port: Option<u32>,
    server_version: Option<String>,
) -> Result<()> {
    let runtime = Runtime::new().map(Arc::new).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create Tokio runtime: {error}"),
        )
    })?;
    let username = username.unwrap_or_else(|| "admin".to_string());
    let password = password.unwrap_or_else(|| "password".to_string());
    let options = napi_connect_options(http_port, server_version)?;
    runtime
        .block_on(type_bridge_orm::ensure_database_exists(
            &address, &database, &username, &password, options,
        ))
        .map_err(napi_orm_error)
}

/// Connect to TypeDB using the shared Rust ORM session layer.
#[napi(js_name = "connectRustDatabase")]
pub fn connect_rust_database(
    address: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
    http_port: Option<u32>,
    server_version: Option<String>,
) -> Result<NodeRustDatabase> {
    let runtime = Runtime::new().map(Arc::new).map_err(|error| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create Tokio runtime: {error}"),
        )
    })?;
    let username = username.unwrap_or_else(|| "admin".to_string());
    let password = password.unwrap_or_else(|| "password".to_string());
    let options = napi_connect_options(http_port, server_version)?;
    let db = runtime
        .block_on(type_bridge_orm::Database::connect_with_options(
            &address, &database, &username, &password, options,
        ))
        .map_err(napi_orm_error)?;

    Ok(NodeRustDatabase {
        db: Arc::new(db),
        runtime,
    })
}

/// Normalize one TypeScript attribute value object into canonical Rust JSON.
#[napi(js_name = "normalizeAttributeValueJson")]
pub fn normalize_attribute_value_json(value_json: String) -> Result<String> {
    let value: Value =
        serde_json::from_str(&value_json).map_err(invalid_json_error("attribute value"))?;
    let value = attribute_value_from_js(&value, None)?;
    serde_json::to_string(&value).map_err(json_serialize_error)
}

/// Normalize entity attributes into the shared Rust dynamic attribute map JSON.
#[napi(js_name = "normalizeEntityAttributesJson")]
pub fn normalize_entity_attributes_json(
    descriptor_json: String,
    attributes_json: String,
) -> Result<String> {
    let descriptor: EntityDescriptor =
        serde_json::from_str(&descriptor_json).map_err(invalid_json_error("entity descriptor"))?;
    let attributes: Value =
        serde_json::from_str(&attributes_json).map_err(invalid_json_error("attributes"))?;
    let attributes = attributes_from_json(&descriptor.owned_attributes, &attributes)?;
    serde_json::to_string(&attributes).map_err(json_serialize_error)
}

/// Normalize relation attributes into the shared Rust dynamic attribute map JSON.
#[napi(js_name = "normalizeRelationAttributesJson")]
pub fn normalize_relation_attributes_json(
    descriptor_json: String,
    attributes_json: String,
) -> Result<String> {
    let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
        .map_err(invalid_json_error("relation descriptor"))?;
    let attributes: Value =
        serde_json::from_str(&attributes_json).map_err(invalid_json_error("attributes"))?;
    let attributes = attributes_from_json(&descriptor.owned_attributes, &attributes)?;
    serde_json::to_string(&attributes).map_err(json_serialize_error)
}

/// Normalize filters into the shared Rust filter JSON.
#[napi(js_name = "normalizeFiltersJson")]
pub fn normalize_filters_json(descriptor_json: String, filters_json: String) -> Result<String> {
    let descriptor: EntityDescriptor =
        serde_json::from_str(&descriptor_json).map_err(invalid_json_error("entity descriptor"))?;
    let filters: Value =
        serde_json::from_str(&filters_json).map_err(invalid_json_error("filters"))?;
    let filters = filters_from_json(&descriptor.owned_attributes, &filters)?;
    serde_json::to_string(&filters).map_err(json_serialize_error)
}

/// Normalize relation filters into the shared Rust filter JSON.
#[napi(js_name = "normalizeRelationFiltersJson")]
pub fn normalize_relation_filters_json(
    descriptor_json: String,
    filters_json: String,
) -> Result<String> {
    let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
        .map_err(invalid_json_error("relation descriptor"))?;
    let filters: Value =
        serde_json::from_str(&filters_json).map_err(invalid_json_error("filters"))?;
    let filters = filters_from_json(&descriptor.owned_attributes, &filters)?;
    serde_json::to_string(&filters).map_err(json_serialize_error)
}

/// Normalize aggregate requests into the shared Rust aggregate JSON.
#[napi(js_name = "normalizeAggregatesJson")]
pub fn normalize_aggregates_json(
    descriptor_json: String,
    aggregates_json: String,
) -> Result<String> {
    let descriptor: EntityDescriptor =
        serde_json::from_str(&descriptor_json).map_err(invalid_json_error("entity descriptor"))?;
    let aggregates: Value =
        serde_json::from_str(&aggregates_json).map_err(invalid_json_error("aggregates"))?;
    let aggregates = aggregates_from_json(&descriptor.owned_attributes, &aggregates)?;
    serde_json::to_string(&aggregates).map_err(json_serialize_error)
}

/// Normalize relation role-player inputs into the shared Rust role-player JSON.
#[napi(js_name = "normalizeRolePlayersJson")]
pub fn normalize_role_players_json(
    descriptor_json: String,
    role_players_json: String,
) -> Result<String> {
    let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
        .map_err(invalid_json_error("relation descriptor"))?;
    let role_players: Value =
        serde_json::from_str(&role_players_json).map_err(invalid_json_error("role players"))?;
    let role_players = role_players_from_json(&descriptor, &role_players)?;
    serde_json::to_string(&role_players).map_err(json_serialize_error)
}

/// Normalize relation write batch input into shared Rust dynamic batch JSON.
#[napi(js_name = "normalizeRelationWriteBatchJson")]
pub fn normalize_relation_write_batch_json(
    descriptor_json: String,
    batch_json: String,
) -> Result<String> {
    let descriptor: RelationDescriptor = serde_json::from_str(&descriptor_json)
        .map_err(invalid_json_error("relation descriptor"))?;
    let batch: Value =
        serde_json::from_str(&batch_json).map_err(invalid_json_error("relation batch"))?;
    let batch = relation_write_batch_from_json(&descriptor, &batch)?;
    serde_json::to_string(&batch).map_err(json_serialize_error)
}

/// Parse a TQL `define` block and return the resolved [`TypeSchema`] as a
/// JSON string.
///
/// This is a marshalling-only binding: parse + inheritance resolution happen in
/// the shared Rust core; the result is serialized straight to JSON. No query
/// construction, no generation policy.
#[napi(js_name = "parseSchemaJson")]
pub fn parse_schema_json(input: String) -> Result<String> {
    let schema = TypeSchema::from_typeql(&input)
        .map_err(|e| Error::from_reason(format!("Failed to parse schema definition: {e}")))?;
    schema
        .to_json()
        .map_err(|e| Error::from_reason(format!("Failed to serialize schema JSON: {e}")))
}

/// Render generated model files as a JSON package for a target language.
#[napi(js_name = "renderModelsJson")]
pub fn render_models_json(
    input: String,
    target: String,
    options_json: Option<String>,
) -> Result<String> {
    let target: TargetLanguage = target
        .parse()
        .map_err(|e| Error::from_reason(format!("Invalid bindgen target: {e}")))?;
    let options: BindgenOptions = match options_json {
        Some(options_json) => {
            serde_json::from_str(&options_json).map_err(invalid_json_error("bindgen options"))?
        }
        None => BindgenOptions::default(),
    };
    type_bridge_core_lib::bindgen::generate_json_from_typeql(&input, target, &options)
        .map_err(|e| Error::from_reason(format!("Failed to render models: {e}")))
}

/// Generate a TypeQL `define` block from serialized SchemaInfo JSON.
#[napi(js_name = "generateDefineBlockJson")]
pub fn generate_define_block_json(schema_info_json: String) -> Result<String> {
    let info: SchemaInfo =
        serde_json::from_str(&schema_info_json).map_err(invalid_json_error("schema info"))?;
    info.validate()
        .map_err(|error| Error::from_reason(error.to_string()))?;
    Ok(type_bridge_orm::schema::generator::generate_define_block(
        &info,
    ))
}

fn attributes_from_json(
    descriptors: &[OwnedAttributeDescriptor],
    value: &Value,
) -> Result<DynamicAttributeMap> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::from_reason("Attributes must be an object"))?;
    let mut attributes = Vec::new();

    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        let descriptor = find_attribute(descriptors, key)
            .ok_or_else(|| Error::from_reason(format!("Unknown attribute '{key}'")))?;
        if let Some(values) = value.as_array() {
            for item in values {
                attributes.push((
                    descriptor.attr_name.clone(),
                    attribute_value_from_js(item, Some(descriptor.value_type))?,
                ));
            }
        } else {
            attributes.push((
                descriptor.attr_name.clone(),
                attribute_value_from_js(value, Some(descriptor.value_type))?,
            ));
        }
    }

    Ok(attributes)
}

fn filters_from_json(
    descriptors: &[OwnedAttributeDescriptor],
    value: &Value,
) -> Result<Vec<Filter>> {
    if value.is_null() {
        return Ok(vec![]);
    }
    if let Some(items) = value.as_array() {
        let mut filters = Vec::with_capacity(items.len());
        for item in items {
            let obj = item
                .as_object()
                .ok_or_else(|| Error::from_reason("Each filter must be an object"))?;
            let attr_name = required_string(obj, "attr_name")?;
            let descriptor = find_attribute(descriptors, &attr_name).ok_or_else(|| {
                Error::from_reason(format!("Unknown filter attribute '{attr_name}'"))
            })?;
            let operator = obj
                .get("operator")
                .and_then(Value::as_str)
                .unwrap_or("==")
                .to_string();
            let value = obj
                .get("value")
                .ok_or_else(|| Error::from_reason("Filter missing value"))?;
            filters.push(Filter::compare(
                descriptor.attr_name.clone(),
                operator,
                attribute_value_from_js(value, Some(descriptor.value_type))?,
            ));
        }
        return Ok(filters);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| Error::from_reason("Filters must be an object or array"))?;
    let mut filters = Vec::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        let descriptor = find_attribute(descriptors, key)
            .ok_or_else(|| Error::from_reason(format!("Unknown filter attribute '{key}'")))?;
        filters.push(Filter::eq(
            descriptor.attr_name.clone(),
            attribute_value_from_js(value, Some(descriptor.value_type))?,
        ));
    }
    Ok(filters)
}

fn aggregates_from_json(
    descriptors: &[OwnedAttributeDescriptor],
    value: &Value,
) -> Result<Vec<DynamicAggregate>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::from_reason("Aggregates must be an array"))?;
    let mut aggregates = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| Error::from_reason("Each aggregate must be an object"))?;
        let result_key = required_string(obj, "result_key")?;
        let function = required_string(obj, "function")?;
        let attr_name = match obj.get("attr_name") {
            Some(Value::Null) | None => None,
            Some(value) => {
                let attr_name = value
                    .as_str()
                    .ok_or_else(|| Error::from_reason("Aggregate attr_name must be a string"))?;
                let descriptor = find_attribute(descriptors, attr_name).ok_or_else(|| {
                    Error::from_reason(format!("Unknown aggregate attribute '{attr_name}'"))
                })?;
                Some(descriptor.attr_name.clone())
            }
        };
        aggregates.push(DynamicAggregate {
            result_key,
            function,
            attr_name,
        });
    }
    Ok(aggregates)
}

fn role_players_from_json(
    descriptor: &RelationDescriptor,
    value: &Value,
) -> Result<Vec<DynamicRolePlayerInput>> {
    let players = value
        .as_array()
        .ok_or_else(|| Error::from_reason("Role players must be an array"))?;
    let mut inputs = Vec::with_capacity(players.len());

    for player in players {
        let obj = player
            .as_object()
            .ok_or_else(|| Error::from_reason("Each role player must be an object"))?;
        let role_name = required_string(obj, "role_name")?;
        // Validate that the role exists, but not the player's concrete type.
        // A role's declared player_type_names are the (possibly abstract)
        // declared targets; any concrete subtype is a legal player. Subtype
        // compatibility is enforced by TypeDB at insert time — mirroring the
        // orm backend, which performs no player-type membership check here.
        if descriptor.role(&role_name).is_none() {
            return Err(Error::from_reason(format!("Unknown role '{role_name}'")));
        }
        let player_type_name = required_string(obj, "player_type_name")?;

        let iid = obj
            .get("iid")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let key = match (obj.get("key_attr"), obj.get("key_value")) {
            (Some(key_attr), Some(key_value)) => {
                let key_attr = key_attr
                    .as_str()
                    .ok_or_else(|| Error::from_reason("key_attr must be a string"))?;
                let value = attribute_value_from_js(key_value, None)?;
                Some((key_attr.to_string(), value))
            }
            _ => None,
        };

        if iid.is_none() && key.is_none() {
            return Err(Error::from_reason(format!(
                "Role player for role '{role_name}' requires iid or key fields"
            )));
        }

        inputs.push(DynamicRolePlayerInput {
            role_name,
            player_type_name,
            iid,
            key,
        });
    }

    Ok(inputs)
}

fn relation_write_batch_from_json(
    descriptor: &RelationDescriptor,
    value: &Value,
) -> Result<Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::from_reason("Relation batch must be an array"))?;
    let mut batch = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| Error::from_reason("Each relation batch item must be an object"))?;
        let attributes = obj.get("attributes").unwrap_or(&Value::Null);
        let attributes = if attributes.is_null() {
            vec![]
        } else {
            attributes_from_json(&descriptor.owned_attributes, attributes)?
        };
        let role_players = obj
            .get("role_players")
            .ok_or_else(|| Error::from_reason("Relation batch item missing role_players"))?;
        batch.push((
            attributes,
            role_players_from_json(descriptor, role_players)?,
        ));
    }
    Ok(batch)
}

fn entity_attributes_from_json_string(
    descriptor: &EntityDescriptor,
    value: &str,
) -> Result<DynamicAttributeMap> {
    let value: Value =
        serde_json::from_str(value).map_err(invalid_json_error("entity attributes"))?;
    attributes_from_json(&descriptor.owned_attributes, &value)
}

fn relation_attributes_from_json_string(
    descriptor: &RelationDescriptor,
    value: &str,
) -> Result<DynamicAttributeMap> {
    let value: Value =
        serde_json::from_str(value).map_err(invalid_json_error("relation attributes"))?;
    attributes_from_json(&descriptor.owned_attributes, &value)
}

fn entity_attribute_list_from_json_string(
    descriptor: &EntityDescriptor,
    value: &str,
) -> Result<Vec<DynamicAttributeMap>> {
    let value: Value =
        serde_json::from_str(value).map_err(invalid_json_error("entity attribute batch"))?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::from_reason("Entity attribute batch must be an array"))?;
    items
        .iter()
        .map(|item| attributes_from_json(&descriptor.owned_attributes, item))
        .collect()
}

fn entity_filters_from_json_string(
    descriptor: &EntityDescriptor,
    value: Option<&str>,
) -> Result<Vec<Filter>> {
    match value {
        Some(value) => {
            let value: Value =
                serde_json::from_str(value).map_err(invalid_json_error("entity filters"))?;
            filters_from_json(&descriptor.owned_attributes, &value)
        }
        None => Ok(vec![]),
    }
}

fn relation_filters_from_json_string(
    descriptor: &RelationDescriptor,
    value: Option<&str>,
) -> Result<Vec<Filter>> {
    match value {
        Some(value) => {
            let value: Value =
                serde_json::from_str(value).map_err(invalid_json_error("relation filters"))?;
            filters_from_json(&descriptor.owned_attributes, &value)
        }
        None => Ok(vec![]),
    }
}

fn aggregates_from_json_string(
    descriptors: &[OwnedAttributeDescriptor],
    value: &str,
) -> Result<Vec<DynamicAggregate>> {
    let value: Value = serde_json::from_str(value).map_err(invalid_json_error("aggregates"))?;
    aggregates_from_json(descriptors, &value)
}

fn group_fields_from_json_string(
    descriptors: &[OwnedAttributeDescriptor],
    value: &str,
) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(value).map_err(invalid_json_error("group fields"))?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::from_reason("Group fields must be an array of strings"))?;
    let mut group_fields = Vec::with_capacity(items.len());
    for item in items {
        let field = item
            .as_str()
            .ok_or_else(|| Error::from_reason("Group field must be a string"))?;
        let descriptor = find_attribute(descriptors, field)
            .ok_or_else(|| Error::from_reason(format!("Unknown group field '{field}'")))?;
        group_fields.push(descriptor.attr_name.clone());
    }
    Ok(group_fields)
}

fn role_players_from_json_string(
    descriptor: &RelationDescriptor,
    value: &str,
) -> Result<Vec<DynamicRolePlayerInput>> {
    let value: Value = serde_json::from_str(value).map_err(invalid_json_error("role players"))?;
    role_players_from_json(descriptor, &value)
}

fn relation_write_batch_from_json_string(
    descriptor: &RelationDescriptor,
    value: &str,
) -> Result<Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)>> {
    let value: Value =
        serde_json::from_str(value).map_err(invalid_json_error("relation write batch"))?;
    relation_write_batch_from_json(descriptor, &value)
}

fn attribute_value_from_js(
    value: &Value,
    expected_type: Option<ValueType>,
) -> Result<AttributeValue> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::from_reason("Attribute value must be an object"))?;
    let value_type_name = required_string(obj, "value_type")?;
    let value_type = ValueType::parse(&value_type_name).ok_or_else(|| {
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
    let raw = obj
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

fn find_attribute<'a>(
    descriptors: &'a [OwnedAttributeDescriptor],
    name: &str,
) -> Option<&'a OwnedAttributeDescriptor> {
    descriptors
        .iter()
        .find(|descriptor| descriptor.field_name == name || descriptor.attr_name == name)
}

fn required_string(obj: &Map<String, Value>, key: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::from_reason(format!("{key} must be a string")))
}

fn invalid_json_error(
    kind: &'static str,
) -> impl FnOnce(serde_json::Error) -> napi::Error + 'static {
    move |error| Error::from_reason(format!("Invalid {kind} descriptor JSON: {error}"))
}

fn json_serialize_error(error: serde_json::Error) -> napi::Error {
    Error::from_reason(format!("Failed to serialize descriptor JSON: {error}"))
}

fn query_result_to_json(result: QueryResult) -> Result<String> {
    let values = match result {
        QueryResult::Ok => Vec::new(),
        QueryResult::Documents(values) | QueryResult::Rows(values) => values,
    };
    serde_json::to_string(&values).map_err(json_serialize_error)
}

fn entity_rows_to_json(rows: &[DynamicEntityRow]) -> Result<String> {
    let values: Vec<_> = rows.iter().map(entity_row_to_json).collect();
    serde_json::to_string(&values).map_err(json_serialize_error)
}

fn relation_rows_to_json(rows: &[DynamicRelationRow]) -> Result<String> {
    let values: Vec<_> = rows.iter().map(relation_row_to_json).collect();
    serde_json::to_string(&values).map_err(json_serialize_error)
}

fn entity_row_to_json(row: &DynamicEntityRow) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "iid".to_string(),
        row.iid
            .as_ref()
            .map(|iid| Value::String(iid.clone()))
            .unwrap_or(Value::Null),
    );
    obj.insert(
        "type_name".to_string(),
        row.type_name
            .as_ref()
            .map(|type_name| Value::String(type_name.clone()))
            .unwrap_or(Value::Null),
    );
    obj.insert(
        "attributes".to_string(),
        Value::Array(
            row.attributes
                .iter()
                .map(|(name, value)| {
                    Value::Array(vec![
                        Value::String(name.clone()),
                        attribute_value_to_json(value),
                    ])
                })
                .collect(),
        ),
    );
    Value::Object(obj)
}

fn relation_row_to_json(row: &DynamicRelationRow) -> Value {
    let mut obj = match entity_row_to_json(&DynamicEntityRow {
        iid: row.iid.clone(),
        type_name: row.type_name.clone(),
        attributes: row.attributes.clone(),
    }) {
        Value::Object(obj) => obj,
        _ => Map::new(),
    };
    obj.insert(
        "role_players".to_string(),
        Value::Array(
            row.role_players
                .iter()
                .map(|player| {
                    let mut obj = Map::new();
                    obj.insert(
                        "role_name".to_string(),
                        Value::String(player.role_name.clone()),
                    );
                    obj.insert(
                        "player_iid".to_string(),
                        player
                            .player_iid
                            .as_ref()
                            .map(|iid| Value::String(iid.clone()))
                            .unwrap_or(Value::Null),
                    );
                    obj.insert(
                        "player_type_name".to_string(),
                        player
                            .player_type_name
                            .as_ref()
                            .map(|type_name| Value::String(type_name.clone()))
                            .unwrap_or(Value::Null),
                    );
                    obj.insert(
                        "attributes".to_string(),
                        Value::Array(
                            player
                                .attributes
                                .iter()
                                .map(|(name, value)| {
                                    Value::Array(vec![Value::String(name.clone()), value.clone()])
                                })
                                .collect(),
                        ),
                    );
                    Value::Object(obj)
                })
                .collect(),
        ),
    );
    Value::Object(obj)
}

fn attribute_value_to_json(value: &AttributeValue) -> Value {
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

/// Build [`type_bridge_orm::ConnectOptions`] from optional parameters coming
/// across the NAPI boundary.
///
/// NAPI does not expose `u16` natively; callers pass an optional `u32` and we
/// clamp it here.  `None` → default (8000).  An out-of-range value is a caller
/// error, not a configuration error, so we return a hard `InvalidArg`.
fn napi_connect_options(
    http_port: Option<u32>,
    server_version: Option<String>,
) -> Result<type_bridge_orm::ConnectOptions> {
    let mut opts = type_bridge_orm::ConnectOptions::default();
    if let Some(port) = http_port {
        opts.http_port = u16::try_from(port).map_err(|_| {
            Error::new(
                Status::InvalidArg,
                format!("httpPort {port} is out of the valid port range (0–65535)"),
            )
        })?;
    }
    if let Some(version) = server_version {
        opts.server_version = Some(version.parse::<core_version::Version>().map_err(|error| {
            Error::new(
                Status::InvalidArg,
                format!("serverVersion must be a TypeDB semantic version: {error}"),
            )
        })?);
    }
    Ok(opts)
}

fn napi_orm_error(error: OrmError) -> napi::Error {
    match error {
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

    fn person_descriptor_json() -> String {
        r#"{
            "type_name": "person",
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "name",
                    "attr_name": "person-name",
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": false
                },
                {
                    "field_name": "age",
                    "attr_name": "age",
                    "value_type": "long",
                    "annotations": [],
                    "is_optional": true
                },
                {
                    "field_name": "scores",
                    "attr_name": "score",
                    "value_type": "double",
                    "annotations": [{"Card": [0, null]}],
                    "is_optional": true
                }
            ]
        }"#
        .to_string()
    }

    fn relation_descriptor_json() -> String {
        r#"{
            "type_name": "employment",
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "since",
                    "attr_name": "since",
                    "value_type": "date",
                    "annotations": [],
                    "is_optional": true
                }
            ],
            "roles": [
                {
                    "role_name": "employee",
                    "player_type_names": ["person"],
                    "cardinality": [1, 1]
                },
                {
                    "role_name": "employer",
                    "player_type_names": ["company"],
                    "cardinality": [1, 1]
                }
            ]
        }"#
        .to_string()
    }

    #[test]
    fn entity_descriptor_json_round_trips_through_registry() {
        let registry = NodeDescriptorRegistry::new();
        let descriptor = person_descriptor_json();

        let registered = registry
            .register_entity_json(descriptor)
            .expect("entity descriptor should register");
        let fetched = registry
            .entity_json("person".to_string())
            .expect("entity descriptor should be found");

        assert_eq!(registered, fetched);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&registered).unwrap()["type_name"],
            "person"
        );
    }

    #[test]
    fn relation_descriptor_json_round_trips_through_registry() {
        let registry = NodeDescriptorRegistry::new();
        let descriptor = relation_descriptor_json();

        let registered = registry
            .register_relation_json(descriptor)
            .expect("relation descriptor should register");
        let fetched = registry
            .relation_json("employment".to_string())
            .expect("relation descriptor should be found");
        let snapshot = registry
            .snapshot_json()
            .expect("snapshot should serialize after registration");

        assert_eq!(registered, fetched);
        assert!(snapshot.contains(r#""kind":"relation""#));
        assert!(snapshot.contains(r#""type_name":"employment""#));
    }

    #[test]
    fn attribute_value_json_uses_string_for_long_input() {
        let value = normalize_attribute_value_json(
            r#"{"value_type":"long","value":"9223372036854775807"}"#.to_string(),
        )
        .expect("bigint string should parse as i64");

        assert_eq!(
            serde_json::from_str::<Value>(&value).unwrap(),
            serde_json::json!({"Long": 9223372036854775807_i64})
        );
    }

    #[test]
    fn attribute_value_json_rejects_implicit_number_for_long() {
        let error = normalize_attribute_value_json(
            r#"{"value_type":"long","value":9007199254740993}"#.to_string(),
        )
        .expect_err("long must not accept JSON number input");

        assert!(error.reason.contains("TypeScript bigint"));
    }

    #[test]
    fn all_primitive_attribute_value_shapes_normalize() {
        let values = [
            (r#"{"value_type":"string","value":"Alice"}"#, "String"),
            (r#"{"value_type":"double","value":1.25}"#, "Double"),
            (r#"{"value_type":"boolean","value":true}"#, "Boolean"),
            (r#"{"value_type":"date","value":"2026-05-27"}"#, "Date"),
            (
                r#"{"value_type":"datetime","value":"2026-05-27T01:02:03"}"#,
                "DateTime",
            ),
            (
                r#"{"value_type":"datetime-tz","value":"2026-05-27T01:02:03Z"}"#,
                "DateTimeTZ",
            ),
            (r#"{"value_type":"decimal","value":"123.456"}"#, "Decimal"),
            (r#"{"value_type":"duration","value":"P1D"}"#, "Duration"),
        ];

        for (input, variant) in values {
            let normalized = normalize_attribute_value_json(input.to_string())
                .expect("attribute value should normalize");
            let normalized = serde_json::from_str::<Value>(&normalized).unwrap();
            assert!(normalized.get(variant).is_some());
        }
    }

    #[test]
    fn entity_attributes_are_descriptor_aware() {
        let normalized = normalize_entity_attributes_json(
            person_descriptor_json(),
            r#"{
                "name": {"value_type":"string","value":"Alice"},
                "age": {"value_type":"long","value":"42"},
                "scores": [
                    {"value_type":"double","value":1.5},
                    {"value_type":"double","value":2.5}
                ]
            }"#
            .to_string(),
        )
        .expect("attributes should normalize");

        let normalized = serde_json::from_str::<Value>(&normalized).unwrap();
        let entries = normalized.as_array().expect("attribute map is an array");
        assert_eq!(entries.len(), 4);
        assert!(entries.contains(&serde_json::json!(["person-name", {"String": "Alice"}])));
        assert!(entries.contains(&serde_json::json!(["age", {"Long": 42}])));
        assert!(entries.contains(&serde_json::json!(["score", {"Double": 1.5}])));
        assert!(entries.contains(&serde_json::json!(["score", {"Double": 2.5}])));
    }

    #[test]
    fn filters_and_aggregates_are_descriptor_aware() {
        let filters = normalize_filters_json(
            person_descriptor_json(),
            r#"[{
                "attr_name": "age",
                "operator": ">=",
                "value": {"value_type":"long","value":"30"}
            }]"#
            .to_string(),
        )
        .expect("filters should normalize");
        let aggregates = normalize_aggregates_json(
            person_descriptor_json(),
            r#"[{"result_key":"mean_score","function":"mean","attr_name":"scores"}]"#.to_string(),
        )
        .expect("aggregates should normalize");

        assert_eq!(
            serde_json::from_str::<Value>(&filters).unwrap(),
            serde_json::json!([{"attr_name": "age", "operator": ">=", "value": {"Long": 30}}])
        );
        assert_eq!(
            serde_json::from_str::<Value>(&aggregates).unwrap(),
            serde_json::json!([{"result_key": "mean_score", "function": "mean", "attr_name": "score"}])
        );
    }

    #[test]
    fn role_players_and_relation_batch_normalize() {
        let role_players = r#"[
            {
                "role_name": "employee",
                "player_type_name": "person",
                "key_attr": "person-name",
                "key_value": {"value_type":"string","value":"Alice"}
            },
            {
                "role_name": "employer",
                "player_type_name": "company",
                "iid": "0xabc"
            }
        ]"#;
        let normalized =
            normalize_role_players_json(relation_descriptor_json(), role_players.to_string())
                .expect("role players should normalize");
        let batch = normalize_relation_write_batch_json(
            relation_descriptor_json(),
            format!(
                r#"[{{
                    "attributes": {{"since": {{"value_type":"date","value":"2026-05-27"}}}},
                    "role_players": {role_players}
                }}]"#
            ),
        )
        .expect("relation batch should normalize");

        assert_eq!(
            serde_json::from_str::<Value>(&normalized).unwrap(),
            serde_json::json!([
                {
                    "role_name": "employee",
                    "player_type_name": "person",
                    "iid": null,
                    "key": ["person-name", {"String": "Alice"}]
                },
                {
                    "role_name": "employer",
                    "player_type_name": "company",
                    "iid": "0xabc",
                    "key": null
                }
            ])
        );
        assert_eq!(
            serde_json::from_str::<Value>(&batch).unwrap(),
            serde_json::json!([
                [
                    [["since", {"Date": "2026-05-27"}]],
                    [
                        {
                            "role_name": "employee",
                            "player_type_name": "person",
                            "iid": null,
                            "key": ["person-name", {"String": "Alice"}]
                        },
                        {
                            "role_name": "employer",
                            "player_type_name": "company",
                            "iid": "0xabc",
                            "key": null
                        }
                    ]
                ]
            ])
        );
    }

    #[test]
    fn transaction_type_parsing_matches_rust_facade_semantics() {
        assert_eq!(parse_tx_type("read").unwrap(), TxType::Read);
        assert_eq!(parse_tx_type(" WRITE ").unwrap(), TxType::Write);
        assert_eq!(parse_tx_type("schema").unwrap(), TxType::Schema);

        let error = parse_tx_type("bad").expect_err("unknown transaction type should fail");
        assert_eq!(error.status, Status::InvalidArg);
        assert!(
            error
                .reason
                .contains("transaction_type must be 'read', 'write', or 'schema'")
        );
    }

    #[test]
    fn query_result_json_matches_python_facade_shape() {
        assert_eq!(
            query_result_to_json(QueryResult::Ok).expect("ok result should serialize"),
            "[]"
        );
        assert_eq!(
            query_result_to_json(QueryResult::Rows(vec![
                serde_json::json!({"name": "Alice"})
            ]))
            .expect("row result should serialize"),
            r#"[{"name":"Alice"}]"#
        );
        assert_eq!(
            query_result_to_json(QueryResult::Documents(vec![serde_json::json!({"x": 1})]))
                .expect("document result should serialize"),
            r#"[{"x":1}]"#
        );
    }

    #[test]
    fn dynamic_entity_rows_use_js_safe_long_strings() {
        let row = DynamicEntityRow {
            iid: Some("0xabc".to_string()),
            type_name: Some("person".to_string()),
            attributes: vec![
                (
                    "person-name".to_string(),
                    AttributeValue::String("Alice".to_string()),
                ),
                (
                    "age".to_string(),
                    AttributeValue::Long(9_007_199_254_740_993),
                ),
            ],
        };

        assert_eq!(
            entity_rows_to_json(&[row]).expect("row should serialize"),
            r#"[{"attributes":[["person-name",{"String":"Alice"}],["age",{"Long":"9007199254740993"}]],"iid":"0xabc","type_name":"person"}]"#
        );
    }

    #[test]
    fn group_fields_are_descriptor_aware() {
        let descriptor: EntityDescriptor = serde_json::from_str(&person_descriptor_json()).unwrap();

        assert_eq!(
            group_fields_from_json_string(&descriptor.owned_attributes, r#"["name","scores"]"#)
                .expect("group fields should normalize"),
            vec!["person-name".to_string(), "score".to_string()]
        );
    }
}
