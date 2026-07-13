//! Dynamic managers backed by runtime descriptors.

use std::sync::Arc;

use crate::descriptor::{EntityDescriptor, RelationDescriptor};
use crate::dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicEntityRow, DynamicExpr, DynamicRelationRow,
    DynamicRolePlayerInput, DynamicSort,
};
use crate::error::{OrmError, Result};
use crate::filter::Filter;
use crate::session::backend::{GivenRowsSpec, GivenValue, QueryResult, TxType};
use crate::session::{Database, TransactionContext};
use crate::value::AttributeValue;

use super::hydration::{extract_count, hydrate_dynamic_entity, hydrate_dynamic_relation};
use super::query_builder;

/// CRUD manager for an entity described at runtime.
pub struct DynamicEntityManager<'db> {
    target: DynamicExecutionTarget<'db>,
    descriptor: Arc<EntityDescriptor>,
}

impl<'db> DynamicEntityManager<'db> {
    /// Create a dynamic entity manager from a registered descriptor.
    pub fn new(db: &'db Database, descriptor: Arc<EntityDescriptor>) -> Self {
        Self {
            target: DynamicExecutionTarget::Database(db),
            descriptor,
        }
    }

    /// Create a dynamic entity manager bound to an existing transaction context.
    pub fn with_transaction(tx: TransactionContext, descriptor: Arc<EntityDescriptor>) -> Self {
        Self {
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor,
        }
    }

    /// Return the descriptor used by this manager.
    pub fn descriptor(&self) -> &Arc<EntityDescriptor> {
        &self.descriptor
    }

    /// Insert one dynamic entity and return the assigned IID.
    pub async fn insert(&self, attributes: &DynamicAttributeMap) -> Result<String> {
        let typeql = query_builder::build_dynamic_entity_insert_with_iid(
            &self.descriptor,
            attributes,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC INSERT");
        let result = self.target.execute(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Insert multiple dynamic entities in one Rust transaction.
    pub async fn insert_many(&self, items: &[DynamicAttributeMap]) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Insert).await
    }

    /// Put one dynamic entity and return its IID.
    pub async fn put(&self, attributes: &DynamicAttributeMap) -> Result<String> {
        let typeql = query_builder::build_dynamic_entity_put(&self.descriptor, attributes, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC PUT");
        let result = self.target.execute(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Put multiple dynamic entities in one Rust transaction.
    pub async fn put_many(&self, items: &[DynamicAttributeMap]) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Put).await
    }

    /// Update one dynamic entity's non-key attributes.
    pub async fn update(&self, iid: Option<&str>, attributes: &DynamicAttributeMap) -> Result<()> {
        let typeql =
            query_builder::build_dynamic_entity_update(&self.descriptor, iid, attributes, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC UPDATE");
        self.target.execute(&typeql, TxType::Write).await?;
        Ok(())
    }

    /// Fetch entities matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicEntityRow>> {
        let typeql = query_builder::build_dynamic_entity_fetch(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC FETCH");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch entities matching dynamic expressions, sorting, and pagination.
    pub async fn get_with_query(
        &self,
        expressions: &[DynamicExpr],
        sorts: &[DynamicSort],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<DynamicEntityRow>> {
        let typeql = query_builder::build_dynamic_entity_expr_fetch(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR FETCH");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch exactly one entity matching equality filters.
    pub async fn get_one(&self, filters: &[Filter]) -> Result<DynamicEntityRow> {
        let rows = self.get(filters).await?;
        match rows.len() {
            0 => Err(OrmError::NotFound(format!(
                "No {} matching filters",
                self.descriptor.type_name
            ))),
            1 => Ok(rows.into_iter().next().unwrap()),
            n => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: format!("Expected 1 result, got {n}"),
            }),
        }
    }

    /// Fetch one entity by its TypeDB IID.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Option<DynamicEntityRow>> {
        let typeql = query_builder::build_dynamic_entity_fetch_by_iid(&self.descriptor, iid, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC FETCH BY IID");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => match docs.len() {
                0 => Ok(None),
                1 => hydrate_dynamic_entity(&self.descriptor, &docs[0]).map(Some),
                n => Err(OrmError::Hydration {
                    type_name: self.descriptor.type_name.clone(),
                    message: format!("Expected 0 or 1 result for IID lookup, got {n}"),
                }),
            },
            QueryResult::Ok => Ok(None),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
    }

    /// Fetch all entities for this descriptor.
    pub async fn all(&self) -> Result<Vec<DynamicEntityRow>> {
        self.get(&[]).await
    }

    /// Count entities for this descriptor.
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Count entities matching equality filters.
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let typeql = query_builder::build_dynamic_entity_count(&self.descriptor, filters, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC COUNT");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count entities matching dynamic expressions.
    pub async fn count_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        let typeql =
            query_builder::build_dynamic_entity_expr_count(&self.descriptor, expressions, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR COUNT");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Run aggregate reductions over entities matching equality filters.
    pub async fn aggregate(
        &self,
        filters: &[Filter],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_entity_aggregate(
            &self.descriptor,
            filters,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over entities matching dynamic expressions.
    pub async fn aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_entity_expr_aggregate(
            &self.descriptor,
            expressions,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run grouped aggregate reductions over entities matching equality filters.
    pub async fn group_by_aggregate(
        &self,
        filters: &[Filter],
        group_fields: &[String],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_entity_group_by_aggregate(
            &self.descriptor,
            filters,
            group_fields,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC GROUP BY AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run grouped aggregate reductions over entities matching dynamic expressions.
    pub async fn group_by_aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        group_fields: &[String],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_entity_expr_group_by_aggregate(
            &self.descriptor,
            expressions,
            group_fields,
            aggregates,
            "$e",
        )?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC EXPR GROUP BY AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Delete one entity by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let typeql =
            query_builder::build_dynamic_entity_delete_by_iid(&self.descriptor, iid, "$e")?;
        tracing::debug!(typeql = %typeql, entity_type = %self.descriptor.type_name, "DYNAMIC DELETE");
        self.target.execute(&typeql, TxType::Write).await?;
        Ok(())
    }

    async fn write_many(
        &self,
        items: &[DynamicAttributeMap],
        operation: DynamicWriteOperation,
    ) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        // Given-stage fast path (TypeDB 3.12+): one compiled insert pipeline
        // over all rows, values passed through the driver API instead of
        // being interpolated per row. Requires a homogeneous batch; anything
        // else falls through to the per-row path below.
        if matches!(operation, DynamicWriteOperation::Insert)
            && let DynamicExecutionTarget::Database(db) = &self.target
            && db.supports_given_stage()
            && let Some((typeql, rows)) = given_entity_insert(&self.descriptor, items, "$e")
        {
            tracing::debug!(
                typeql = %typeql,
                entity_type = %self.descriptor.type_name,
                rows = items.len(),
                "DYNAMIC GIVEN INSERT MANY"
            );
            let result = db.execute_with_rows(&typeql, TxType::Write, rows).await?;
            return extract_insert_iids(&self.descriptor.type_name, result, items.len());
        }

        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = DynamicEntityManager::with_transaction(
                    tx.clone(),
                    Arc::clone(&self.descriptor),
                );
                let mut iids = Vec::with_capacity(items.len());
                for item in items {
                    match manager.write_one(item, operation).await {
                        Ok(iid) => iids.push(iid),
                        Err(error) => {
                            let _ = tx.rollback().await;
                            return Err(error);
                        }
                    }
                }
                tx.commit().await?;
                Ok(iids)
            }
            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                let mut iids = Vec::with_capacity(items.len());
                for item in items {
                    iids.push(self.write_one(item, operation).await?);
                }
                Ok(iids)
            }
        }
    }

    async fn write_one(
        &self,
        item: &DynamicAttributeMap,
        operation: DynamicWriteOperation,
    ) -> Result<String> {
        match operation {
            DynamicWriteOperation::Insert => self.insert(item).await,
            DynamicWriteOperation::Put => self.put(item).await,
        }
    }

    fn hydrate_documents(&self, result: QueryResult) -> Result<Vec<DynamicEntityRow>> {
        match result {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_entity(&self.descriptor, doc))
                .collect(),
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
    }
}

/// CRUD manager for a relation described at runtime.
pub struct DynamicRelationManager<'db> {
    target: DynamicExecutionTarget<'db>,
    descriptor: Arc<RelationDescriptor>,
}

impl<'db> DynamicRelationManager<'db> {
    /// Create a dynamic relation manager from a registered descriptor.
    pub fn new(db: &'db Database, descriptor: Arc<RelationDescriptor>) -> Self {
        Self {
            target: DynamicExecutionTarget::Database(db),
            descriptor,
        }
    }

    /// Create a dynamic relation manager bound to an existing transaction context.
    pub fn with_transaction(tx: TransactionContext, descriptor: Arc<RelationDescriptor>) -> Self {
        Self {
            target: DynamicExecutionTarget::Transaction(tx),
            descriptor,
        }
    }

    /// Return the descriptor used by this manager.
    pub fn descriptor(&self) -> &Arc<RelationDescriptor> {
        &self.descriptor
    }

    /// Insert one dynamic relation and return the assigned IID.
    pub async fn insert(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<String> {
        let typeql = query_builder::build_dynamic_relation_insert_with_iid(
            &self.descriptor,
            attributes,
            role_players,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION INSERT");
        let result = self.target.execute(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Insert multiple dynamic relations in one Rust transaction.
    pub async fn insert_many(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Insert).await
    }

    /// Put one dynamic relation and return its IID.
    pub async fn put(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<String> {
        let typeql = query_builder::build_dynamic_relation_put(
            &self.descriptor,
            attributes,
            role_players,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION PUT");
        let result = self.target.execute(&typeql, TxType::Write).await?;
        extract_insert_iid(&self.descriptor.type_name, result)
    }

    /// Put multiple dynamic relations in one Rust transaction.
    pub async fn put_many(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
    ) -> Result<Vec<String>> {
        self.write_many(items, DynamicWriteOperation::Put).await
    }

    /// Update one dynamic relation's scalar non-key attributes.
    pub async fn update(
        &self,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
    ) -> Result<()> {
        let typeql = query_builder::build_dynamic_relation_update(
            &self.descriptor,
            iid,
            attributes,
            role_players,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION UPDATE");
        self.target.execute(&typeql, TxType::Write).await?;
        Ok(())
    }

    /// Fetch relations matching equality filters.
    pub async fn get(&self, filters: &[Filter]) -> Result<Vec<DynamicRelationRow>> {
        let typeql = query_builder::build_dynamic_relation_fetch(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch relations matching dynamic expressions, sorting, and pagination.
    pub async fn get_with_query(
        &self,
        expressions: &[DynamicExpr],
        sorts: &[DynamicSort],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<DynamicRelationRow>> {
        let typeql = query_builder::build_dynamic_relation_expr_fetch(
            &self.descriptor,
            expressions,
            sorts,
            limit,
            offset,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR FETCH");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch relations matching attribute and role-player filters.
    pub async fn get_with_role_filters(
        &self,
        filters: &[Filter],
        role_filters: &[DynamicRolePlayerInput],
    ) -> Result<Vec<DynamicRelationRow>> {
        let typeql = query_builder::build_dynamic_relation_fetch_with_role_filters(
            &self.descriptor,
            filters,
            role_filters,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH WITH ROLE FILTERS");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        self.hydrate_documents(result)
    }

    /// Fetch exactly one relation matching equality filters.
    pub async fn get_one(&self, filters: &[Filter]) -> Result<DynamicRelationRow> {
        let rows = self.get(filters).await?;
        match rows.len() {
            0 => Err(OrmError::NotFound(format!(
                "No {} matching filters",
                self.descriptor.type_name
            ))),
            1 => Ok(rows.into_iter().next().unwrap()),
            n => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: format!("Expected 1 result, got {n}"),
            }),
        }
    }

    /// Fetch relation rows by TypeDB IID.
    pub async fn get_by_iid(&self, iid: &str) -> Result<Vec<DynamicRelationRow>> {
        let typeql =
            query_builder::build_dynamic_relation_fetch_by_iid(&self.descriptor, iid, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION FETCH BY IID");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        match result {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_relation(&self.descriptor, doc))
                .collect(),
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
    }

    /// Fetch all relations for this descriptor.
    pub async fn all(&self) -> Result<Vec<DynamicRelationRow>> {
        self.get(&[]).await
    }

    /// Count relations for this descriptor.
    pub async fn count(&self) -> Result<u64> {
        self.count_with_filters(&[]).await
    }

    /// Count relations matching equality filters.
    pub async fn count_with_filters(&self, filters: &[Filter]) -> Result<u64> {
        let typeql = query_builder::build_dynamic_relation_count(&self.descriptor, filters, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION COUNT");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Count relations matching dynamic expressions.
    pub async fn count_with_query(&self, expressions: &[DynamicExpr]) -> Result<u64> {
        let typeql =
            query_builder::build_dynamic_relation_expr_count(&self.descriptor, expressions, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR COUNT");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_count(&result)
    }

    /// Run aggregate reductions over relations matching equality filters.
    pub async fn aggregate(
        &self,
        filters: &[Filter],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_relation_aggregate(
            &self.descriptor,
            filters,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run aggregate reductions over relations matching dynamic expressions.
    pub async fn aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_relation_expr_aggregate(
            &self.descriptor,
            expressions,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run grouped aggregate reductions over relations matching equality filters.
    pub async fn group_by_aggregate(
        &self,
        filters: &[Filter],
        group_fields: &[String],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_relation_group_by_aggregate(
            &self.descriptor,
            filters,
            group_fields,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION GROUP BY AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Run grouped aggregate reductions over relations matching dynamic expressions.
    pub async fn group_by_aggregate_with_query(
        &self,
        expressions: &[DynamicExpr],
        group_fields: &[String],
        aggregates: &[DynamicAggregate],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let typeql = query_builder::build_dynamic_relation_expr_group_by_aggregate(
            &self.descriptor,
            expressions,
            group_fields,
            aggregates,
            "$r",
        )?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION EXPR GROUP BY AGGREGATE");
        let result = self.target.execute(&typeql, TxType::Read).await?;
        extract_rows(&self.descriptor.type_name, result)
    }

    /// Delete one relation by IID.
    pub async fn delete_by_iid(&self, iid: &str) -> Result<()> {
        let typeql =
            query_builder::build_dynamic_relation_delete_by_iid(&self.descriptor, iid, "$r")?;
        tracing::debug!(typeql = %typeql, relation_type = %self.descriptor.type_name, "DYNAMIC RELATION DELETE");
        self.target.execute(&typeql, TxType::Write).await?;
        Ok(())
    }

    async fn write_many(
        &self,
        items: &[(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)],
        operation: DynamicWriteOperation,
    ) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        match &self.target {
            DynamicExecutionTarget::Database(db) => {
                let tx = db.transaction_context(TxType::Write).await?;
                let manager = DynamicRelationManager::with_transaction(
                    tx.clone(),
                    Arc::clone(&self.descriptor),
                );
                let mut iids = Vec::with_capacity(items.len());
                for (attributes, role_players) in items {
                    match manager.write_one(attributes, role_players, operation).await {
                        Ok(iid) => iids.push(iid),
                        Err(error) => {
                            let _ = tx.rollback().await;
                            return Err(error);
                        }
                    }
                }
                tx.commit().await?;
                Ok(iids)
            }
            DynamicExecutionTarget::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), TxType::Write)?;
                let mut iids = Vec::with_capacity(items.len());
                for (attributes, role_players) in items {
                    iids.push(self.write_one(attributes, role_players, operation).await?);
                }
                Ok(iids)
            }
        }
    }

    async fn write_one(
        &self,
        attributes: &DynamicAttributeMap,
        role_players: &[DynamicRolePlayerInput],
        operation: DynamicWriteOperation,
    ) -> Result<String> {
        match operation {
            DynamicWriteOperation::Insert => self.insert(attributes, role_players).await,
            DynamicWriteOperation::Put => self.put(attributes, role_players).await,
        }
    }

    fn hydrate_documents(&self, result: QueryResult) -> Result<Vec<DynamicRelationRow>> {
        match result {
            QueryResult::Documents(docs) => docs
                .iter()
                .map(|doc| hydrate_dynamic_relation(&self.descriptor, doc))
                .collect(),
            QueryResult::Ok => Ok(vec![]),
            QueryResult::Rows(_) => Err(OrmError::Hydration {
                type_name: self.descriptor.type_name.clone(),
                message: "Expected Documents from fetch query, got Rows".into(),
            }),
        }
    }
}

#[derive(Clone, Copy)]
enum DynamicWriteOperation {
    Insert,
    Put,
}

enum DynamicExecutionTarget<'db> {
    Database(&'db Database),
    Transaction(TransactionContext),
}

impl DynamicExecutionTarget<'_> {
    async fn execute(&self, typeql: &str, required_tx_type: TxType) -> Result<QueryResult> {
        match self {
            Self::Database(db) => db.execute_raw(typeql, required_tx_type).await,
            Self::Transaction(tx) => {
                ensure_transaction_can_execute(tx.tx_type(), required_tx_type)?;
                tx.query(typeql).await
            }
        }
    }
}

fn ensure_transaction_can_execute(active: TxType, required: TxType) -> Result<()> {
    let allowed = match required {
        TxType::Read => matches!(active, TxType::Read | TxType::Write),
        TxType::Write => active == TxType::Write,
        TxType::Schema => active == TxType::Schema,
    };
    if allowed {
        Ok(())
    } else {
        Err(OrmError::Transaction(format!(
            "Cannot execute {required:?} operation in {active:?} transaction"
        )))
    }
}

/// Build the `given`-stage bulk insert for a homogeneous entity batch.
///
/// Returns `None` when the batch cannot ride one compiled pipeline, so the
/// caller falls back to per-row inserts:
/// - an item binds an attribute the descriptor does not know,
/// - items bind different attribute sets (optional attributes present on
///   some rows only), or repeat an attribute within one row,
/// - a column's value type varies across rows, or
/// - a value type has no given-row representation (decimal, duration).
fn given_entity_insert(
    descriptor: &EntityDescriptor,
    items: &[DynamicAttributeMap],
    var: &str,
) -> Option<(String, GivenRowsSpec)> {
    let first = items.first()?;
    if first.is_empty() {
        return None;
    }

    // Column layout comes from the first item: resolved TypeDB attribute
    // names, the given type of each column, and one given variable each.
    let mut columns: Vec<(String, &'static str)> = Vec::with_capacity(first.len());
    for (name, value) in first {
        let attr = descriptor.attribute(name)?;
        let type_name = given_type_name(value)?;
        if columns
            .iter()
            .any(|(existing, _)| *existing == attr.attr_name)
        {
            return None;
        }
        columns.push((attr.attr_name.clone(), type_name));
    }

    let mut rows = Vec::with_capacity(items.len());
    for (row_index, item) in items.iter().enumerate() {
        if item.len() != columns.len() {
            return None;
        }
        let mut row: Vec<Option<GivenValue>> = vec![None; columns.len()];
        for (name, value) in item {
            let attr = descriptor.attribute(name)?;
            let index = columns
                .iter()
                .position(|(attr_name, _)| *attr_name == attr.attr_name)?;
            if row[index].is_some() || given_type_name(value)? != columns[index].1 {
                return None;
            }
            row[index] = Some(given_value(value)?);
        }
        let mut row = row.into_iter().collect::<Option<Vec<_>>>()?;
        // Synthetic trailing column: the input row index, fetched back so
        // IIDs correlate to input rows explicitly instead of relying on
        // pipeline stream order.
        row.push(GivenValue::Integer(row_index as i64));
        rows.push(row);
    }

    let mut variables: Vec<String> = (0..columns.len()).map(|i| format!("g{i}")).collect();
    let given_header = columns
        .iter()
        .zip(&variables)
        .map(|((_, type_name), variable)| format!("${variable}: {type_name}"))
        .chain(std::iter::once("$g_row: integer".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let has_clauses = columns
        .iter()
        .zip(&variables)
        .map(|((attr_name, _), variable)| format!("has {attr_name} == ${variable}"))
        .collect::<Vec<_>>()
        .join(", ");
    variables.push("g_row".to_string());
    let typeql = format!(
        "given {given_header};\ninsert {var} isa {}, {has_clauses};\nfetch {{ \"iid\": iid({var}), \"row\": $g_row }};",
        descriptor.type_name,
    );

    Some((typeql, GivenRowsSpec { variables, rows }))
}

/// The TypeQL value type name for a `given` header column, or `None` when
/// the value has no given-row representation.
fn given_type_name(value: &AttributeValue) -> Option<&'static str> {
    Some(match value {
        AttributeValue::String(_) => "string",
        AttributeValue::Long(_) => "integer",
        AttributeValue::Double(_) => "double",
        AttributeValue::Boolean(_) => "boolean",
        AttributeValue::Date(_) => "date",
        AttributeValue::DateTime(_) => "datetime",
        AttributeValue::DateTimeTZ(_) => "datetime-tz",
        AttributeValue::Decimal(_) | AttributeValue::Duration(_) => return None,
    })
}

/// Convert an attribute value into its given-row form, or `None` when the
/// value type has no given-row representation.
fn given_value(value: &AttributeValue) -> Option<GivenValue> {
    Some(match value {
        AttributeValue::String(s) => GivenValue::String(s.clone()),
        AttributeValue::Long(n) => GivenValue::Integer(*n),
        AttributeValue::Double(d) => GivenValue::Double(*d),
        AttributeValue::Boolean(b) => GivenValue::Boolean(*b),
        AttributeValue::Date(s) => GivenValue::Date(s.clone()),
        AttributeValue::DateTime(s) => GivenValue::Datetime(s.clone()),
        AttributeValue::DateTimeTZ(s) => GivenValue::DatetimeTz(s.clone()),
        AttributeValue::Decimal(_) | AttributeValue::Duration(_) => return None,
    })
}

/// Extract one IID per input row from a bulk insert + fetch result.
///
/// Each document carries the fetched-back input row index (`"row"`), so
/// IIDs are placed by explicit correlation rather than stream order.
fn extract_insert_iids(
    type_name: &str,
    result: QueryResult,
    expected: usize,
) -> Result<Vec<String>> {
    let hydration_error = |message: String| OrmError::Hydration {
        type_name: type_name.to_string(),
        message,
    };
    match result {
        QueryResult::Documents(docs) => {
            if docs.len() != expected {
                return Err(hydration_error(format!(
                    "Bulk insert returned {} documents for {expected} input rows",
                    docs.len()
                )));
            }
            let mut iids: Vec<Option<String>> = vec![None; expected];
            for doc in &docs {
                let obj = doc.as_object().ok_or_else(|| {
                    hydration_error("Expected JSON object from insert+fetch".into())
                })?;
                let iid = super::hydration::extract_scalar_string(obj, "iid")
                    .ok_or_else(|| hydration_error("No IID in bulk insert response".into()))?;
                let row_index = obj
                    .get("row")
                    .and_then(serde_json::Value::as_f64)
                    .map(|row| row as usize)
                    .ok_or_else(|| {
                        hydration_error("No row index in bulk insert response".into())
                    })?;
                let slot = iids.get_mut(row_index).ok_or_else(|| {
                    hydration_error(format!(
                        "Bulk insert row index {row_index} out of range for {expected} input rows"
                    ))
                })?;
                if slot.replace(iid).is_some() {
                    return Err(hydration_error(format!(
                        "Bulk insert returned row index {row_index} twice"
                    )));
                }
            }
            iids.into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| hydration_error("Bulk insert response missing rows".into()))
        }
        QueryResult::Ok => Err(hydration_error(
            "Expected Documents from bulk insert+fetch, got Ok".into(),
        )),
        QueryResult::Rows(_) => Err(hydration_error(
            "Expected Documents from bulk insert+fetch, got Rows".into(),
        )),
    }
}

fn extract_insert_iid(type_name: &str, result: QueryResult) -> Result<String> {
    match result {
        QueryResult::Documents(docs) => {
            let doc = docs.first().ok_or_else(|| OrmError::Hydration {
                type_name: type_name.to_string(),
                message: "Insert returned no documents".into(),
            })?;
            let obj = doc.as_object().ok_or_else(|| OrmError::Hydration {
                type_name: type_name.to_string(),
                message: "Expected JSON object from insert+fetch".into(),
            })?;
            super::hydration::extract_scalar_string(obj, "iid").ok_or_else(|| OrmError::Hydration {
                type_name: type_name.to_string(),
                message: "No IID in insert response".into(),
            })
        }
        QueryResult::Ok => Err(OrmError::Hydration {
            type_name: type_name.to_string(),
            message: "Expected Documents from insert+fetch, got Ok".into(),
        }),
        QueryResult::Rows(_) => Err(OrmError::Hydration {
            type_name: type_name.to_string(),
            message: "Expected Documents from insert+fetch, got Rows".into(),
        }),
    }
}

fn extract_rows(
    type_name: &str,
    result: QueryResult,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    match result {
        QueryResult::Rows(rows) => rows
            .into_iter()
            .map(|row| {
                row.as_object().cloned().ok_or_else(|| OrmError::Hydration {
                    type_name: type_name.to_string(),
                    message: "Expected row object from reduce query".into(),
                })
            })
            .collect(),
        QueryResult::Ok => Ok(vec![]),
        QueryResult::Documents(_) => Err(OrmError::Hydration {
            type_name: type_name.to_string(),
            message: "Expected Rows from reduce query, got Documents".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::descriptor::OwnedAttributeDescriptor;

    fn attribute(
        field_name: &str,
        attr_name: &str,
        value_type: ValueType,
    ) -> OwnedAttributeDescriptor {
        OwnedAttributeDescriptor {
            field_name: field_name.to_string(),
            attr_name: attr_name.to_string(),
            value_type,
            annotations: vec![],
            is_optional: false,
            is_ordered: false,
            doc: None,
            meta: std::collections::BTreeMap::new(),
        }
    }

    fn person_descriptor() -> EntityDescriptor {
        EntityDescriptor {
            type_name: "person".to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                attribute("name", "name", ValueType::String),
                attribute("age", "age", ValueType::Long),
            ],
            doc: None,
            meta: std::collections::BTreeMap::new(),
        }
    }

    fn item(pairs: &[(&str, AttributeValue)]) -> DynamicAttributeMap {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn given_insert_builds_typed_header_and_rows() {
        let descriptor = person_descriptor();
        let items = vec![
            item(&[
                ("name", AttributeValue::String("alice".into())),
                ("age", AttributeValue::Long(28)),
            ]),
            item(&[
                ("age", AttributeValue::Long(26)),
                ("name", AttributeValue::String("bob".into())),
            ]),
        ];

        let (typeql, spec) =
            given_entity_insert(&descriptor, &items, "$e").expect("homogeneous batch");

        assert_eq!(
            typeql,
            "given $g0: string, $g1: integer, $g_row: integer;\n\
             insert $e isa person, has name == $g0, has age == $g1;\n\
             fetch { \"iid\": iid($e), \"row\": $g_row };"
        );
        assert_eq!(
            spec.variables,
            vec!["g0".to_string(), "g1".to_string(), "g_row".to_string()]
        );
        // The second item binds in a different order; the row is normalized
        // to the first item's column layout, with the trailing row index.
        assert_eq!(
            spec.rows,
            vec![
                vec![
                    GivenValue::String("alice".into()),
                    GivenValue::Integer(28),
                    GivenValue::Integer(0),
                ],
                vec![
                    GivenValue::String("bob".into()),
                    GivenValue::Integer(26),
                    GivenValue::Integer(1),
                ],
            ]
        );
    }

    #[test]
    fn given_insert_resolves_field_names_to_attr_names() {
        let mut descriptor = person_descriptor();
        descriptor.owned_attributes[0] =
            attribute("display_name", "display-name", ValueType::String);
        let items = vec![item(&[(
            "display_name",
            AttributeValue::String("alice".into()),
        )])];

        let (typeql, _) = given_entity_insert(&descriptor, &items, "$e").expect("resolvable");
        assert!(
            typeql.contains("has display-name == $g0"),
            "field name must resolve to the TypeDB attribute name: {typeql}"
        );
    }

    #[test]
    fn given_insert_rejects_heterogeneous_attribute_sets() {
        let descriptor = person_descriptor();
        let items = vec![
            item(&[
                ("name", AttributeValue::String("alice".into())),
                ("age", AttributeValue::Long(28)),
            ]),
            item(&[("name", AttributeValue::String("bob".into()))]),
        ];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }

    #[test]
    fn given_insert_rejects_mixed_value_types_per_column() {
        let descriptor = person_descriptor();
        let items = vec![
            item(&[("age", AttributeValue::Long(28))]),
            item(&[("age", AttributeValue::Double(26.5))]),
        ];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }

    #[test]
    fn given_insert_rejects_unrepresentable_value_types() {
        let mut descriptor = person_descriptor();
        descriptor.owned_attributes[0] = attribute("price", "price", ValueType::Decimal);
        let items = vec![item(&[("price", AttributeValue::Decimal("1.50".into()))])];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }

    #[test]
    fn given_insert_rejects_repeated_attribute_within_row() {
        let descriptor = person_descriptor();
        let items = vec![item(&[
            ("name", AttributeValue::String("alice".into())),
            ("name", AttributeValue::String("also-alice".into())),
        ])];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }

    #[test]
    fn given_insert_rejects_unknown_attribute() {
        let descriptor = person_descriptor();
        let items = vec![item(&[("nickname", AttributeValue::String("al".into()))])];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }

    #[test]
    fn given_insert_rejects_empty_first_item() {
        let descriptor = person_descriptor();
        let items = vec![item(&[])];
        assert!(given_entity_insert(&descriptor, &items, "$e").is_none());
    }
}
